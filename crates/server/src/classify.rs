//! The model half of classification: one call to the Anthropic Messages API
//! asking for a structured verdict, with the header heuristic as the answer
//! whenever that call cannot be made good.
//!
//! There is no Rust SDK, so this writes what the TypeScript SDK's
//! `messages.parse` + `zodOutputFormat` put on the wire (checked against
//! `@anthropic-ai/sdk` 0.124.0, request captured from a loopback server),
//! except for the tier enum and the timeout and retry budget, and
//! does what its parser does with the reply: take every text block, read the
//! first as JSON, check it against the verdict shape.

use std::time::Duration;

use postage_core::classify::{
    MailFacts, ModelVerdict, SYSTEM_PROMPT, Verdict, VerdictParseError, classify_from_headers,
    sanitized_reason, user_content,
};
use postage_shared::Tier;
use serde_json::{Value, json};

use crate::config::{ConfigError, required};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Written exactly as it is in the TypeScript gateway.
const MODEL: &str = "claude-opus-5";
const MAX_TOKENS: u32 = 2048;
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// What one attempt may take. Far below the SDK's 10 minute default: a
/// classifier that has not answered in 20s is worth less than the header
/// check it would delay.
pub const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(20);
/// Retries after the first attempt, so two attempts in all.
pub const MAX_RETRIES: u32 = 1;
/// The SDK's backoff: 0.5s, doubling, never more than 8s.
const SDK_RETRY_START: Duration = Duration::from_millis(500);
const SDK_RETRY_CEILING: Duration = Duration::from_secs(8);

/// An error body longer than this is cut before it reaches a log line.
const ERROR_BODY_LOG_CAP: usize = 500;

#[derive(Debug, thiserror::Error)]
pub enum ClassifyError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("{0}")]
    Transport(String),
    #[error("{status} {body}")]
    Api { status: u16, body: String },
    #[error("Classifier response was not a message: {0}")]
    Response(String),
    #[error("Classifier returned no parsed output")]
    NoParsedOutput,
    #[error(transparent)]
    Parse(#[from] VerdictParseError),
}

/// Classifies mail with the model, degrading to the header heuristic.
#[derive(Debug, Clone)]
pub struct Classifier {
    client: reqwest::Client,
    /// A missing key is not fatal at construction: like the TypeScript
    /// gateway, the first call fails with the reason and degrades, and the log
    /// line says which setting is missing.
    api_key: Result<String, ConfigError>,
    base_url: String,
    timeout: Duration,
    max_retries: u32,
    retry_start: Duration,
}

impl Classifier {
    pub fn new(api_key: Result<String, ConfigError>, base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::default(),
            api_key,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            timeout: ATTEMPT_TIMEOUT,
            max_retries: MAX_RETRIES,
            retry_start: SDK_RETRY_START,
        }
    }

    /// `ANTHROPIC_API_KEY`, and `ANTHROPIC_BASE_URL` when set (the SDK reads
    /// the same name; tests and proxies point it elsewhere).
    pub fn from_env<F>(env: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let base_url = env("ANTHROPIC_BASE_URL")
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        Self::new(required(&env, "ANTHROPIC_API_KEY"), base_url)
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_retries(mut self, max_retries: u32, first_delay: Duration) -> Self {
        self.max_retries = max_retries;
        self.retry_start = first_delay;
        self
    }

    /// The model's verdict, or the header heuristic's when the model cannot
    /// give one. The reason goes to stderr, never the mail itself.
    pub async fn classify(&self, mail: &MailFacts) -> Verdict {
        self.classify_reporting(mail, |reason| {
            eprintln!("classification degraded to header-only fallback: reason={reason}");
        })
        .await
    }

    /// [`Classifier::classify`] with the degradation reason handed to `report`
    /// instead of stderr, so a test can read what would have been logged.
    pub async fn classify_reporting(&self, mail: &MailFacts, report: impl FnOnce(&str)) -> Verdict {
        match self.classify_with_model(mail).await {
            Ok(verdict) => verdict,
            Err(cause) => {
                // An email gateway that stops delivering when its classifier is
                // down is worse than one that falls back to what the headers
                // already told it, but the fallback must not be silent.
                report(&sanitized_reason(&cause));
                classify_from_headers(mail)
            }
        }
    }

    async fn classify_with_model(&self, mail: &MailFacts) -> Result<Verdict, ClassifyError> {
        let api_key = self.api_key.clone()?;
        let body = request_body(mail).to_string();
        let reply = self.post_with_retries(&api_key, body).await?;
        Ok(parse_reply(&reply)?.into_verdict())
    }

    async fn post_with_retries(
        &self,
        api_key: &str,
        body: String,
    ) -> Result<Vec<u8>, ClassifyError> {
        let mut delay = self.retry_start;
        let mut retries_left = self.max_retries;
        loop {
            let outcome = self.post_once(api_key, body.clone()).await;
            let retryable = match &outcome {
                Ok(_) => false,
                Err((error, should_retry)) => should_retry.unwrap_or(is_retryable(error)),
            };
            if !retryable || retries_left == 0 {
                return outcome.map_err(|(error, _)| error);
            }
            retries_left -= 1;
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(SDK_RETRY_CEILING);
        }
    }

    /// One attempt. On failure, also what the server's `x-should-retry` header
    /// said, which overrides the status-based rule when present.
    async fn post_once(
        &self,
        api_key: &str,
        body: String,
    ) -> Result<Vec<u8>, (ClassifyError, Option<bool>)> {
        let transport = |error: reqwest::Error| {
            (
                ClassifyError::Transport(error.without_url().to_string()),
                None,
            )
        };
        let response = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .timeout(self.timeout)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .body(body)
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        let should_retry = response
            .headers()
            .get("x-should-retry")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| match value {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            });
        let bytes = response.bytes().await.map_err(transport)?.to_vec();
        if status.is_success() {
            return Ok(bytes);
        }
        let body: String = String::from_utf8_lossy(&bytes)
            .chars()
            .take(ERROR_BODY_LOG_CAP)
            .collect();
        Err((
            ClassifyError::Api {
                status: status.as_u16(),
                body,
            },
            should_retry,
        ))
    }
}

/// The SDK's rule: connection failures and timeouts, 408, 409, 429 and 5xx.
fn is_retryable(error: &ClassifyError) -> bool {
    match error {
        ClassifyError::Transport(_) => true,
        ClassifyError::Api { status, .. } => matches!(status, 408 | 409 | 429 | 500..),
        _ => false,
    }
}

/// The JSON schema for the verdict, as sent. `tier` carries a real `enum`
/// built from the shared `Tier`, so the list cannot drift from the type the
/// reply is parsed into. (The TypeScript SDK's schema transform moved the enum
/// and the root `$schema` URI into `description` text; both were residue of
/// that transform, not intent, and are not reproduced.)
fn verdict_schema() -> Value {
    let tiers: Vec<&str> = Tier::ALL.iter().map(|tier| tier.as_str()).collect();
    json!({
        "type": "object",
        "properties": {
            "tier": { "type": "string", "enum": tiers },
            "confidence": { "type": "number" },
            "reasons": { "type": "array", "items": { "type": "string" } },
        },
        "additionalProperties": false,
        "required": ["tier", "confidence", "reasons"],
    })
}

pub fn request_body(mail: &MailFacts) -> Value {
    json!({
        "model": MODEL,
        "max_tokens": MAX_TOKENS,
        "system": SYSTEM_PROMPT,
        "messages": [{ "role": "user", "content": user_content(mail) }],
        "output_config": {
            "format": { "type": "json_schema", "schema": verdict_schema() },
            "effort": "low",
        },
    })
}

/// What `messages.parse` does with a reply: every text block is parsed (one
/// that fails fails the call) and the first parsed one is the output.
fn parse_reply(reply: &[u8]) -> Result<ModelVerdict, ClassifyError> {
    let message: Value = serde_json::from_slice(reply)
        .map_err(|error| ClassifyError::Response(error.to_string()))?;
    let blocks = message
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| ClassifyError::Response("no content array".to_owned()))?;

    let mut first = None;
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let text = block
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| ClassifyError::Response("text block without text".to_owned()))?;
        let parsed = ModelVerdict::parse(text)?;
        first.get_or_insert(parsed);
    }
    first.ok_or(ClassifyError::NoParsedOutput)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::post;

    type LastRequest = Arc<Mutex<Option<(HeaderMap, Vec<u8>)>>>;

    /// A stand-in for the model on loopback, mirroring `web/test/model.ts`:
    /// `Some(answer)` replies with a message whose one text block is the
    /// answer as JSON; `None` refuses every request with a 400 (which is not
    /// retried, so the fallback path stays fast).
    struct StubModel {
        base_url: String,
        calls: Arc<AtomicUsize>,
        last_request: LastRequest,
        server: tokio::task::JoinHandle<()>,
    }

    #[derive(Clone)]
    struct Script {
        /// Statuses served before the real answer, one per call.
        failures_first: Arc<Mutex<Vec<u16>>>,
        answer: Option<Value>,
        raw_reply: Option<String>,
        calls: Arc<AtomicUsize>,
        last_request: LastRequest,
    }

    fn message_saying(text: &str) -> Value {
        json!({
            "id": "msg_stub",
            "type": "message",
            "role": "assistant",
            "model": "claude-opus-5",
            "content": [{ "type": "text", "text": text }],
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": { "input_tokens": 0, "output_tokens": 0 },
        })
    }

    async fn serve(
        State(script): State<Script>,
        headers: HeaderMap,
        body: Bytes,
    ) -> impl IntoResponse {
        script.calls.fetch_add(1, Ordering::SeqCst);
        *script.last_request.lock().unwrap() = Some((headers, body.to_vec()));
        let failure = {
            let mut pending = script.failures_first.lock().unwrap();
            (!pending.is_empty()).then(|| pending.remove(0))
        };
        let json_type = [("content-type", "application/json")];
        if let Some(status) = failure {
            return (
                StatusCode::from_u16(status).unwrap(),
                json_type,
                "{}".to_owned(),
            );
        }
        if let Some(raw) = script.raw_reply {
            return (StatusCode::OK, json_type, raw);
        }
        match script.answer {
            Some(answer) => (
                StatusCode::OK,
                json_type,
                message_saying(&answer.to_string()).to_string(),
            ),
            None => (
                StatusCode::BAD_REQUEST,
                json_type,
                json!({
                    "type": "error",
                    "error": {
                        "type": "invalid_request_error",
                        "message": "no model is reachable from a test",
                    },
                })
                .to_string(),
            ),
        }
    }

    impl StubModel {
        async fn start(answer: Option<Value>) -> Self {
            Self::scripted(answer, None, vec![]).await
        }

        async fn scripted(
            answer: Option<Value>,
            raw_reply: Option<String>,
            failures: Vec<u16>,
        ) -> Self {
            let calls = Arc::new(AtomicUsize::new(0));
            let last_request = Arc::new(Mutex::new(None));
            let script = Script {
                failures_first: Arc::new(Mutex::new(failures)),
                answer,
                raw_reply,
                calls: calls.clone(),
                last_request: last_request.clone(),
            };
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let app = axum::Router::new()
                .route("/v1/messages", post(serve))
                .with_state(script);
            let server = tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            Self {
                base_url: format!("http://{address}"),
                calls,
                last_request,
                server,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn classifier(&self) -> Classifier {
            Classifier::new(
                Ok("sk-ant-nothing-real-answers-this".to_owned()),
                &self.base_url,
            )
            .with_retries(2, Duration::ZERO)
        }
    }

    impl Drop for StubModel {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    fn mail() -> MailFacts {
        MailFacts {
            from: "someone@example.com".into(),
            to: "demo@usepostage.com".into(),
            subject: String::new(),
            body: String::new(),
            spf: Some("pass".into()),
            dkim: Some("pass".into()),
            dmarc: Some("pass".into()),
            urls: vec![],
        }
    }

    /// Runs one classification and returns the verdict and what was reported.
    async fn classify_logged(classifier: &Classifier, facts: &MailFacts) -> (Verdict, Vec<String>) {
        let mut logged = Vec::new();
        let verdict = classifier
            .classify_reporting(facts, |reason| logged.push(reason.to_owned()))
            .await;
        (verdict, logged)
    }

    /// The bug this trio of tests exists to catch: a classifier outage used to
    /// degrade every verdict with nothing in the log to say why.
    #[tokio::test]
    async fn when_the_model_is_unreachable_classify_still_returns_the_header_derived_degraded_verdict()
     {
        let model = StubModel::start(None).await;
        let facts = MailFacts {
            dmarc: Some("fail".into()),
            ..mail()
        };

        let (verdict, _) = classify_logged(&model.classifier(), &facts).await;

        assert_eq!(verdict, classify_from_headers(&facts));
        assert!(verdict.degraded);
        assert_eq!(model.calls(), 1, "a refusal is not retried");
    }

    #[tokio::test]
    async fn when_the_model_is_unreachable_classify_logs_why_instead_of_failing_silently() {
        let model = StubModel::start(None).await;

        let (_, logged) = classify_logged(&model.classifier(), &mail()).await;

        assert_eq!(logged.len(), 1);
        assert!(
            !logged[0].is_empty(),
            "a degraded verdict must say why, not log an empty reason"
        );
        assert!(logged[0].contains("400"), "{}", logged[0]);
    }

    #[tokio::test]
    async fn the_reason_logged_for_a_degraded_verdict_never_carries_the_mails_subject_or_body() {
        let model = StubModel::start(None).await;
        let facts = MailFacts {
            subject: "quarterly board minutes".into(),
            body: "the acquisition price is $40M".into(),
            ..mail()
        };

        let (_, logged) = classify_logged(&model.classifier(), &facts).await;
        let reason = &logged[0];

        assert!(!reason.contains("quarterly board minutes"));
        assert!(!reason.contains("acquisition price"));
    }

    #[tokio::test]
    async fn a_missing_api_key_degrades_and_names_the_setting_without_calling_out() {
        let model = StubModel::start(None).await;
        let classifier = Classifier::from_env(|name| {
            (name == "ANTHROPIC_BASE_URL").then(|| model.base_url.clone())
        });

        let (verdict, logged) = classify_logged(&classifier, &mail()).await;

        assert!(verdict.degraded);
        assert_eq!(logged, ["ANTHROPIC_API_KEY is not set"]);
        assert_eq!(model.calls(), 0);
    }

    #[tokio::test]
    async fn a_model_answer_becomes_a_tidied_non_degraded_verdict() {
        let model = StubModel::start(Some(json!({
            "tier": "important",
            "confidence": 0.9,
            "reasons": ["sent from a bulk mail platform.", "one two three four five six seven eight nine"],
            "unrequested": true,
        })))
        .await;

        let (verdict, logged) = classify_logged(&model.classifier(), &mail()).await;

        assert_eq!(
            verdict,
            Verdict {
                tier: Tier::Important,
                confidence: 0.9,
                reasons: vec![
                    "sent from a bulk mail platform".to_owned(),
                    "one two three four five six seven eight".to_owned(),
                ],
                degraded: false,
            }
        );
        assert!(logged.is_empty());
    }

    #[tokio::test]
    async fn the_request_is_the_one_the_sdk_sends() {
        let model = StubModel::start(Some(
            json!({"tier": "human", "confidence": 1, "reasons": []}),
        ))
        .await;
        let facts = MailFacts {
            subject: "hello".into(),
            body: "hi there".into(),
            spf: None,
            ..mail()
        };

        model.classifier().classify(&facts).await;

        let (headers, body) = model.last_request.lock().unwrap().clone().unwrap();
        assert_eq!(headers["x-api-key"], "sk-ant-nothing-real-answers-this");
        assert_eq!(headers["anthropic-version"], "2023-06-01");
        assert_eq!(headers["content-type"], "application/json");

        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent, request_body(&facts));
        assert_eq!(sent["model"], "claude-opus-5");
        assert_eq!(sent["max_tokens"], 2048);
        assert_eq!(sent["output_config"]["effort"], "low");
        assert_eq!(sent["output_config"]["format"]["type"], "json_schema");
        assert_eq!(
            sent["output_config"]["format"]["schema"],
            json!({
                "type": "object",
                "properties": {
                    "tier": {
                        "type": "string",
                        "enum": ["human", "important", "commercial", "dangerous"]
                    },
                    "confidence": {"type": "number"},
                    "reasons": {"type": "array", "items": {"type": "string"}}
                },
                "additionalProperties": false,
                "required": ["tier", "confidence", "reasons"]
            })
        );
        assert_eq!(
            sent["messages"],
            json!([{
                "role": "user",
                "content": "From: someone@example.com\nTo: demo@usepostage.com\nSubject: hello\nSPF: unknown  DKIM: pass  DMARC: pass\nLinks: none\n\nBody:\nhi there"
            }])
        );
        assert!(
            sent["system"]
                .as_str()
                .unwrap()
                .starts_with("You classify inbound email for a gateway that charges senders.")
        );
    }

    #[tokio::test]
    async fn answers_that_do_not_fit_the_verdict_shape_degrade() {
        for reply in [
            message_saying("not json").to_string(),
            message_saying(r#"{"tier":"spam","confidence":1,"reasons":[]}"#).to_string(),
            message_saying(r#"{"tier":"human","confidence":"high","reasons":[]}"#).to_string(),
            json!({"content": []}).to_string(),
            json!({"content": [{"type": "tool_use"}]}).to_string(),
            "not a message".to_owned(),
        ] {
            let model = StubModel::scripted(None, Some(reply.clone()), vec![]).await;
            let (verdict, logged) = classify_logged(&model.classifier(), &mail()).await;
            assert!(verdict.degraded, "{reply}");
            assert_eq!(logged.len(), 1, "{reply}");
        }
    }

    #[tokio::test]
    async fn a_server_error_is_retried_and_the_later_answer_is_used() {
        let model = StubModel::scripted(
            Some(json!({"tier": "human", "confidence": 1, "reasons": []})),
            None,
            vec![529, 429],
        )
        .await;

        let (verdict, _) = classify_logged(&model.classifier(), &mail()).await;

        assert!(!verdict.degraded);
        assert_eq!(model.calls(), 3);
    }

    #[tokio::test]
    async fn retries_stop_after_the_configured_number_and_the_verdict_degrades() {
        let model = StubModel::scripted(None, None, vec![500, 500, 500, 500]).await;

        let (verdict, logged) = classify_logged(&model.classifier(), &mail()).await;

        assert!(verdict.degraded);
        assert_eq!(model.calls(), 3);
        assert!(logged[0].starts_with("500"), "{}", logged[0]);
    }

    #[tokio::test]
    async fn a_request_that_outlasts_the_timeout_degrades() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/v1/messages",
            post(|| async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                "late"
            }),
        );
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let classifier = Classifier::new(Ok("k".to_owned()), format!("http://{address}"))
            .with_timeout(Duration::from_millis(100))
            .with_retries(0, Duration::ZERO);

        let (verdict, logged) = classify_logged(&classifier, &mail()).await;
        server.abort();

        assert!(verdict.degraded);
        assert_eq!(logged.len(), 1);
    }

    #[test]
    fn the_defaults_are_twenty_seconds_per_attempt_and_one_retry() {
        let classifier =
            Classifier::from_env(|name| (name == "ANTHROPIC_API_KEY").then(|| "k".to_owned()));
        assert_eq!(classifier.base_url, "https://api.anthropic.com");
        assert_eq!(classifier.timeout, Duration::from_secs(20));
        assert_eq!(classifier.max_retries, 1);
        assert_eq!(classifier.retry_start, Duration::from_millis(500));
        assert_eq!(ATTEMPT_TIMEOUT, Duration::from_secs(20));
        assert_eq!(MAX_RETRIES, 1);
    }

    #[test]
    fn the_schemas_tier_enum_is_exactly_the_shared_tiers_in_order() {
        let schema = verdict_schema();
        let listed: Vec<&str> = schema["properties"]["tier"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap())
            .collect();
        let tiers: Vec<&str> = Tier::ALL.iter().map(|tier| tier.as_str()).collect();
        assert_eq!(listed, tiers);
        assert!(schema["properties"]["tier"].get("description").is_none());
        assert!(schema.get("description").is_none());
    }

    #[tokio::test]
    async fn a_model_slower_than_the_timeout_gets_exactly_two_attempts_then_degrades() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/v1/messages",
            post(move || {
                let seen = seen.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    "late"
                }
            }),
        );
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let attempt_timeout = Duration::from_millis(150);
        let backoff = Duration::from_millis(50);
        // The default retry count, with only the clock shortened.
        let classifier = Classifier::new(Ok("k".to_owned()), format!("http://{address}"))
            .with_timeout(attempt_timeout)
            .with_retries(MAX_RETRIES, backoff);

        let started = std::time::Instant::now();
        let (verdict, logged) = classify_logged(&classifier, &mail()).await;
        let elapsed = started.elapsed();
        server.abort();

        assert!(verdict.degraded);
        assert_eq!(logged.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(elapsed >= attempt_timeout * 2 + backoff, "{elapsed:?}");
        assert!(
            elapsed < attempt_timeout * 2 + backoff + Duration::from_secs(2),
            "{elapsed:?}"
        );
    }
}

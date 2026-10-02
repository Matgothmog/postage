//! Naming which part of the gateway failed, and answering with it
//! (`web/src/app/api/mail/inbound/faults.ts`).
//!
//! Shared by every route that has to say what broke without saying what the
//! dependency said: the inbound mail gateway answers with [`fault_response`],
//! and `/api/world/verify` runs its own log lines through [`redact`].

use std::error::Error;
use std::fmt;
use std::future::Future;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::config::{ConfigError, Env, required};
use crate::log;

/// Whatever a step failed with, as a `catch` clause would hand it over.
pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// Which part of the gateway failed. Not a verdict on the message: every one
/// of these is ours, and the same message would have been judged the same way
/// a minute earlier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FaultStage {
    Config,
    Database,
    Chain,
    Unexpected,
}

impl FaultStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Database => "database",
            Self::Chain => "chain",
            Self::Unexpected => "unexpected",
        }
    }

    /// What the caller is told, per stage.
    ///
    /// Fixed strings rather than whatever the dependency said. libsql quotes
    /// the database path it could not open, an RPC client quotes the whole URL
    /// it called, and a refusing upstream can echo any of it back: useful to
    /// whoever operates this, and this is the one that goes out over the wire.
    fn safe_message(self) -> &'static str {
        match self {
            Self::Config => "The gateway is missing a required setting",
            Self::Database => "The gateway could not reach its database",
            Self::Chain => "The gateway could not read the price floor from the chain",
            Self::Unexpected => "The gateway failed for an unexpected reason",
        }
    }
}

/// Every fault answers with the status this route has always given one.
///
/// Unchanged on purpose. The mail worker reads the status to decide a real
/// message's fate: 404 is the only code it takes as an answer rather than as
/// the gateway having fallen over, and every other non-2xx makes it refuse the
/// SMTP session, which Cloudflare emits as a permanent 5.7.1 that Gmail does
/// not hold and retry. Moving a fault onto a different code would move which
/// mail bounces. What was missing was never the code: the body was empty, so
/// the worker could log nothing after `Gateway returned 500: `.
const FAULT_STATUS: StatusCode = StatusCode::INTERNAL_SERVER_ERROR;

/// A failure of the gateway itself, carrying the part that failed and a
/// message safe to say out loud.
#[derive(Debug)]
pub struct GatewayFault {
    stage: FaultStage,
    message: String,
    cause: Option<BoxError>,
}

impl GatewayFault {
    pub fn new(stage: FaultStage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
            cause: None,
        }
    }

    fn caused_by(stage: FaultStage, message: impl Into<String>, cause: BoxError) -> Self {
        Self {
            stage,
            message: message.into(),
            cause: Some(cause),
        }
    }

    pub fn stage(&self) -> FaultStage {
        self.stage
    }
}

impl fmt::Display for GatewayFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for GatewayFault {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause
            .as_deref()
            .map(|cause| cause as &(dyn Error + 'static))
    }
}

/// Runs one step of the route under the name of the thing that can fail in
/// it, so a failure says which without anyone having to read the error's own
/// words, which are written by a dependency and cannot be relied on to mean
/// the same thing next release. A step that already failed with a fault of
/// its own keeps its own stage.
pub async fn during<T, E, F>(stage: FaultStage, step: F) -> Result<T, GatewayFault>
where
    F: Future<Output = Result<T, E>>,
    E: Into<BoxError>,
{
    step.await.map_err(|error| {
        let error: BoxError = error.into();
        match error.downcast::<GatewayFault>() {
            Ok(fault) => *fault,
            Err(cause) => GatewayFault::caused_by(stage, stage.safe_message(), cause),
        }
    })
}

/// A required variable under the same labelling, and the one fault allowed to
/// name what it was: which variable is unset is the whole diagnosis and the
/// whole fix, and a variable's name is not its value.
pub fn configured(env: &Env, name: &'static str) -> Result<String, GatewayFault> {
    required(env.lookup(), name).map_err(|cause| {
        GatewayFault::caused_by(
            FaultStage::Config,
            format!("{name} is not set on the gateway"),
            Box::new(cause),
        )
    })
}

/// The same check for a variable that has to be read before the caller has
/// proved anything, answering without saying which one it was.
///
/// `/api/world/context` states the policy: the credential is checked before
/// any other setting is read, so an anonymous caller learns nothing about how
/// this deployment is configured. A route whose credential *is* a setting
/// cannot follow it by ordering alone, so the answer names the class of
/// failure, and the name goes to the log under the cause, where only an
/// operator reads it.
pub fn configured_without_naming(env: &Env, name: &'static str) -> Result<String, GatewayFault> {
    required(env.lookup(), name).map_err(|cause: ConfigError| {
        GatewayFault::caused_by(
            FaultStage::Config,
            FaultStage::Config.safe_message(),
            Box::new(cause),
        )
    })
}

/// The same check, made before the work that needs the variables rather than
/// left to whichever library first reaches for one and reports it as a
/// malformed key rather than an unfinished deployment.
pub fn require_configured(env: &Env, names: &[&'static str]) -> Result<(), GatewayFault> {
    for name in names {
        configured(env, name)?;
    }
    Ok(())
}

/// Environment variables whose *value* must never reach a log line. Matched
/// on the name rather than kept as a list of variables, so a new variable
/// whose name says what it holds is covered the day it is added.
///
/// That is the whole guarantee, and it is narrower than it looks: coverage
/// follows the *name*, so a credential under a name holding none of these
/// words travels intact. The three URL names are that edit, made three times
/// already: `DATABASE_URL` carries whatever the driver authenticates with, a
/// keyed `*_RPC_URL` carries its key in the path, and so does a `*_QUERY_URL`.
/// `APP_URL` and `MAIL_WORKER_URL` are public and stay readable.
const SECRET_NAME_PARTS: [&str; 8] = [
    "SECRET",
    "KEY",
    "TOKEN",
    "PASSWORD",
    "CREDENTIAL",
    "DATABASE_URL",
    "RPC_URL",
    "QUERY_URL",
];

/// Shortest value worth hiding, in UTF-16 code units as JavaScript counts a
/// string's length. A one- or two-character value is not a secret, and
/// blanking every occurrence of it would shred the message it appears in.
const SHORTEST_SECRET: usize = 8;

fn is_secret_name(name: &str) -> bool {
    SECRET_NAME_PARTS.iter().any(|part| name.contains(part))
}

/// Whatever a dependency put in an error message, with this process's own
/// secrets taken back out of it. The log is where the real reason has to
/// survive, and it is also the thing that gets shipped to an aggregator.
///
/// The match is on the literal value, so a secret a dependency
/// percent-encoded into a URL before quoting it is a different string and
/// comes through. Both limits, this one and the name matching above, are
/// pinned in the tests rather than left to be discovered.
pub fn redact(env: &Env, text: &str) -> String {
    let mut safe = text.to_owned();
    for (name, value) in env.vars() {
        if value.encode_utf16().count() < SHORTEST_SECRET || !is_secret_name(&name) {
            continue;
        }
        safe = safe.replace(&value, &format!("[redacted {name}]"));
    }
    safe
}

/// How far down a chain of causes to read. Deep enough for the wrappers a
/// transport failure arrives under, short enough to stay one line.
const MAX_CAUSE_LINKS: usize = 5;

/// An error's own message, and the messages under it.
///
/// The top of that chain is routinely its least useful link: an unreachable
/// database can surface as `fetch failed` and nothing else, while the
/// sentence worth having sits one cause down.
pub fn reason_chain(error: &(dyn Error + 'static)) -> String {
    let mut links = Vec::new();
    let mut link = Some(error);
    while let Some(current) = link {
        if links.len() == MAX_CAUSE_LINKS {
            break;
        }
        links.push(current.to_string());
        link = current.source();
    }
    links.join(": ")
}

#[derive(Serialize)]
struct FaultBody<'a> {
    error: &'a str,
    fault: FaultStage,
}

/// The answer to a fault: the status this route has always given one, and a
/// body saying which part of the gateway broke.
///
/// The two halves are deliberately different. The body carries a stable stage
/// and a sentence written here, which is what the worker logs and what tells
/// an operator whether to look at the database, the chain, or the
/// environment. The real words go to the log, where they are worth having and
/// where the caller cannot read them.
pub fn fault_response(env: &Env, cause: BoxError) -> Response {
    let fault = match cause.downcast::<GatewayFault>() {
        Ok(fault) => *fault,
        Err(other) => GatewayFault::caused_by(
            FaultStage::Unexpected,
            FaultStage::Unexpected.safe_message(),
            other,
        ),
    };

    let reason = match fault.cause.as_deref() {
        Some(cause) => reason_chain(cause),
        None => reason_chain(&fault),
    };
    log::error(
        "inbound mail gateway fault",
        &[
            ("stage", &fault.stage.as_str()),
            ("reason", &redact(env, &reason)),
        ],
    );

    let body = FaultBody {
        error: &fault.message,
        fault: fault.stage,
    };
    (FAULT_STATUS, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::log::captured;

    #[derive(Debug)]
    struct Failure {
        message: &'static str,
        cause: Option<Box<Failure>>,
    }

    impl fmt::Display for Failure {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.message)
        }
    }

    impl Error for Failure {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.cause
                .as_deref()
                .map(|cause| cause as &(dyn Error + 'static))
        }
    }

    fn failure(message: &'static str) -> Failure {
        Failure {
            message,
            cause: None,
        }
    }

    async fn body_of(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// Anything that fails outside a stage the route knows about still has to
    /// arrive as a body. The fault carrying the least information is the one
    /// most worth pinning.
    #[tokio::test]
    async fn something_nobody_anticipated_still_answers_with_a_body_rather_than_nothing() {
        let response = fault_response(&Env::empty(), Box::from("a library threw a string"));

        let (status, body) = body_of(response).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            body,
            json!({
                "error": "The gateway failed for an unexpected reason",
                "fault": "unexpected",
            })
        );
    }

    /// The message on the outermost error is often the least useful thing
    /// about it: a database nobody can reach arrives as `fetch failed` and
    /// stops there.
    #[tokio::test]
    async fn a_fault_is_logged_with_the_reason_under_the_reason_not_only_the_top_of_it() {
        let env = Env::empty();
        let cause = Failure {
            message: "fetch failed",
            cause: Some(Box::new(failure("connect ECONNREFUSED 10.0.0.1:443"))),
        };

        let (_, lines) = captured::during(async { fault_response(&env, Box::new(cause)) }).await;

        assert_eq!(
            lines,
            [
                "inbound mail gateway fault stage=unexpected reason=fetch failed: connect ECONNREFUSED 10.0.0.1:443"
            ]
        );
    }

    #[tokio::test]
    async fn a_stage_does_not_relabel_a_fault_that_already_named_its_own() {
        let inner = GatewayFault::new(FaultStage::Chain, "the chain said no");

        let fault = during(FaultStage::Database, async {
            Err::<(), GatewayFault>(inner)
        })
        .await
        .unwrap_err();

        assert_eq!(fault.stage(), FaultStage::Chain);
        assert_eq!(fault.to_string(), "the chain said no");
    }

    #[tokio::test]
    async fn a_stage_passes_a_value_straight_back_when_nothing_goes_wrong() {
        let value = during(FaultStage::Database, async { Ok::<_, Failure>(7) })
            .await
            .unwrap();
        assert_eq!(value, 7);
    }

    #[tokio::test]
    async fn a_stage_labels_a_foreign_error_with_its_own_safe_message_and_keeps_the_cause() {
        let fault = during(FaultStage::Database, async {
            Err::<(), _>(failure("SQLITE_BUSY: database is locked"))
        })
        .await
        .unwrap_err();

        assert_eq!(fault.stage(), FaultStage::Database);
        assert_eq!(
            fault.to_string(),
            "The gateway could not reach its database"
        );
        assert_eq!(
            fault.source().map(ToString::to_string).as_deref(),
            Some("SQLITE_BUSY: database is locked")
        );
    }

    #[tokio::test]
    async fn a_setting_named_by_configured_is_named_in_the_answer() {
        let env = Env::empty();
        let fault = configured(&env, "APP_URL").unwrap_err();

        let (status, body) = body_of(fault_response(&env, Box::new(fault))).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            body,
            json!({ "error": "APP_URL is not set on the gateway", "fault": "config" })
        );
    }

    #[tokio::test]
    async fn a_setting_read_without_naming_answers_only_with_the_class_and_logs_the_name() {
        let env = Env::empty();
        let fault = configured_without_naming(&env, "MAIL_WEBHOOK_SECRET").unwrap_err();

        let (response, lines) =
            captured::during(async { fault_response(&env, Box::new(fault)) }).await;
        let (_, body) = body_of(response).await;

        assert_eq!(
            body,
            json!({ "error": "The gateway is missing a required setting", "fault": "config" })
        );
        assert!(!body.to_string().contains("MAIL_WEBHOOK_SECRET"));
        assert_eq!(
            lines,
            ["inbound mail gateway fault stage=config reason=MAIL_WEBHOOK_SECRET is not set"]
        );
    }

    #[test]
    fn a_set_setting_is_returned_and_require_configured_checks_each_in_turn() {
        let env = Env::fixed([("APP_URL", "http://localhost")]);
        assert_eq!(configured(&env, "APP_URL").unwrap(), "http://localhost");
        let fault = require_configured(&env, &["APP_URL", "CLASSIFIER_PRIVATE_KEY"]).unwrap_err();
        assert_eq!(
            fault.to_string(),
            "CLASSIFIER_PRIVATE_KEY is not set on the gateway"
        );
    }

    /// `redact` exists because the message on an error is written by whoever
    /// raised it: libsql quotes the database path it could not open, an RPC
    /// client quotes the URL it called, and a refusing upstream can echo any of
    /// it back. All of that belongs in a log and none of it may carry a
    /// credential there.
    #[test]
    fn a_secrets_value_is_taken_out_of_a_message_that_quoted_it() {
        let env = Env::fixed([("TEST_REDACT_API_KEY", "sk-live-0123456789abcdef")]);
        assert_eq!(
            redact(
                &env,
                "call to https://rpc.example/sk-live-0123456789abcdef failed"
            ),
            "call to https://rpc.example/[redacted TEST_REDACT_API_KEY] failed"
        );
    }

    #[test]
    fn every_occurrence_goes_not_just_the_first() {
        let env = Env::fixed([("TEST_REDACT_SECRET", "hunter2-hunter2")]);
        assert_eq!(
            redact(&env, "hunter2-hunter2 then hunter2-hunter2"),
            "[redacted TEST_REDACT_SECRET] then [redacted TEST_REDACT_SECRET]"
        );
    }

    /// A connection string is not named like a secret and routinely carries
    /// one.
    #[test]
    fn a_database_url_is_treated_as_a_secret_even_though_it_is_not_named_like_one() {
        let env = Env::fixed([("DATABASE_URL", "libsql://postage-demo.turso.io")]);
        assert_eq!(
            redact(&env, "could not open libsql://postage-demo.turso.io"),
            "could not open [redacted DATABASE_URL]"
        );
    }

    /// The variables saying what this deployment is, rather than what it
    /// knows, stay readable: losing them would cost a diagnosis and protect
    /// nothing.
    #[test]
    fn a_public_url_is_left_alone() {
        let env = Env::fixed([("APP_URL", "https://postage-seven.vercel.app")]);
        assert_eq!(
            redact(&env, "could not reach https://postage-seven.vercel.app"),
            "could not reach https://postage-seven.vercel.app"
        );
    }

    #[test]
    fn a_value_too_short_to_be_a_secret_does_not_shred_the_message_it_appears_in() {
        let env = Env::fixed([("TEST_REDACT_TOKEN", "ab")]);
        assert_eq!(redact(&env, "a fabulous database"), "a fabulous database");
    }

    #[test]
    fn a_message_with_nothing_to_hide_comes_back_unchanged() {
        let env = Env::fixed([("TEST_REDACT_SECRET", "hunter2-hunter2")]);
        assert_eq!(
            redact(&env, "the server is not accepting queries"),
            "the server is not accepting queries"
        );
    }

    /// A Graph Studio query URL holds its API key in the path, and
    /// `GRAPH_QUERY_URL` is no more named like a secret than `DATABASE_URL`.
    #[test]
    fn the_subgraph_query_url_is_treated_as_a_secret_even_though_it_is_not_named_like_one() {
        let env = Env::fixed([(
            "GRAPH_QUERY_URL",
            "https://gateway.thegraph.com/api/0123456789abcdef/subgraphs/id/Qm1",
        )]);
        assert_eq!(
            redact(
                &env,
                "Graph query failed: https://gateway.thegraph.com/api/0123456789abcdef/subgraphs/id/Qm1"
            ),
            "Graph query failed: [redacted GRAPH_QUERY_URL]"
        );
    }

    /// The limit the matching design actually has, pinned rather than left
    /// implied: coverage is by name, so a credential under a name holding none
    /// of the listed words travels intact.
    #[test]
    fn a_secret_under_a_name_the_pattern_does_not_know_is_left_in_the_message() {
        let env = Env::fixed([(
            "TEST_REDACT_ENDPOINT",
            "https://operator:hunter2-hunter2@db.example",
        )]);
        assert_eq!(
            redact(
                &env,
                "could not open https://operator:hunter2-hunter2@db.example"
            ),
            "could not open https://operator:hunter2-hunter2@db.example"
        );
    }

    /// The second limit: the match is on the literal value. A dependency that
    /// percent-encodes the URL it failed on has written a different string.
    #[test]
    fn a_value_the_message_re_encoded_on_its_way_in_is_not_caught() {
        let env = Env::fixed([("TEST_REDACT_SECRET", "hunter2/hunter2")]);
        assert_eq!(
            redact(&env, "POST https://rpc.example/hunter2%2Fhunter2 failed"),
            "POST https://rpc.example/hunter2%2Fhunter2 failed"
        );
    }

    #[test]
    fn a_reason_chain_stops_after_five_links() {
        let mut error = failure("6");
        for message in ["5", "4", "3", "2", "1"] {
            error = Failure {
                message,
                cause: Some(Box::new(error)),
            };
        }
        assert_eq!(reason_chain(&error), "1: 2: 3: 4: 5");
    }
}

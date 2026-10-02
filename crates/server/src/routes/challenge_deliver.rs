//! `POST /api/challenge/deliver` (`web/src/app/api/challenge/deliver/route.ts`):
//! delivers a message the sender pastes back in.
//!
//! The way through is normally the held message, released as the bytes that
//! arrived. This is what is left when there is nothing to release: a hold that
//! ran out, a sender refused inside the session rather than held, or a relay
//! that would not take it. What goes out here is written in our form, so it
//! goes under our name. It is not a relay anyone can use: it spends the same
//! pass an inbound message would have spent.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use postage_core::classify::{MailFacts, extract_urls};
use postage_core::handle::postage_address;
use postage_shared::Tier;
use serde::Serialize;
use serde_json::Value;

use super::js::{TypeError, is_truthy, js_length, js_trim, lookup_text, property, request_json};
use super::{Exit, RouteResult, json, refuse};
use crate::app::AppState;
use crate::db::Db;
use crate::db::challenges::{Challenge, challenge_by_token};
use crate::db::classifications::{claim_classification, release_classification_slot};
use crate::db::inboxes::{Inbox, inbox_by_handle};
use crate::db::passes::{extend_pass_if_expiring, has_live_pass, refund_pass, spend_pass};
use crate::mail::HeldMessage;

const MAX_SUBJECT: f64 = 200.0;
const MAX_BODY: usize = 20_000;
const DANGEROUS: &str = "dangerous";
const PASS_RUN_OUT: &str = "That pass has run out. Prove you are a person again, or pay";

#[derive(Serialize)]
struct Delivered<'a> {
    status: &'static str,
    to: &'a str,
}

/// What the sender pasted, read the way the TypeScript read it.
struct Paste<'a> {
    token: String,
    /// `subject?.trim()`. A subject that is not a string got past every
    /// check in the TypeScript and threw at its first `trim()`, after the
    /// budget had been charged, so the error waits until then here too.
    subject: Result<Option<String>, TypeError>,
    /// As pasted; trimmed where the TypeScript trimmed it.
    text: &'a str,
}

pub(crate) async fn post(State(state): State<AppState>, body: Bytes) -> RouteResult {
    // Unguarded in the TypeScript: a body that is not JSON threw, which
    // Next.js answered with a bare 500.
    let request = request_json(&body)?;
    let paste = read_paste(&request)?;

    let db = state.db().await?;
    let now = state.now();
    let (challenge, inbox) = cleared_challenge(db, &paste.token, now).await?;

    // The same budget the inbound path answers to, counted against its own
    // pool rather than the inbox's: sharing it let a handful of senders with
    // live passes spend a recipient's hourly allowance on refused pastes.
    let budget = format!("paste:{}", challenge.handle);
    if claim_classification(db, &budget, &challenge.sender, now)
        .await?
        .is_some()
    {
        return Err(refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "Too much has been sent this hour. Try again later",
        ));
    }

    let subject = paste.subject?;
    judge(
        &state,
        db,
        &challenge,
        subject.as_deref(),
        paste.text,
        &budget,
    )
    .await?;

    let Some(pass) = spend_pass(db, &challenge.handle, &challenge.sender, false, now).await? else {
        return Err(refuse(StatusCode::FORBIDDEN, PASS_RUN_OUT));
    };
    let relay_subject = subject
        .as_deref()
        .filter(|subject| !subject.is_empty())
        .unwrap_or("(no subject)");
    let held = HeldMessage {
        to: &inbox.destination,
        from: &challenge.sender,
        handle: &challenge.handle,
        subject: relay_subject,
        body: js_trim(paste.text),
    };
    if let Err(detail) = relay(&state, &held).await {
        // Only a counted pass had anything taken from it. An unlimited window
        // is returned untouched by `spend_pass`, so refunding here would add a
        // use nobody spent. Best effort: whatever broke the relay may break
        // this too, and the sender should still be told what went wrong.
        if pass.uses_left.is_some() {
            let _ = refund_pass(db, &challenge.handle, &challenge.sender, now).await;
        }
        return Err(refuse(StatusCode::BAD_GATEWAY, &detail));
    }

    Ok(json(
        StatusCode::OK,
        &Delivered {
            status: "delivered",
            to: &challenge.handle,
        },
    ))
}

/// `if (!token || !body?.trim())`, then the length limits.
fn read_paste(request: &Value) -> Result<Paste<'_>, Exit> {
    let token = property(request, "token")?;
    let subject = property(request, "subject")?;
    let pasted = property(request, "body")?;

    let missing = || refuse(StatusCode::BAD_REQUEST, "token and a message are required");
    let Some(token) = token.filter(|token| is_truthy(Some(token))) else {
        return Err(missing());
    };
    let text = match pasted {
        None | Some(Value::Null) => return Err(missing()),
        Some(Value::String(text)) if js_trim(text).is_empty() => return Err(missing()),
        Some(Value::String(text)) => text,
        Some(_) => return Err(TypeError("body?.trim is not a function".to_owned()).into()),
    };
    if js_length(text) > MAX_BODY || subject_length(subject) > MAX_SUBJECT {
        return Err(refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            "That message is too long to send this way",
        ));
    }
    Ok(Paste {
        token: lookup_text(token)?,
        subject: trimmed_subject(subject),
        text,
    })
}

/// The challenge this paste answers and the inbox it goes to, provided the
/// gate was cleared, the mail is not dangerous, and the sender's pass is live.
async fn cleared_challenge(db: &Db, token: &str, now: i64) -> Result<(Challenge, Inbox), Exit> {
    let Some(challenge) = challenge_by_token(db, token).await? else {
        return Err(refuse(StatusCode::NOT_FOUND, "Unknown challenge"));
    };
    if challenge
        .resolved_at
        .is_none_or(|resolved_at| resolved_at == 0)
    {
        return Err(refuse(StatusCode::FORBIDDEN, "Clear the gate first"));
    }

    // What was pasted is not what was judged. The verdict on this token
    // describes the message that was held; the box below it accepts anything,
    // and the pass that authorises the send belongs to the sender rather than
    // to this token. Both are checked: the tier this token was given here, and
    // the words actually about to be delivered in `judge`.
    if challenge.tier == DANGEROUS {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "This will not be delivered whoever sends it, and paying did not buy that",
        ));
    }

    let Some(inbox) = inbox_by_handle(db, &challenge.handle).await? else {
        return Err(refuse(StatusCode::NOT_FOUND, "That inbox no longer exists"));
    };

    // Checked without being spent, so the classifier cannot be run up by
    // anyone holding a token whose pass is long gone.
    if !has_live_pass(db, &challenge.handle, &challenge.sender, now).await? {
        return Err(refuse(StatusCode::FORBIDDEN, PASS_RUN_OUT));
    }
    Ok((challenge, inbox))
}

/// Refuses a paste the model reads as dangerous, or one nobody can judge.
/// Judged before anything is spent, so a refusal costs the sender nothing and
/// a false positive does not burn a delivery they paid for.
async fn judge(
    state: &AppState,
    db: &Db,
    challenge: &Challenge,
    subject: Option<&str>,
    text: &str,
    budget: &str,
) -> Result<(), Exit> {
    let verdict = state
        .classifier()
        .classify(&MailFacts {
            from: challenge.sender.clone(),
            to: postage_address(&challenge.handle),
            subject: subject.unwrap_or_default().to_owned(),
            body: js_trim(text).to_owned(),
            spf: None,
            dkim: None,
            dmarc: None,
            urls: extract_urls(text),
        })
        .await;

    if verdict.tier == Tier::Dangerous {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "That reads as an attempt to deceive the recipient, so it will not be sent",
        ));
    }

    // A verdict from the headers alone cannot say "dangerous" at all, and this
    // text has no headers to read. Relaying it under our own name while unable
    // to judge it is how a gateway lends its reputation to whatever it is
    // handed. The reason we cannot judge it is ours, so the budget slot goes
    // back and the window is pushed out if it is nearly gone.
    if verdict.degraded {
        let now = state.now();
        release_classification_slot(db, budget, &challenge.sender).await?;
        extend_pass_if_expiring(db, &challenge.handle, &challenge.sender, now).await?;
        return Err(refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "Cannot check that right now. Try again in a few minutes",
        ));
    }
    Ok(())
}

/// Sends the paste through Resend, or says why not. Missing settings are a
/// failed send, worded as `required()` worded them inside the TypeScript's
/// fetch.
async fn relay(state: &AppState, held: &HeldMessage<'_>) -> Result<(), String> {
    match state.mailer() {
        Ok(mailer) => mailer
            .relay_held_message(held)
            .await
            .map_err(|error| error.to_string()),
        Err(missing) => Err(missing.to_string()),
    }
}

/// `subject?.length ?? 0`, compared as JavaScript compares it: a string's
/// UTF-16 length, an array's element count, an object's own numeric `length`.
fn subject_length(subject: Option<&Value>) -> f64 {
    match subject {
        Some(Value::String(text)) => js_length(text) as f64,
        Some(Value::Array(items)) => items.len() as f64,
        Some(Value::Object(fields)) => fields.get("length").and_then(Value::as_f64).unwrap_or(0.0),
        _ => 0.0,
    }
}

/// `subject?.trim()`: `None` for an absent subject, the trimmed text for a
/// string, and the `TypeError` anything else threw.
fn trimmed_subject(subject: Option<&Value>) -> Result<Option<String>, TypeError> {
    match subject {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(js_trim(text).to_owned())),
        Some(_) => Err(TypeError("subject?.trim is not a function".to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::classify::Classifier;
    use crate::config::Env;
    use crate::db::challenges::claim_challenge;
    use crate::db::inboxes::create_inbox;
    use crate::db::passes::grant_pass;
    use crate::db::testing::TestDb;
    use crate::http_stub::{Reply, Stub, closed_port, serve_with};
    use crate::mail::Mailer;
    use crate::routes::router;
    use crate::routes::testing::{Answer, NOW, challenge, clock_at, post, seed, send};

    const PATH: &str = "/api/challenge/deliver";
    const HANDLE: &str = "demo";
    const SENDER: &str = "sender@x.com";

    /// The model's answer, in the shape the Messages API wraps it in.
    fn model_saying(tier: &str) -> String {
        let verdict = json!({
            "tier": tier,
            "confidence": 0.9,
            "reasons": ["a stub answers for the model here"],
        });
        json!({
            "id": "msg_stub",
            "type": "message",
            "role": "assistant",
            "model": "claude-opus-5",
            "content": [{ "type": "text", "text": verdict.to_string() }],
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": { "input_tokens": 0, "output_tokens": 0 },
        })
        .to_string()
    }

    struct Fixture {
        db: Arc<TestDb>,
        model: Stub,
        resend: Stub,
        classifier_base: String,
    }

    /// An inbox, a commercial challenge, a model answering `tier`, and a
    /// Resend that accepts unless `resend_status` says otherwise.
    async fn fixture(tier: &'static str, resend_status: u16) -> Fixture {
        let db = Arc::new(TestDb::fresh().await);
        create_inbox(
            &db,
            HANDLE,
            "demo@example.com",
            Some(&format!("0x{}", "11".repeat(20))),
            NOW,
        )
        .await
        .unwrap();
        seed(&db, &challenge("tok", "commercial")).await;
        let model = serve_with(move |_| Reply::new(200, model_saying(tier))).await;
        let resend = serve_with(move |_| match resend_status {
            200 => Reply::new(200, r#"{"id":"stub"}"#),
            status => Reply::new(status, "mailbox unavailable"),
        })
        .await;
        Fixture {
            db,
            classifier_base: model.base.clone(),
            model,
            resend,
        }
    }

    impl Fixture {
        /// A challenge already settled by paying, and the delivery that
        /// payment bought.
        async fn cleared(&self) {
            claim_challenge(&self.db, "tok", "paid", NOW).await.unwrap();
            grant_pass(&self.db, HANDLE, SENDER, "paid", Some(1), NOW)
                .await
                .unwrap();
        }

        fn app(&self) -> axum::Router {
            router(
                AppState::builder(Env::empty())
                    .clock(clock_at(NOW))
                    .db(self.db.clone())
                    .classifier(
                        Classifier::new(Ok("test".to_owned()), &self.classifier_base)
                            .with_retries(0, Duration::ZERO),
                    )
                    .mailer(
                        Mailer::new(
                            reqwest::Client::default(),
                            "test".to_owned(),
                            "Postage <hello@usepostage.com>".to_owned(),
                        )
                        .with_endpoint(format!("{}/emails", self.resend.base)),
                    )
                    .build(),
            )
        }

        async fn paste(&self, body: &str) -> Answer {
            self.send(&json!({ "token": "tok", "subject": "hello", "body": body }).to_string())
                .await
        }

        async fn send(&self, raw: &str) -> Answer {
            send(self.app(), post(PATH, raw)).await
        }

        fn relayed_to(&self) -> Vec<String> {
            self.resend
                .sent()
                .iter()
                .map(|sent| sent.body["to"].as_str().unwrap_or_default().to_owned())
                .collect()
        }

        async fn holds_pass(&self) -> bool {
            has_live_pass(&self.db, HANDLE, SENDER, NOW).await.unwrap()
        }
    }

    /// The budget is reported as a state, where "no state" means there was
    /// room. Read the other way round it inverts: the first paste is refused
    /// for being over budget.
    #[tokio::test]
    async fn a_first_paste_is_not_refused_for_being_over_budget() {
        let fixture = fixture("commercial", 200).await;
        fixture.cleared().await;

        let answer = fixture.paste("here is what I wrote").await;

        assert_eq!(
            answer.status,
            StatusCode::OK,
            "a sender with room left must not be told they have used their hour: {}",
            answer.text
        );
        assert_eq!(answer.text, r#"{"status":"delivered","to":"demo"}"#);
    }

    /// Addressed to where the inbox forwards, not to the handle nobody outside
    /// this system can deliver to.
    #[tokio::test]
    async fn a_cleared_senders_paste_reaches_the_address_the_inbox_forwards_to() {
        let fixture = fixture("commercial", 200).await;
        fixture.cleared().await;

        fixture.paste("here is what I wrote").await;

        assert_eq!(fixture.relayed_to(), ["demo@example.com"]);
    }

    /// One payment buys one delivery, so the pass has to be gone afterwards.
    #[tokio::test]
    async fn a_delivered_paste_spends_the_pass_that_authorised_it() {
        let fixture = fixture("commercial", 200).await;
        fixture.cleared().await;

        fixture.paste("here is what I wrote").await;

        assert!(!fixture.holds_pass().await);
    }

    /// A use taken for a delivery that never happened has to come back, or an
    /// outage at the relay costs the sender the message they paid for.
    #[tokio::test]
    async fn a_paste_the_relay_will_not_take_gives_the_delivery_back() {
        let fixture = fixture("commercial", 502).await;
        fixture.cleared().await;

        let answer = fixture.paste("here is what I wrote").await;

        assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            answer.body,
            json!({ "error": "Could not deliver it: mailbox unavailable" })
        );
        assert!(
            fixture.holds_pass().await,
            "nothing was delivered, so the sender must still be holding what they paid for"
        );
    }

    #[tokio::test]
    async fn an_unsettled_challenge_cannot_be_pasted_through() {
        let fixture = fixture("commercial", 200).await;

        let answer = fixture.paste("let me in").await;

        assert_eq!(answer.status, StatusCode::FORBIDDEN);
        assert_eq!(answer.body, json!({ "error": "Clear the gate first" }));
    }

    #[tokio::test]
    async fn a_paste_the_model_reads_as_deception_is_refused_and_costs_nothing() {
        let fixture = fixture("dangerous", 200).await;
        fixture.cleared().await;

        let answer = fixture.paste("verify your account here").await;

        assert_eq!(answer.status, StatusCode::FORBIDDEN);
        assert!(fixture.relayed_to().is_empty());
        assert!(fixture.holds_pass().await);
    }

    /// Unable to judge it, so not sent under our name; and since the reason is
    /// ours, the budget slot goes back.
    #[tokio::test]
    async fn a_paste_that_cannot_be_judged_is_not_sent_and_its_slot_goes_back() {
        let mut fixture = fixture("commercial", 200).await;
        fixture.classifier_base = closed_port();
        fixture.cleared().await;

        let answer = fixture.paste("here is what I wrote").await;

        assert_eq!(answer.status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(fixture.relayed_to().is_empty());
        assert!(fixture.holds_pass().await);
        let budget = format!("paste:{HANDLE}");
        assert_eq!(
            claim_classification(&fixture.db, &budget, SENDER, NOW)
                .await
                .unwrap(),
            None,
            "the slot the failed check took must have been given back"
        );
    }

    #[tokio::test]
    async fn dangerous_mail_cannot_be_pasted_through_whoever_cleared_it() {
        let fixture = fixture("commercial", 200).await;
        seed(&fixture.db, &challenge("danger", "dangerous")).await;
        claim_challenge(&fixture.db, "danger", "paid", NOW)
            .await
            .unwrap();

        let answer = fixture
            .send(&json!({ "token": "danger", "body": "hi" }).to_string())
            .await;

        assert_eq!(answer.status, StatusCode::FORBIDDEN);
        assert_eq!(fixture.model.sent().len(), 0);
    }

    #[tokio::test]
    async fn a_sender_without_a_live_pass_is_refused_before_the_model_is_asked() {
        let fixture = fixture("commercial", 200).await;
        claim_challenge(&fixture.db, "tok", "paid", NOW)
            .await
            .unwrap();

        let answer = fixture.paste("here is what I wrote").await;

        assert_eq!(answer.status, StatusCode::FORBIDDEN);
        assert_eq!(answer.body, json!({ "error": PASS_RUN_OUT }));
        assert_eq!(fixture.model.sent().len(), 0);
    }

    #[tokio::test]
    async fn a_request_missing_its_token_or_message_is_refused() {
        let fixture = fixture("commercial", 200).await;

        for raw in [
            r#"{"body":"hi"}"#,
            r#"{"token":"tok"}"#,
            r#"{"token":"tok","body":"   "}"#,
            r#"{"token":"","body":"hi"}"#,
        ] {
            let answer = fixture.send(raw).await;
            assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{raw}");
            assert_eq!(
                answer.body,
                json!({ "error": "token and a message are required" })
            );
        }
    }

    #[tokio::test]
    async fn a_message_too_long_to_paste_is_refused() {
        let fixture = fixture("commercial", 200).await;

        let long_body = fixture.paste(&"x".repeat(MAX_BODY + 1)).await;
        let long_subject = fixture
            .send(&json!({ "token": "tok", "subject": "s".repeat(201), "body": "hi" }).to_string())
            .await;

        assert_eq!(long_body.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(long_subject.status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn an_unknown_token_is_refused() {
        let fixture = fixture("commercial", 200).await;

        let answer = fixture
            .send(&json!({ "token": "bogus", "body": "hi" }).to_string())
            .await;

        assert_eq!(answer.status, StatusCode::NOT_FOUND);
        assert_eq!(answer.body, json!({ "error": "Unknown challenge" }));
    }

    /// `relayHeldMessage` read `RESEND_API_KEY` inside its fetch, so a missing
    /// key arrived as the relay failing, with the variable's name as the
    /// error, and the pass came back.
    #[tokio::test]
    async fn a_relay_nobody_configured_fails_the_send_and_gives_the_pass_back() {
        let fixture = fixture("commercial", 200).await;
        fixture.cleared().await;
        let app = router(
            AppState::builder(Env::empty())
                .clock(clock_at(NOW))
                .db(fixture.db.clone())
                .classifier(
                    Classifier::new(Ok("test".to_owned()), &fixture.classifier_base)
                        .with_retries(0, Duration::ZERO),
                )
                .build(),
        );
        let raw = json!({ "token": "tok", "body": "hi" }).to_string();

        let answer = send(app, post(PATH, &raw)).await;

        assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
        assert_eq!(answer.body, json!({ "error": "RESEND_API_KEY is not set" }));
        assert!(fixture.holds_pass().await);
    }

    #[test]
    fn a_subject_is_measured_the_way_javascript_measures_it() {
        assert_eq!(subject_length(Some(&json!("é😀"))), 3.0);
        assert_eq!(subject_length(Some(&json!([1, 2]))), 2.0);
        assert_eq!(subject_length(Some(&json!({ "length": 300 }))), 300.0);
        assert_eq!(subject_length(Some(&json!(5))), 0.0);
        assert_eq!(subject_length(None), 0.0);
    }

    #[tokio::test]
    async fn a_message_that_is_not_a_string_is_the_bare_500_the_typescript_gave() {
        let fixture = fixture("commercial", 200).await;

        let answer = fixture
            .send(&json!({ "token": "tok", "body": 5 }).to_string())
            .await;

        assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(answer.text, "");
    }
}

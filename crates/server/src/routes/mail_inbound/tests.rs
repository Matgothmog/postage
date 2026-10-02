//! The route driven through the router, the way the mail worker drives it:
//! `route.test.ts`, the route-level cases of `faults.test.ts`,
//! `database-fault.test.ts`, the wire contract the worker parses, and the
//! malformed bodies refused before anything is spent.

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use postage_shared::{GatewayAction, GatewayVerdict};
use reqwest::Url;
use serde_json::{Value, json};

use super::tests_support::{
    CHAIN_REFUSAL, WALLET, chain_node, classifier, model, test_env, test_env_without,
};
use crate::app::AppState;
use crate::chain::Chain;
use crate::config::Env;
use crate::db::Db;
use crate::db::challenges::HOLD_SECONDS;
use crate::db::classifications::{
    CLASSIFY_PER_DOMAIN_HOURLY, CLASSIFY_PER_HANDLE_HOURLY, CLASSIFY_PER_SENDER_HOURLY,
    claim_classification,
};
use crate::db::inboxes::create_inbox;
use crate::db::passes::grant_pass;
use crate::db::testing::TestDb;
use crate::http_stub::{Reply, Stub, serve_with};
use crate::log::captured;
use crate::routes::router;
use crate::routes::testing::{Answer, NOW, clock_at, keys_in_order, post_with, send};

const PATH: &str = "/api/mail/inbound";
const DESTINATION: &str = "demo@example.com";
const FAULT_LINE: &str = "inbound mail gateway fault";

/// The message most cases send: authenticated, to the one inbox.
fn message() -> Value {
    json!({
        "from": "someone@nowhere.example",
        "to": "demo@usepostage.com",
        "subject": "hello",
        "body": "x",
        "spf": "pass",
        "dkim": "pass",
        "dmarc": "pass",
    })
}

fn authentication(authenticated: bool) -> (&'static str, &'static str, &'static str) {
    if authenticated {
        ("pass", "pass", "pass")
    } else {
        ("none", "none", "none")
    }
}

/// The gateway with an inbox `demo` forwarding to [`DESTINATION`], a node
/// answering the floor (or refusing), and a model answering `tier` (or
/// refusing, so every verdict is the header fallback).
struct Gateway {
    db: Arc<TestDb>,
    env: Env,
    chain: Stub,
    model: Stub,
}

impl Gateway {
    async fn new(tier: Option<&'static str>) -> Self {
        Self::with(tier, false, test_env()).await
    }

    async fn with(tier: Option<&'static str>, chain_failing: bool, env: Env) -> Self {
        let db = Arc::new(TestDb::fresh().await);
        create_inbox(&db, "demo", DESTINATION, Some(WALLET), NOW)
            .await
            .unwrap();
        Self {
            db,
            env,
            chain: chain_node(chain_failing).await,
            model: model(tier).await,
        }
    }

    fn app(&self) -> Router {
        router(
            AppState::builder(self.env.clone())
                .clock(clock_at(NOW))
                .db(self.db.clone())
                .chain(Chain::new(Url::parse(&self.chain.base).unwrap()))
                .classifier(classifier(&self.model))
                .build(),
        )
    }

    async fn send_raw(&self, raw: &str, secret: Option<&str>) -> Answer {
        let headers: Vec<(&str, &str)> = secret
            .map(|secret| vec![("x-postage-secret", secret)])
            .unwrap_or_default();
        send(self.app(), post_with(PATH, raw, &headers)).await
    }

    async fn post(&self, message: &Value) -> Answer {
        self.send_raw(&message.to_string(), Some("secret")).await
    }

    async fn deliver(&self, from: &str, subject: &str, authenticated: bool) -> Answer {
        let (spf, dkim, dmarc) = authentication(authenticated);
        self.deliver_with_auth(from, subject, spf, dkim, dmarc)
            .await
    }

    async fn deliver_with_auth(
        &self,
        from: &str,
        subject: &str,
        spf: &str,
        dkim: &str,
        dmarc: &str,
    ) -> Answer {
        self.post(&json!({
            "from": from,
            "to": "demo@usepostage.com",
            "subject": subject,
            "body": "x",
            "spf": spf,
            "dkim": dkim,
            "dmarc": dmarc,
        }))
        .await
    }

    fn model_calls(&self) -> usize {
        self.model.sent().len()
    }

    fn chain_calls(&self) -> usize {
        self.chain.sent().len()
    }
}

async fn count(db: &Db, table: &str) -> i64 {
    #[derive(serde::Deserialize)]
    struct Count {
        n: i64,
    }
    let rows: Vec<Count> = db
        .all(&format!("SELECT COUNT(*) AS n FROM {table}"), ())
        .await
        .unwrap();
    rows[0].n
}

/// The raw text of the last field of a serialized object.
fn last_field<'a>(text: &'a str, name: &str) -> &'a str {
    let start = text.rfind(&format!("\"{name}\":")).unwrap() + name.len() + 3;
    &text[start..text.len() - 1]
}

fn wire(answer: &Answer) -> GatewayVerdict {
    serde_json::from_str(&answer.text).unwrap()
}

// --- route.test.ts --------------------------------------------------------

/// The premise the tests below are about, asserted rather than assumed: a
/// stubbed model and a live one are indistinguishable in their results.
#[tokio::test]
async fn the_model_cannot_be_reached_from_here_so_a_verdict_is_the_header_fallback() {
    let gateway = Gateway::new(None).await;

    let answer = gateway
        .deliver("someone@nowhere.example", "hello", true)
        .await;

    assert!(
        gateway.model_calls() > 0,
        "an authenticated first message must reach the classifier"
    );
    assert_eq!(answer.body["verdict"]["degraded"], json!(true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unauthenticated_mail_cannot_drain_the_inboxs_reading_budget() {
    let gateway = Gateway::new(None).await;
    for n in 0..CLASSIFY_PER_HANDLE_HOURLY + 10 {
        gateway
            .deliver(&format!("forged{n}@x.com"), "hello", false)
            .await;
    }

    let answer = gateway
        .deliver("noreply@stripe.com", "Your verification code is 4821", true)
        .await;

    assert_eq!(
        answer.body["action"], "forward",
        "a stranger's flood must not stop a real login code arriving"
    );
}

/// A domain's allowance is only spendable by mail from that domain that the
/// receiving server could actually confirm.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unauthenticated_flood_cannot_spend_the_allowance_of_the_domain_it_names() {
    let gateway = Gateway::new(None).await;
    for n in 0..CLASSIFY_PER_DOMAIN_HOURLY + 5 {
        gateway
            .deliver(&format!("forged{n}@stripe.com"), "hello", false)
            .await;
    }

    let answer = gateway
        .deliver("noreply@stripe.com", "Your verification code is 4821", true)
        .await;

    assert_eq!(
        answer.body["action"], "forward",
        "forged mail claiming a domain must not consume what that domain is allowed"
    );
}

/// Empty the inbox's pool, which takes nothing but authenticated mail spread
/// over enough domains, and every message after it is read from headers
/// alone, where a transactional subject is called important.
#[tokio::test]
async fn a_drained_inbox_pool_does_not_buy_the_next_sender_free_delivery() {
    let gateway = Gateway::new(None).await;
    for taken in 0..CLASSIFY_PER_HANDLE_HOURLY {
        let domain = taken / CLASSIFY_PER_DOMAIN_HOURLY;
        claim_classification(
            &gateway.db,
            "demo",
            &format!("writer{taken}@drain{domain}.example"),
            NOW,
        )
        .await
        .unwrap();
    }

    let answer = gateway
        .deliver("stranger@nowhere.example", "your one-time code", true)
        .await;

    assert_eq!(
        answer.body["action"], "hold",
        "emptying the pool must not be a way through the gate for whoever comes next"
    );
}

#[tokio::test]
async fn unauthenticated_mail_cannot_buy_free_delivery_with_a_transactional_subject() {
    let gateway = Gateway::new(None).await;

    let answer = gateway
        .deliver("spam@nowhere.example", "your one-time code", false)
        .await;

    assert_eq!(answer.body["action"], "hold");
}

/// A sender's own hourly slice is theirs to spend, so spending it must not
/// unlock the free tier.
#[tokio::test]
async fn a_sender_who_spends_their_own_slice_cannot_then_be_delivered_free() {
    let gateway = Gateway::new(None).await;
    for _ in 0..CLASSIFY_PER_SENDER_HOURLY {
        gateway
            .deliver("greedy@nowhere.example", "hello", true)
            .await;
    }

    let answer = gateway
        .deliver("greedy@nowhere.example", "your one-time code", true)
        .await;

    assert_eq!(
        answer.body["action"], "hold",
        "draining your own budget must not be a way to reach the tier nobody pays for"
    );
}

/// The worker collapses a disagreeing Authentication-Results header to null,
/// so spf pass with a merely-not-failing dkim must not count, and nor may a
/// sender who simply never signs.
#[tokio::test]
async fn spf_pass_alone_without_a_verified_dkim_signature_does_not_authenticate_the_sender() {
    let gateway = Gateway::new(None).await;

    let answer = gateway
        .deliver_with_auth("someone@nowhere.example", "hello", "pass", "none", "none")
        .await;

    assert_eq!(
        gateway.model_calls(),
        0,
        "an unverified dkim must not unlock the classifier budget an authenticated sender gets"
    );
    assert_eq!(answer.body["action"], "hold");
    assert_eq!(
        answer.body["notice"],
        Value::Null,
        "a sender we can't confirm must not be mailed back"
    );
}

// --- faults.test.ts, the cases that drive the route -----------------------

/// The defect the live outage exposed: the worker read the status, found
/// nothing after it, and logged `Gateway returned 500: ` with no reason.
#[tokio::test]
async fn a_chain_that_will_not_answer_says_so_in_the_body_rather_than_sending_nothing() {
    let gateway = Gateway::with(None, true, test_env()).await;

    let answer = gateway.post(&message()).await;

    assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!answer.text.is_empty());
    assert_eq!(
        answer.body,
        json!({
            "error": "The gateway could not read the price floor from the chain",
            "fault": "chain",
        })
    );
}

#[tokio::test]
async fn a_chain_fault_is_logged_under_the_stage_that_failed_with_what_the_chain_said() {
    let gateway = Gateway::with(None, true, test_env()).await;

    let (_, lines) = captured::during(gateway.post(&message())).await;

    let line = lines
        .iter()
        .find(|line| line.starts_with(FAULT_LINE))
        .expect("a chain fault must still reach the log");
    assert!(line.contains("stage=chain"), "{line}");
    assert!(line.contains(CHAIN_REFUSAL), "{line}");
}

/// The one variable read before the caller has proved anything, and so the
/// one that may not be named in the answer.
#[tokio::test]
async fn a_webhook_secret_nobody_set_does_not_name_itself_to_an_anonymous_caller() {
    let gateway = Gateway::with(None, false, test_env_without(&["MAIL_WEBHOOK_SECRET"])).await;

    let answer = gateway.post(&message()).await;

    assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        !answer.text.contains("MAIL_WEBHOOK_SECRET"),
        "an unauthenticated caller must not be told which variable is missing"
    );
    assert_eq!(
        answer.body,
        json!({ "error": "The gateway is missing a required setting", "fault": "config" })
    );
}

/// Withholding it from the caller cannot mean losing it: the log is where an
/// operator reads which variable to set.
#[tokio::test]
async fn the_webhook_secrets_name_still_reaches_the_log_where_only_an_operator_sees_it() {
    let gateway = Gateway::with(None, false, test_env_without(&["MAIL_WEBHOOK_SECRET"])).await;

    let (_, lines) = captured::during(gateway.post(&message())).await;

    assert_eq!(
        lines,
        [format!(
            "{FAULT_LINE} stage=config reason=MAIL_WEBHOOK_SECRET is not set"
        )]
    );
}

#[tokio::test]
async fn an_app_url_nobody_set_names_the_variable_too() {
    let gateway = Gateway::with(None, false, test_env_without(&["APP_URL"])).await;

    let answer = gateway.post(&message()).await;

    assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        answer.body,
        json!({ "error": "APP_URL is not set on the gateway", "fault": "config" })
    );
}

/// The worker keys on the status, so both refusals are pinned byte for byte.
#[tokio::test]
async fn a_wrong_secret_is_refused_exactly_as_it_always_was() {
    let gateway = Gateway::new(None).await;

    let wrong = gateway
        .send_raw(&message().to_string(), Some("wrong"))
        .await;
    let missing = gateway.send_raw(&message().to_string(), None).await;

    for answer in [wrong, missing] {
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
        assert_eq!(answer.text, r#"{"error":"Bad secret"}"#);
    }
}

/// 404 is the only status the worker reads as a real answer, so a fault must
/// never borrow it and a real unknown inbox must never lose it.
#[tokio::test]
async fn an_unknown_handle_is_refused_exactly_as_it_always_was() {
    let gateway = Gateway::new(None).await;
    let mut to_nobody = message();
    to_nobody["to"] = json!("nobody@usepostage.com");

    let answer = gateway.post(&to_nobody).await;

    assert_eq!(answer.status, StatusCode::NOT_FOUND);
    assert_eq!(
        answer.text,
        r#"{"action":"reject","reason":"unknown_inbox"}"#
    );
}

#[tokio::test]
async fn a_message_the_gateway_can_answer_is_untouched_by_any_of_this() {
    let gateway = Gateway::new(None).await;

    let answer = gateway.post(&message()).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.body["action"], "hold");
}

// --- database-fault.test.ts ----------------------------------------------

/// Long enough to be mistaken for a real one, so asserting it never appears
/// asserts something the redaction rules actually have to catch.
const BROKEN_DB_TOKEN: &str = "not-a-real-turso-token-but-long-enough-to-look-like-one";

/// A database that accepts the connection and refuses the query: the shape
/// the live outage had.
async fn broken_database() -> (Stub, Router) {
    let database =
        serve_with(|_| Reply::new(500, "SQLITE_UNKNOWN: the server is not accepting queries"))
            .await;
    let env = Env::fixed([
        ("MAIL_WEBHOOK_SECRET", "secret"),
        ("APP_URL", "http://localhost"),
        ("DATABASE_URL", database.base.as_str()),
        ("DATABASE_AUTH_TOKEN", BROKEN_DB_TOKEN),
    ]);
    let app = router(AppState::builder(env).clock(clock_at(NOW)).build());
    (database, app)
}

async fn post_to(app: Router) -> Answer {
    let raw = message().to_string();
    send(
        app,
        post_with(PATH, &raw, &[("x-postage-secret", "secret")]),
    )
    .await
}

#[tokio::test]
async fn a_database_that_will_not_answer_is_a_500_that_says_which_part_fell_over() {
    let (_database, app) = broken_database().await;

    let answer = post_to(app).await;

    assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        answer.body,
        json!({ "error": "The gateway could not reach its database", "fault": "database" })
    );
}

#[tokio::test]
async fn the_answer_to_a_database_fault_quotes_neither_the_connection_nor_its_credential() {
    let (database, app) = broken_database().await;

    let answer = post_to(app).await;

    assert!(!answer.text.contains(&database.base));
    assert!(!answer.text.contains(BROKEN_DB_TOKEN));
}

#[tokio::test]
async fn a_database_fault_is_logged_under_its_stage_with_what_the_database_actually_said() {
    let (_database, app) = broken_database().await;

    let (_, lines) = captured::during(post_to(app)).await;

    let line = lines
        .iter()
        .find(|line| line.starts_with(FAULT_LINE))
        .expect("a database fault must reach the log");
    assert!(line.contains("stage=database"), "{line}");
    assert!(line.contains("not accepting queries"), "{line}");
}

#[tokio::test]
async fn nothing_logged_about_a_database_fault_carries_the_credential_or_the_url() {
    let (database, app) = broken_database().await;

    let (_, lines) = captured::during(post_to(app)).await;

    assert!(!lines.is_empty());
    for line in &lines {
        assert!(!line.contains(BROKEN_DB_TOKEN), "{line}");
        assert!(!line.contains(&database.base), "{line}");
    }
}

// --- the wire contract the mail worker parses -----------------------------

#[tokio::test]
async fn a_forward_reads_as_the_workers_verdict() {
    let gateway = Gateway::new(Some("important")).await;

    let answer = gateway.post(&message()).await;

    assert_eq!(answer.status, StatusCode::OK);
    let verdict = wire(&answer);
    assert_eq!(verdict.action, GatewayAction::Forward);
    assert_eq!(verdict.to.as_deref(), Some(DESTINATION));
    assert_eq!(verdict.reason.as_deref(), Some("important"));
    assert_eq!(
        answer.text,
        r#"{"action":"forward","to":"demo@example.com","reason":"important","verdict":{"tier":"important","confidence":0.9,"reasons":["a stub answers for the model here"],"degraded":false}}"#
    );
}

#[tokio::test]
async fn a_pass_forward_names_the_pass_that_carried_it() {
    let gateway = Gateway::new(Some("commercial")).await;
    grant_pass(
        &gateway.db,
        "demo",
        "someone@nowhere.example",
        "human",
        None,
        NOW,
    )
    .await
    .unwrap();

    let verdict = wire(&gateway.post(&message()).await);

    assert_eq!(verdict.action, GatewayAction::Forward);
    assert_eq!(verdict.reason.as_deref(), Some("human"));
}

#[tokio::test]
async fn a_hold_reads_as_the_workers_verdict_with_the_notice_to_send() {
    let gateway = Gateway::new(Some("commercial")).await;

    let answer = gateway.post(&message()).await;

    assert_eq!(answer.status, StatusCode::OK);
    let verdict = wire(&answer);
    assert_eq!(verdict.action, GatewayAction::Hold);
    assert_eq!(verdict.reason.as_deref(), Some("commercial"));
    let token = verdict.token.clone().unwrap();
    assert_eq!(verdict.held_until, Some((NOW + HOLD_SECONDS) as u64));
    let notice = verdict.notice.unwrap();
    assert_eq!(notice.subject, "Held: your mail to demo@usepostage.com");
    assert!(notice.text.contains(&format!("http://localhost/c/{token}")));
    assert_eq!(
        verdict.bounce.unwrap(),
        format!(
            "Held, not lost: say whether a person or a machine wrote this and we deliver the message you already sent - http://localhost/c/{token}"
        )
    );
}

/// `{ ...wire, ...context }`: the wire fields first, in the order the
/// TypeScript wrote them, then the context; `notice` written as `null`
/// rather than left out when the sender is not written back.
#[tokio::test]
async fn an_unauthenticated_hold_writes_a_null_notice_and_keeps_the_typescript_field_order() {
    let gateway = Gateway::new(None).await;

    let answer = gateway
        .deliver("spam@nowhere.example", "hello", false)
        .await;

    assert_eq!(
        keys_in_order(&answer.text),
        [
            "action",
            "reason",
            "token",
            "held_until",
            "notice",
            "bounce",
            "verdict",
            "price",
            "reasons",
            "challenge_url",
            "quote"
        ]
    );
    assert!(answer.text.contains(r#""notice":null"#));
    let verdict = wire(&answer);
    assert_eq!(verdict.action, GatewayAction::Hold);
    assert_eq!(verdict.notice, None);
    assert_eq!(
        answer.body["price"],
        json!(super::tests_support::FLOOR.to_string())
    );
    assert_eq!(
        keys_in_order(last_field(&answer.text, "quote")),
        [
            "messageId",
            "inbox",
            "tier",
            "amount",
            "expiresAt",
            "signature"
        ]
    );
}

#[tokio::test]
async fn dangerous_mail_reads_as_a_rejection_that_carries_the_link() {
    let gateway = Gateway::new(Some("dangerous")).await;

    let answer = gateway.post(&message()).await;

    assert_eq!(answer.status, StatusCode::OK);
    let verdict = wire(&answer);
    assert_eq!(verdict.action, GatewayAction::Reject);
    assert_eq!(verdict.reason.as_deref(), Some("dangerous"));
    assert_eq!(verdict.token, None);
    assert_eq!(verdict.held_until, None);
    let url = answer.body["challenge_url"].as_str().unwrap();
    assert_eq!(
        verdict.bounce.unwrap(),
        format!(
            "Not delivered: this looks like an attempt to deceive the recipient, and paying will not change that. If it is a mistake, say so at {url}"
        )
    );
    assert_eq!(
        keys_in_order(&answer.text),
        [
            "action",
            "reason",
            "bounce",
            "verdict",
            "price",
            "reasons",
            "challenge_url",
            "quote"
        ]
    );
}

#[tokio::test]
async fn an_inbox_nobody_finished_claiming_is_refused_with_a_reason_the_sender_reads() {
    let gateway = Gateway::new(None).await;
    create_inbox(&gateway.db, "demo", DESTINATION, None, NOW)
        .await
        .unwrap();

    let answer = gateway.post(&message()).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(
        answer.text,
        r#"{"action":"reject","reason":"no_wallet","bounce":"That address cannot receive mail yet: nobody has claimed it fully."}"#
    );
    assert_eq!(wire(&answer).action, GatewayAction::Reject);
    assert_eq!(gateway.model_calls(), 0);
}

#[tokio::test]
async fn an_inbox_forwarding_back_to_this_domain_is_refused_rather_than_looped() {
    let gateway = Gateway::new(None).await;
    create_inbox(
        &gateway.db,
        "demo",
        "other@usepostage.com",
        Some(WALLET),
        NOW,
    )
    .await
    .unwrap();

    let answer = gateway.post(&message()).await;

    assert_eq!(answer.status, StatusCode::OK);
    let verdict = wire(&answer);
    assert_eq!(verdict.action, GatewayAction::Reject);
    assert_eq!(verdict.reason.as_deref(), Some("loop"));
    assert_eq!(gateway.model_calls(), 0);
    assert_eq!(gateway.chain_calls(), 0);
}

#[tokio::test]
async fn an_unknown_inbox_reads_as_the_workers_verdict() {
    let gateway = Gateway::new(None).await;
    let mut to_nobody = message();
    to_nobody["to"] = json!("nobody@usepostage.com");

    let verdict = wire(&gateway.post(&to_nobody).await);

    assert_eq!(verdict, GatewayVerdict::reject("unknown_inbox", None));
}

// --- malformed bodies -------------------------------------------------------

/// Each of these threw in the TypeScript and was answered as a 500
/// "unexpected" fault, some only after the classification budget had been
/// claimed. A deliberate change: all are refused with a 400 before any read,
/// budget slot, model call, chain read or challenge row.
#[tokio::test]
async fn a_malformed_body_is_refused_before_anything_is_read_or_spent() {
    let gateway = Gateway::new(None).await;
    let required = "from and to are required";

    for (raw, error) in [
        ("not json", "Invalid request"),
        ("", "Invalid request"),
        ("null", required),
        ("[]", required),
        ("5", required),
        (r#"{"to":"demo@usepostage.com"}"#, required),
        (r#"{"from":"a@b.c","to":""}"#, required),
        (r#"{"from":0,"to":"demo@usepostage.com"}"#, required),
        (
            r#"{"from":5,"to":"demo@usepostage.com"}"#,
            "Invalid request",
        ),
        (
            r#"{"from":"a@b.c","to":["demo@usepostage.com"]}"#,
            "Invalid request",
        ),
        (
            r#"{"from":{"a":1},"to":"demo@usepostage.com"}"#,
            "Invalid request",
        ),
        (
            r#"{"from":"a@b.c","to":"demo@usepostage.com","subject":5}"#,
            "Invalid request",
        ),
        (
            r#"{"from":"a@b.c","to":"demo@usepostage.com","body":["x"]}"#,
            "Invalid request",
        ),
        (
            r#"{"from":"a@b.c","to":"demo@usepostage.com","dmarc":true}"#,
            "Invalid request",
        ),
        (
            r#"{"from":"a@b.c","to":"demo@usepostage.com","spf":{"r":"pass"}}"#,
            "Invalid request",
        ),
        (
            r#"{"from":"a@b.c","to":"demo@usepostage.com","dkim":1}"#,
            "Invalid request",
        ),
    ] {
        let answer = gateway.send_raw(raw, Some("secret")).await;

        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{raw}");
        assert_eq!(answer.body, json!({ "error": error }), "{raw}");
    }
    assert_eq!(count(&gateway.db, "classifications").await, 0);
    assert_eq!(count(&gateway.db, "challenges").await, 0);
    assert_eq!(gateway.model_calls(), 0);
    assert_eq!(gateway.chain_calls(), 0);
}

/// Null optional fields read as absent, as `?? ""` and `?? null` read them.
#[tokio::test]
async fn null_optional_fields_read_as_absent() {
    let gateway = Gateway::new(None).await;
    let raw = r#"{"from":"a@b.c","to":"demo@usepostage.com","subject":null,"body":null,"spf":null,"dkim":null,"dmarc":null}"#;

    let answer = gateway.send_raw(raw, Some("secret")).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.body["action"], "hold");
}

/// The secret is checked before the body is read, so a stranger learns
/// nothing about what a valid body looks like.
#[tokio::test]
async fn a_malformed_body_without_the_secret_is_refused_for_the_secret() {
    let gateway = Gateway::new(None).await;

    let answer = gateway.send_raw("not json", Some("wrong")).await;

    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
}

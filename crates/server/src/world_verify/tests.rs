//! The stages of `/api/world/verify` up to World's verdict, ported from
//! `web/src/app/api/world/verify/route.test.ts`, driven through [`handle`]
//! with a settle step that records who it was handed. World is a loopback
//! stub; nothing here reaches the network.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::http::StatusCode;
use axum::response::Response;
use futures_util::future::BoxFuture;
use postage_core::signal::hash_signal;
use serde_json::{Value, json};

use super::*;
use crate::config::Env;
use crate::db::challenges::{NewChallenge, claim_challenge};
use crate::db::issued_contexts::{
    MAX_LIVE_CONTEXTS_PER_TOKEN, consume_issued_context, record_issued_context,
};
use crate::db::testing::TestDb;
use crate::http_stub::{Reply, Stub, closed_port, serve, serve_with};
use crate::log::captured;
use crate::routes::json;
use crate::routes::testing::{NOW, challenge, clock_at, seed};
use crate::world::WorldVerify;

const TOKEN: &str = "tok";
const OTHER_TOKEN: &str = "tok-two";
const SENDER: &str = "sender@x.com";
const OTHER_SENDER: &str = "attacker@x.com";
const RP_ID: &str = "app_test_rp_id";
const VERIFY_PATH: &str = "/verify/app_test_rp_id";

const WORLD_ACCEPTS: &str = r#"{"success":true,"results":[{"identifier":"selfie","success":true,"nullifier":"world-nullifier"}]}"#;

/// What the settle step answers: success, or the failed attestation the
/// mock-mode ceiling tests need the chain to keep producing.
#[derive(Clone, Copy)]
enum Settles {
    Cleared,
    AttestationFails,
}

/// Stands in for everything after World: records what it was handed.
struct Recording {
    seen: Arc<Mutex<Vec<VerifiedHuman>>>,
    settles: Settles,
}

impl SettleVerifiedHuman for Recording {
    fn settle<'a>(
        &'a self,
        _state: &'a AppState,
        verified: VerifiedHuman,
    ) -> BoxFuture<'a, Result<Response, Stopped>> {
        self.seen.lock().unwrap().push(verified);
        let settles = self.settles;
        Box::pin(async move {
            match settles {
                Settles::Cleared => Ok(json(StatusCode::OK, &json!({ "status": "cleared" }))),
                Settles::AttestationFails => Err(Stopped::refused(
                    StatusCode::BAD_GATEWAY,
                    "Could not record the attestation. Nothing was saved. Try again, or pay instead",
                )),
            }
        })
    }
}

#[derive(Debug)]
struct Answer {
    status: StatusCode,
    text: String,
    body: Value,
    /// What a bare 500 carries to the router's error log, if anything.
    unhandled: Option<String>,
}

struct Fixture {
    db: Arc<TestDb>,
    world: Stub,
    seen: Arc<Mutex<Vec<VerifiedHuman>>>,
}

impl Fixture {
    async fn new(world_status: u16, world_body: &str) -> Self {
        let world = serve(&[(VERIFY_PATH, world_status, world_body)]).await;
        Self::with_world(world).await
    }

    async fn accepting() -> Self {
        Self::new(200, WORLD_ACCEPTS).await
    }

    async fn with_world(world: Stub) -> Self {
        let db = Arc::new(TestDb::fresh().await);
        seed(&db, &challenge(TOKEN, "commercial")).await;
        Self {
            db,
            world,
            seen: Arc::default(),
        }
    }

    fn state(&self, env: Env, world: WorldVerify) -> AppState {
        AppState::builder(env)
            .clock(clock_at(NOW))
            .db(self.db.clone())
            .world(world)
            .build()
    }

    async fn verify(&self, mode: &str, body: &Value) -> Answer {
        self.send(env(mode), &body.to_string(), Settles::Cleared)
            .await
    }

    async fn live(&self, body: &Value) -> Answer {
        self.verify("live", body).await
    }

    async fn send(&self, env: Env, body: &str, settles: Settles) -> Answer {
        let world = WorldVerify::default().with_base(&self.world.base);
        self.send_to(env, world, body, settles).await
    }

    async fn send_to(&self, env: Env, world: WorldVerify, body: &str, settles: Settles) -> Answer {
        let state = self.state(env, world);
        let settle = Recording {
            seen: self.seen.clone(),
            settles,
        };
        let response = handle(&state, &Bytes::from(body.to_owned()), &settle).await;
        answer(response).await
    }

    fn world_calls(&self) -> usize {
        self.world.sent().len()
    }

    fn settled(&self) -> Vec<VerifiedHuman> {
        self.seen.lock().unwrap().clone()
    }

    /// Simulates the `/api/world/context` call every live proof follows.
    async fn issue_context(&self, token: &str, nonce: &str) {
        let issued = record_issued_context(&self.db, token, nonce, NOW, NOW + 300, NOW)
            .await
            .unwrap();
        assert!(issued, "test setup: the context has to be recorded");
    }

    async fn other_sender_challenge(&self) {
        seed(
            &self.db,
            &NewChallenge {
                sender: OTHER_SENDER.to_owned(),
                ..challenge(OTHER_TOKEN, "commercial")
            },
        )
        .await;
    }
}

fn env(mode: &str) -> Env {
    Env::fixed([("IDENTITY_MODE", mode), ("WORLD_RP_ID", RP_ID)])
}

async fn answer(response: Response) -> Answer {
    let status = response.status();
    let unhandled = response
        .extensions()
        .get::<Unhandled>()
        .map(|unhandled| reason_chain(unhandled.error()));
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    Answer {
        status,
        text,
        body,
        unhandled,
    }
}

/// The shape the challenge page sends: a `responses` array naming the
/// Selfie Check credential, bound to `token` by its `signal_hash`.
fn proof_for(token: &str) -> Value {
    json!({
        "protocol_version": "3.0",
        "nonce": format!("nonce-{token}"),
        "responses": [{
            "identifier": "selfie",
            "signal_hash": hash_signal(token),
            "proof": "0xproof",
            "merkle_root": "0xroot",
            "nullifier": "world-nullifier",
        }],
    })
}

fn with_proof(token: &str, proof: &Value) -> Value {
    json!({ "token": token, "proof": proof })
}

fn error_of(answer: &Answer) -> String {
    answer.body["error"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase()
}

/// A live verification with World answering `status` and `body`.
async fn rejected_by_world(status: u16, body: &str) -> (Fixture, Answer) {
    let fixture = Fixture::new(status, body).await;
    let proof = proof_for(TOKEN);
    fixture.issue_context(TOKEN, "nonce-tok").await;
    let answer = fixture.live(&with_proof(TOKEN, &proof)).await;
    (fixture, answer)
}

fn has_token_ref(lines: &[String]) -> bool {
    let fingerprint = format!("tokenRef={}", token_fingerprint(TOKEN));
    lines.iter().any(|line| line.contains(&fingerprint))
}

/// Reads past the field names, so a key called `tokenRef` is not mistaken for
/// the token itself.
fn logs_the_token(lines: &[String]) -> bool {
    lines.iter().any(|line| {
        line.split(' ')
            .filter_map(|pair| pair.split_once('=').map(|(_, value)| value))
            .any(|value| value == TOKEN || value.contains(&format!("\"{TOKEN}\"")))
    })
}

// --- Live mode: the proof reaches World --------------------------------------

#[tokio::test]
async fn live_mode_accepts_a_well_formed_proof_and_forwards_it_to_world() {
    let fixture = Fixture::accepting().await;
    let proof = proof_for(TOKEN);
    fixture.issue_context(TOKEN, "nonce-tok").await;

    let answer = fixture.live(&with_proof(TOKEN, &proof)).await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let sent = fixture.world.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].method, "POST");
    assert_eq!(sent[0].path, VERIFY_PATH);
    assert_eq!(sent[0].content_type.as_deref(), Some("application/json"));
    assert_eq!(sent[0].body, proof, "the IDKit result goes on unchanged");
    let settled = fixture.settled();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].token, TOKEN);
    assert_eq!(settled[0].challenge.sender, SENDER);
    assert_eq!(settled[0].nullifier, "world-nullifier");
    assert!(settled[0].attest_on_chain);
}

#[tokio::test]
async fn live_mode_rejects_a_request_with_no_proof_before_ever_calling_world() {
    let fixture = Fixture::accepting().await;

    let answer = fixture.live(&json!({ "token": TOKEN })).await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        answer.body,
        json!({ "error": "A World ID proof is required" })
    );
    assert_eq!(fixture.world_calls(), 0);
}

#[tokio::test]
async fn live_mode_rejects_a_malformed_proof_before_ever_calling_world() {
    let fixture = Fixture::accepting().await;

    let answer = fixture
        .live(&with_proof(TOKEN, &json!({ "not": "a proof" })))
        .await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_eq!(answer.body, json!({ "error": "Malformed World ID proof" }));
    assert_eq!(fixture.world_calls(), 0);
}

#[tokio::test]
async fn a_network_failure_reaching_world_is_a_gateway_error_and_logs_a_correlator() {
    let fixture = Fixture::accepting().await;
    fixture.issue_context(TOKEN, "nonce-tok").await;
    let world = WorldVerify::default().with_base(closed_port());

    let (answer, lines) = captured::during(fixture.send_to(
        env("live"),
        world,
        &with_proof(TOKEN, &proof_for(TOKEN)).to_string(),
        Settles::Cleared,
    ))
    .await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(answer.body, json!({ "error": "Could not reach World ID" }));
    assert!(has_token_ref(&lines), "{lines:?}");
    assert!(!logs_the_token(&lines), "{lines:?}");
}

/// The TypeScript set no timeout; a World that never answers is unreachable.
#[tokio::test]
async fn a_world_that_does_not_answer_in_time_is_unreachable() {
    let world =
        serve_with(|_| Reply::new(200, WORLD_ACCEPTS).after(Duration::from_millis(500))).await;
    let fixture = Fixture::with_world(world).await;
    fixture.issue_context(TOKEN, "nonce-tok").await;
    let world = WorldVerify::default()
        .with_base(&fixture.world.base)
        .with_timeout(Duration::from_millis(50));

    let answer = fixture
        .send_to(
            env("live"),
            world,
            &with_proof(TOKEN, &proof_for(TOKEN)).to_string(),
            Settles::Cleared,
        )
        .await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(answer.body, json!({ "error": "Could not reach World ID" }));
    assert!(fixture.settled().is_empty());
}

#[tokio::test]
async fn worlds_own_server_error_is_a_gateway_error_and_logs_a_correlator() {
    let ((fixture, answer), lines) = captured::during(rejected_by_world(
        503,
        r#"{"detail":"internal error, try again later"}"#,
    ))
    .await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        answer.body,
        json!({ "error": "World ID is currently unavailable" })
    );
    assert!(!answer.text.contains("internal error"));
    assert!(has_token_ref(&lines), "{lines:?}");
    assert!(fixture.settled().is_empty());
}

#[tokio::test]
async fn a_proof_world_rejects_outright_is_a_bad_request_not_a_gateway_error() {
    let (_, answer) =
        rejected_by_world(400, r#"{"success":false,"detail":"invalid rp signature"}"#).await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert!(
        error_of(&answer).contains("verification failed"),
        "{}",
        answer.text
    );
    assert!(!answer.text.contains("invalid rp signature"));
}

#[tokio::test]
async fn worlds_own_detail_text_is_never_handed_back_to_the_sender() {
    let (_, answer) = rejected_by_world(
        400,
        r#"{"success":false,"detail":"No action found for this app."}"#,
    )
    .await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert!(!answer.text.contains("No action found for this app"));
    assert!(
        error_of(&answer).contains("verification failed"),
        "{}",
        answer.text
    );
}

#[tokio::test]
async fn a_signature_expired_rejection_gets_curated_copy_not_worlds_own_wording() {
    let (_, answer) = rejected_by_world(
        400,
        r#"{"success":false,"code":"rp_signature_expired","detail":"signature outside its validity window"}"#,
    )
    .await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert!(error_of(&answer).contains("expired"), "{}", answer.text);
    assert!(!answer.text.contains("validity window"));
}

/// All three spellings World has used for its verification limit.
#[tokio::test]
async fn a_verification_limit_rejection_gets_curated_copy_not_worlds_own_wording() {
    for code in [
        "max_verifications_reached",
        "exceeded_max_verifications",
        "already_verified",
    ] {
        let body = json!({
            "success": false,
            "code": code,
            "detail": "this account has no verifications left",
        });
        let (_, answer) = rejected_by_world(400, &body.to_string()).await;

        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{code}");
        let error = error_of(&answer);
        assert!(error.contains("already been used"), "{code}: {error}");
        assert!(!error.contains("no verifications left"), "{code}");
        assert!(error.contains("pay instead"), "{code}: {error}");
    }
}

#[tokio::test]
async fn a_rejection_with_neither_code_nor_detail_still_gets_safe_generic_copy() {
    let (_, answer) = rejected_by_world(400, r#"{"success":false}"#).await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert!(
        error_of(&answer).contains("verification failed"),
        "{}",
        answer.text
    );
}

#[tokio::test]
async fn an_unrecognised_code_falls_back_to_generic_copy_and_is_logged_for_follow_up() {
    let ((_, answer), lines) = captured::during(rejected_by_world(
        400,
        r#"{"success":false,"code":"some_new_code_world_added","detail":"World's own new explanation"}"#,
    ))
    .await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert!(
        error_of(&answer).contains("verification failed"),
        "{}",
        answer.text
    );
    assert!(!answer.text.contains("World's own new explanation"));
    assert!(!answer.text.contains("some_new_code_world_added"));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("code=some_new_code_world_added")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("detail=World's own new explanation")),
        "{lines:?}"
    );
    assert!(!logs_the_token(&lines), "{lines:?}");
}

#[tokio::test]
async fn a_code_naming_an_inherited_property_gets_generic_copy_not_a_stringified_function() {
    let (_, answer) = rejected_by_world(400, r#"{"success":false,"code":"toString"}"#).await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    let error = error_of(&answer);
    assert!(error.contains("verification failed"), "{error}");
    assert!(!error.contains("native code") && !error.contains("function"));
}

/// `!response.ok` alone refuses, whatever `success` says.
#[tokio::test]
async fn a_4xx_claiming_success_is_still_a_rejection() {
    let (fixture, answer) = rejected_by_world(400, WORLD_ACCEPTS).await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert!(fixture.settled().is_empty());
}

#[tokio::test]
async fn a_2xx_response_with_no_selfie_check_credential_is_a_bad_request() {
    let (fixture, answer) = rejected_by_world(
        200,
        r#"{"success":true,"results":[{"identifier":"orb","success":true,"nullifier":"x"}]}"#,
    )
    .await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        answer.body,
        json!({ "error": "Proof did not include a Selfie Check credential" })
    );
    assert!(fixture.settled().is_empty());
}

#[tokio::test]
async fn a_selfie_result_that_failed_or_names_no_nullifier_is_a_bad_request() {
    for results in [
        json!([{ "identifier": "selfie", "success": false, "nullifier": "n" }]),
        json!([{ "identifier": "selfie", "success": true, "nullifier": "" }]),
        json!([{ "identifier": "selfie", "success": true }]),
        json!([
            { "identifier": "selfie", "success": true, "nullifier": "n1" },
            { "identifier": "selfie", "success": true, "nullifier": "n2" },
        ]),
        json!([]),
    ] {
        let body = json!({ "success": true, "results": results });
        let (_, answer) = rejected_by_world(200, &body.to_string()).await;

        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{results}");
        assert_eq!(
            answer.body,
            json!({ "error": "Proof did not include a Selfie Check credential" }),
            "{results}"
        );
    }
}

/// The TypeScript threw reading each of these, which Next.js answered with a
/// bare 500. They are World making no sense, which this route's own
/// taxonomy answers with a 502.
#[tokio::test]
async fn an_answer_world_could_not_have_meant_is_unreadable() {
    for body in [
        "not json",
        "null",
        r#"{"success":true,"results":"selfie"}"#,
        r#"{"success":true,"results":[null]}"#,
        r#"{"success":true,"results":[{"identifier":"selfie","success":true,"nullifier":7}]}"#,
    ] {
        let ((fixture, answer), lines) = captured::during(rejected_by_world(200, body)).await;

        assert_eq!(answer.status, StatusCode::BAD_GATEWAY, "{body}");
        assert_eq!(
            answer.body,
            json!({ "error": "World ID returned an unreadable response" }),
            "{body}"
        );
        assert!(has_token_ref(&lines), "{body}: {lines:?}");
        assert!(fixture.settled().is_empty(), "{body}");
    }
}

// --- Live mode: refused before World is asked -------------------------------

#[tokio::test]
async fn a_proof_made_for_another_challenge_is_refused_before_ever_calling_world() {
    let fixture = Fixture::accepting().await;
    let proof = proof_for(OTHER_TOKEN);
    fixture.issue_context(TOKEN, "nonce-tok-two").await;

    let answer = fixture.live(&with_proof(TOKEN, &proof)).await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        answer.body,
        json!({ "error": "This proof was not made for this challenge" })
    );
    assert_eq!(fixture.world_calls(), 0);
    // Checked before the replay guard, so the context is still there to spend.
    assert!(
        consume_issued_context(&fixture.db, TOKEN, "nonce-tok-two", NOW)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn a_proof_bound_to_nothing_at_all_is_refused_before_ever_calling_world() {
    let fixture = Fixture::accepting().await;
    let unbound = json!({
        "protocol_version": "3.0",
        "nonce": "test-nonce",
        "responses": [{ "identifier": "selfie", "proof": "0xproof", "nullifier": "world-nullifier" }],
    });

    let answer = fixture.live(&with_proof(TOKEN, &unbound)).await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert!(error_of(&answer).contains("not made for this challenge"));
    assert_eq!(fixture.world_calls(), 0);
}

#[tokio::test]
async fn a_proof_with_no_selfie_check_credential_at_all_is_refused_before_ever_calling_world() {
    let fixture = Fixture::accepting().await;
    let no_selfie = json!({
        "nonce": "nonce-no-selfie",
        "responses": [{ "identifier": "orb", "proof": "0xproof", "nullifier": "world-nullifier" }],
    });

    let answer = fixture.live(&with_proof(TOKEN, &no_selfie)).await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        answer.body,
        json!({ "error": "Proof must include exactly one Selfie Check credential" })
    );
    assert_eq!(fixture.world_calls(), 0);
}

#[tokio::test]
async fn a_proof_carrying_two_selfie_check_credentials_is_refused_before_ever_calling_world() {
    let fixture = Fixture::accepting().await;
    let entry = |n: &str| json!({ "identifier": "selfie", "signal_hash": hash_signal(TOKEN), "proof": n, "nullifier": n });
    let two = json!({ "nonce": "nonce-two-selfies", "responses": [entry("n1"), entry("n2")] });
    fixture.issue_context(TOKEN, "nonce-two-selfies").await;

    let answer = fixture.live(&with_proof(TOKEN, &two)).await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert!(error_of(&answer).contains("exactly one selfie check credential"));
    assert_eq!(fixture.world_calls(), 0);
}

#[tokio::test]
async fn a_proof_carrying_a_context_this_server_never_issued_is_refused_before_ever_calling_world()
{
    let fixture = Fixture::accepting().await;

    let answer = fixture.live(&with_proof(TOKEN, &proof_for(TOKEN))).await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        answer.body,
        json!({ "error": "This proof's signing context was already used, or was never issued for this challenge" })
    );
    assert_eq!(fixture.world_calls(), 0);
}

#[tokio::test]
async fn a_context_issued_for_another_challenge_cannot_be_spent_here() {
    let fixture = Fixture::accepting().await;
    fixture.other_sender_challenge().await;
    fixture.issue_context(OTHER_TOKEN, "nonce-tok").await;

    let answer = fixture.live(&with_proof(TOKEN, &proof_for(TOKEN))).await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert!(error_of(&answer).contains("never issued for this challenge"));
    assert_eq!(fixture.world_calls(), 0);
}

#[tokio::test]
async fn a_proof_whose_signed_context_was_already_spent_is_refused_on_the_second_presentation() {
    let fixture = Fixture::accepting().await;
    let body = with_proof(TOKEN, &proof_for(TOKEN));
    fixture.issue_context(TOKEN, "nonce-tok").await;
    let first = fixture.live(&body).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.text);

    let replayed = fixture.live(&body).await;

    assert_eq!(replayed.status, StatusCode::BAD_REQUEST);
    assert!(error_of(&replayed).contains("already used"));
    assert_eq!(fixture.world_calls(), 1, "the replay never reached World");
    assert_eq!(fixture.settled().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_context_presented_twice_at_once_reaches_world_once() {
    let fixture = Arc::new(Fixture::accepting().await);
    let body = with_proof(TOKEN, &proof_for(TOKEN));
    fixture.issue_context(TOKEN, "nonce-tok").await;

    let (first, second) = tokio::join!(fixture.live(&body), fixture.live(&body));

    let mut statuses = [first.status, second.status];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::BAD_REQUEST]);
    assert_eq!(fixture.world_calls(), 1);
}

#[tokio::test]
async fn a_signing_context_ledger_it_cannot_reach_fails_closed() {
    let fixture = Fixture::accepting().await;
    fixture
        .db
        .run("DROP TABLE issued_rp_contexts", ())
        .await
        .unwrap();

    let (answer, lines) =
        captured::during(fixture.live(&with_proof(TOKEN, &proof_for(TOKEN)))).await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        answer.body,
        json!({ "error": "Could not verify this proof's signing context" })
    );
    assert_eq!(fixture.world_calls(), 0);
    assert!(has_token_ref(&lines), "{lines:?}");
}

/// Live mode keeps no settled check of its own: a context minted while the
/// challenge was open and presented after it settled still reaches the
/// settle step, whose gate repairs a sender whose answer was lost.
#[tokio::test]
async fn live_mode_still_reaches_settlement_for_a_challenge_settled_while_its_context_was_outstanding()
 {
    let fixture = Fixture::accepting().await;
    claim_challenge(&fixture.db, TOKEN, "human", NOW)
        .await
        .unwrap();
    fixture.issue_context(TOKEN, "nonce-outstanding").await;
    let outstanding =
        json!({ "nonce": "nonce-outstanding", "responses": proof_for(TOKEN)["responses"] });

    let answer = fixture.live(&with_proof(TOKEN, &outstanding)).await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert!(fixture.settled()[0].attest_on_chain);
}

// --- Configuration and unexpected failures ----------------------------------

#[tokio::test]
async fn a_missing_rp_id_is_our_misconfiguration_and_exposes_nothing() {
    let fixture = Fixture::accepting().await;
    fixture.issue_context(TOKEN, "nonce-tok").await;

    let answer = fixture
        .send(
            Env::fixed([("IDENTITY_MODE", "live")]),
            &with_proof(TOKEN, &proof_for(TOKEN)).to_string(),
            Settles::Cleared,
        )
        .await;

    assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(answer.text, "");
    assert_eq!(answer.unhandled.as_deref(), Some("WORLD_RP_ID is not set"));
    assert_eq!(fixture.world_calls(), 0);
}

#[tokio::test]
async fn an_unexpected_failure_logs_a_fingerprint_of_the_challenge_token_never_the_token_itself() {
    let fixture = Fixture::accepting().await;
    fixture.issue_context(TOKEN, "nonce-tok").await;

    let (answer, lines) = captured::during(fixture.send(
        Env::fixed([("IDENTITY_MODE", "live")]),
        &with_proof(TOKEN, &proof_for(TOKEN)).to_string(),
        Settles::Cleared,
    ))
    .await;

    assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(has_token_ref(&lines), "{lines:?}");
    assert!(!logs_the_token(&lines), "{lines:?}");
    assert!(
        lines.iter().any(
            |line| line.starts_with("world id verification failed unexpectedly")
                && line.contains("WORLD_RP_ID is not set")
        ),
        "{lines:?}"
    );
}

#[tokio::test]
async fn the_token_fingerprint_is_the_keccak_of_the_token() {
    // keccak256(stringToBytes("tok")), from viem.
    assert_eq!(
        token_fingerprint(TOKEN),
        "0xda03d64422fa290e9f4fda8133195ec7eb805d596b47f456858fed27c07732a6"
    );
}

#[tokio::test]
async fn an_unrecognised_identity_mode_fails_loudly_rather_than_guessing() {
    let fixture = Fixture::accepting().await;

    let (answer, lines) =
        captured::during(fixture.verify("Mock ", &json!({ "token": TOKEN }))).await;

    assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(answer.text, "");
    assert!(has_token_ref(&lines), "{lines:?}");
    assert!(fixture.settled().is_empty());
}

#[tokio::test]
async fn mock_mode_in_production_is_refused_unless_explicitly_allowed() {
    let fixture = Fixture::accepting().await;
    let body = json!({ "token": TOKEN }).to_string();
    let production = |allow: Option<&str>| {
        let mut vars = vec![
            ("IDENTITY_MODE", "mock"),
            ("WORLD_RP_ID", RP_ID),
            ("VERCEL_ENV", "production"),
        ];
        vars.extend(allow.map(|value| ("POSTAGE_ALLOW_MOCK_IN_PRODUCTION", value)));
        Env::fixed(vars)
    };

    for allow in [None, Some("true")] {
        let refused = fixture
            .send(production(allow), &body, Settles::Cleared)
            .await;
        assert_eq!(
            refused.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{allow:?}"
        );
        assert_eq!(refused.text, "");
    }
    assert!(
        fixture.settled().is_empty(),
        "a refused mode attests nothing"
    );

    let allowed = fixture
        .send(production(Some("1")), &body, Settles::Cleared)
        .await;
    assert_eq!(allowed.status, StatusCode::OK);
    assert_eq!(fixture.settled().len(), 1);
}

#[tokio::test]
async fn mock_mode_in_a_preview_deployment_still_verifies() {
    let fixture = Fixture::accepting().await;
    let preview = Env::fixed([
        ("IDENTITY_MODE", "mock"),
        ("WORLD_RP_ID", RP_ID),
        ("VERCEL_ENV", "preview"),
    ]);

    let answer = fixture
        .send(
            preview,
            &json!({ "token": TOKEN }).to_string(),
            Settles::Cleared,
        )
        .await;

    assert_eq!(answer.status, StatusCode::OK);
}

// --- Before any mode is read -----------------------------------------------

#[tokio::test]
async fn a_body_with_nothing_readable_is_refused_before_anything_is_spent() {
    let fixture = Fixture::accepting().await;
    fixture.issue_context(TOKEN, "nonce-tok").await;

    for (body, error) in [
        ("not json", "Body must be JSON"),
        ("", "Body must be JSON"),
        ("null", "A challenge token is required"),
        ("[]", "A challenge token is required"),
        ("42", "A challenge token is required"),
        (r#"{"proof":{}}"#, "A challenge token is required"),
        (r#"{"token":""}"#, "A challenge token is required"),
        (r#"{"token":["tok"]}"#, "Invalid request"),
        (r#"{"token":{"t":1}}"#, "Invalid request"),
    ] {
        for mode in ["live", "mock"] {
            let answer = fixture.send(env(mode), body, Settles::Cleared).await;

            assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{mode} {body}");
            assert_eq!(answer.body, json!({ "error": error }), "{mode} {body}");
        }
    }
    assert_eq!(fixture.world_calls(), 0);
    assert!(fixture.settled().is_empty());
    assert!(
        consume_issued_context(&fixture.db, TOKEN, "nonce-tok", NOW)
            .await
            .unwrap(),
        "no refusal may spend the context"
    );
    // The one context above is the only row: no refusal took a mock slot.
    for slot in 1..MAX_LIVE_CONTEXTS_PER_TOKEN {
        fixture.issue_context(TOKEN, &format!("slot-{slot}")).await;
    }
}

#[tokio::test]
async fn an_unknown_challenge_is_not_found() {
    let fixture = Fixture::accepting().await;

    let answer = fixture
        .live(&with_proof("bogus", &proof_for("bogus")))
        .await;

    assert_eq!(answer.status, StatusCode::NOT_FOUND);
    assert_eq!(answer.body, json!({ "error": "Unknown challenge" }));
}

#[tokio::test]
async fn a_dangerous_challenge_is_refused_whoever_proves_what() {
    let fixture = Fixture::accepting().await;
    seed(&fixture.db, &challenge("danger", "dangerous")).await;
    fixture.issue_context("danger", "nonce-danger").await;

    for mode in ["live", "mock"] {
        let answer = fixture
            .verify(mode, &with_proof("danger", &proof_for("danger")))
            .await;

        assert_eq!(answer.status, StatusCode::FORBIDDEN, "{mode}");
        assert_eq!(
            answer.body,
            json!({ "error": "This will not be delivered whoever sends it. Being a person does not change that" })
        );
    }
    assert_eq!(fixture.world_calls(), 0);
}

// --- Mock mode ----------------------------------------------------------------

#[tokio::test]
async fn mock_mode_still_verifies_with_no_proof_at_all() {
    let fixture = Fixture::accepting().await;

    let answer = fixture.verify("mock", &json!({ "token": TOKEN })).await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(fixture.world_calls(), 0);
    let settled = fixture.settled();
    assert_eq!(settled[0].nullifier, ledger::mock_nullifier(SENDER));
    assert!(settled[0].attest_on_chain);
}

#[tokio::test]
async fn mock_mode_ignores_whatever_proof_is_sent() {
    let fixture = Fixture::accepting().await;

    let answer = fixture
        .verify("mock", &with_proof(TOKEN, &json!("not a proof")))
        .await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(fixture.world_calls(), 0);
}

#[tokio::test]
async fn mock_mode_still_clears_a_second_senders_own_challenge() {
    let fixture = Fixture::accepting().await;
    fixture.other_sender_challenge().await;

    let first = fixture.verify("mock", &json!({ "token": TOKEN })).await;
    let second = fixture
        .verify("mock", &json!({ "token": OTHER_TOKEN }))
        .await;

    assert_eq!(first.status, StatusCode::OK, "{}", first.text);
    assert_eq!(second.status, StatusCode::OK, "{}", second.text);
    let settled = fixture.settled();
    assert_ne!(
        settled[0].nullifier, settled[1].nullifier,
        "mock derives a nullifier per sender"
    );
}

#[tokio::test]
async fn mock_mode_stops_reaching_the_chain_once_one_token_has_spent_its_verification_attempts() {
    let fixture = Fixture::accepting().await;
    let body = json!({ "token": TOKEN }).to_string();
    for attempt in 1..=MAX_LIVE_CONTEXTS_PER_TOKEN {
        let answer = fixture
            .send(env("mock"), &body, Settles::AttestationFails)
            .await;
        assert_eq!(
            answer.status,
            StatusCode::BAD_GATEWAY,
            "attempt {attempt} should still be spending an allowance"
        );
    }
    let reached = fixture.settled().len();

    let refused = fixture
        .send(env("mock"), &body, Settles::AttestationFails)
        .await;

    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        refused.body,
        json!({ "error": "Too many verification attempts. Wait a moment and try again." })
    );
    assert_eq!(
        fixture.settled().len(),
        reached,
        "a refused attempt never reaches the chain"
    );
}

/// The stage half of two TypeScript tests (the settled challenge buying no
/// second attestation, and the repair of a settled sender): this route must
/// hand over a settled challenge with the chain write switched off, and must
/// not spend one of the token's slots on a request that sends nothing.
#[tokio::test]
async fn a_mock_verification_of_an_already_answered_challenge_asks_for_no_attestation_and_spends_no_slot()
 {
    let fixture = Fixture::accepting().await;
    claim_challenge(&fixture.db, TOKEN, "human", NOW)
        .await
        .unwrap();

    let answer = fixture.verify("mock", &json!({ "token": TOKEN })).await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert!(!fixture.settled()[0].attest_on_chain);
    for slot in 0..MAX_LIVE_CONTEXTS_PER_TOKEN {
        fixture.issue_context(TOKEN, &format!("slot-{slot}")).await;
    }
}

#[tokio::test]
async fn one_tokens_spent_attempts_never_refuse_another_senders_challenge() {
    let fixture = Fixture::accepting().await;
    fixture.other_sender_challenge().await;
    for taken in 0..MAX_LIVE_CONTEXTS_PER_TOKEN {
        fixture
            .issue_context(TOKEN, &format!("spent-{taken}"))
            .await;
    }

    let refused = fixture.verify("mock", &json!({ "token": TOKEN })).await;
    let other = fixture
        .verify("mock", &json!({ "token": OTHER_TOKEN }))
        .await;

    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(other.status, StatusCode::OK, "{}", other.text);
}

/// The stage half: a failed attempt costs one slot, not the sender's chance.
#[tokio::test]
async fn mock_mode_still_lets_a_sender_retry_after_a_failed_attestation() {
    let fixture = Fixture::accepting().await;
    let body = json!({ "token": TOKEN }).to_string();
    let failed = fixture
        .send(env("mock"), &body, Settles::AttestationFails)
        .await;
    assert_eq!(failed.status, StatusCode::BAD_GATEWAY);

    let retried = fixture.send(env("mock"), &body, Settles::Cleared).await;

    assert_eq!(retried.status, StatusCode::OK, "{}", retried.text);
    assert_eq!(fixture.settled().len(), 2);
}

#[tokio::test]
async fn a_mock_attempt_ledger_it_cannot_reach_fails_closed() {
    let fixture = Fixture::accepting().await;
    fixture
        .db
        .run("DROP TABLE issued_rp_contexts", ())
        .await
        .unwrap();

    let (answer, lines) =
        captured::during(fixture.verify("mock", &json!({ "token": TOKEN }))).await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        answer.body,
        json!({ "error": "Could not verify this challenge" })
    );
    assert!(fixture.settled().is_empty());
    assert!(has_token_ref(&lines), "{lines:?}");
}

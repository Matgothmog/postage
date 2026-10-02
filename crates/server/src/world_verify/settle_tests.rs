//! `/api/world/verify` end to end through the router: the onchain
//! attestation, the nullifier binding and the gate, ported from
//! `web/src/app/api/world/verify/route.test.ts`.
//!
//! World and the chain are loopback stubs; nothing here reaches the network.
//! The chain stub answers the JSON-RPC methods an attestation needs from a
//! script each test adjusts, and records anything it was not taught, which
//! fails the test: a route that starts making a call nobody accounted for
//! should say so rather than land on whatever status the test expected.

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use postage_core::attestation::{CREDENTIAL_LIFETIME_SECONDS, identity_for, to_bytes32};
use postage_core::signal::hash_signal;
use serde_json::{Value, json};

use crate::app::AppState;
use crate::config::Env;
use crate::db::challenges::{NewChallenge, challenge_by_token, claim_challenge};
use crate::db::issued_contexts::{MAX_LIVE_CONTEXTS_PER_TOKEN, record_issued_context};
use crate::db::nullifiers::{rebinds_of, sender_holding_nullifier};
use crate::db::passes::{add_paid_use, has_live_pass};
use crate::db::testing::TestDb;
use crate::http_stub::{Reply, Stub, closed_port, serve_with};
use crate::log::captured;
use crate::routes::router;
use crate::routes::testing::{Answer, NOW, challenge, clock_at, keys_in_order, post, seed, send};
use crate::world::WorldVerify;

const HANDLE: &str = "demo";
const TOKEN: &str = "tok";
const OTHER_TOKEN: &str = "tok-two";
const SENDER: &str = "sender@x.com";
const OTHER_SENDER: &str = "attacker@x.com";
const VERIFY_PATH: &str = "/verify/app_test_rp_id";
/// Keyed like a real load-balanced endpoint: a fake key in the path, which a
/// transport failure's message quotes in full.
const RPC_PATH: &str = "/rpc/fake-key-9f3a7c21b5e8";
const RPC_KEY: &str = "fake-key-9f3a7c21b5e8";
const WORLD_ACCEPTS: &str = r#"{"success":true,"results":[{"identifier":"selfie","success":true,"nullifier":"world-nullifier"}]}"#;

/// The largest `uint40`: the registry already holds a fresher record than
/// any attempt here, so the attestation returns before sending anything.
const HUMAN_UNTIL_MAX: u64 = (1 << 40) - 1;
/// Nobody attested yet, so the whole sign, send and wait leg runs.
const HUMAN_UNTIL_NEVER: u64 = 0;
const TX_HASH: &str = "0xcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

/// What `world-nullifier` becomes as a ledger key: it is not 32 bytes of hex,
/// so it is hashed.
fn nullifier_hash() -> String {
    to_bytes32("world-nullifier").to_string()
}

fn mock_nullifier_hash(sender: &str) -> String {
    to_bytes32(&super::ledger::mock_nullifier(sender)).to_string()
}

fn expires_at() -> i64 {
    NOW + CREDENTIAL_LIFETIME_SECONDS
}

/// How the chain stub answers, adjusted per test.
struct Script {
    /// Answers to `humanUntil`, one per read; the last repeats.
    human_until: Vec<u64>,
    human_until_reads: usize,
    receipt_status: &'static str,
    /// Methods answered with a JSON-RPC error: method, code, message.
    failures: Vec<(&'static str, i64, &'static str)>,
    unexpected: Vec<String>,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            human_until: vec![HUMAN_UNTIL_MAX],
            human_until_reads: 0,
            receipt_status: "0x1",
            failures: Vec::new(),
            unexpected: Vec::new(),
        }
    }
}

fn word(value: u64) -> String {
    format!("0x{value:064x}")
}

fn receipt(status: &str) -> Value {
    json!({
        "transactionHash": TX_HASH,
        "transactionIndex": "0x0",
        "blockHash": format!("0x{}", "ef".repeat(32)),
        "blockNumber": "0x1",
        "from": "0x00000000000000000000000000000000000000aa",
        "to": "0x0f9a1c7e971df81adc1b0335a527b30b6f136d05",
        "cumulativeGasUsed": "0x186a0",
        "gasUsed": "0x186a0",
        "effectiveGasPrice": "0x3b9aca00",
        "contractAddress": null,
        "logs": [],
        "logsBloom": format!("0x{}", "00".repeat(256)),
        "type": "0x2",
        "status": status
    })
}

/// The answer to one JSON-RPC method on a working chain, or `None` for one
/// nothing here expects.
fn rpc_result(script: &mut Script, method: &str) -> Option<Value> {
    Some(match method {
        "eth_call" => {
            let last = script.human_until.len().saturating_sub(1);
            let answer = script.human_until[script.human_until_reads.min(last)];
            script.human_until_reads += 1;
            json!(word(answer))
        }
        "eth_getTransactionCount" | "eth_blockNumber" => json!("0x1"),
        "eth_estimateGas" => json!("0x186a0"),
        "eth_gasPrice" | "eth_maxPriorityFeePerGas" => json!("0x3b9aca00"),
        "eth_feeHistory" => json!({
            "oldestBlock": "0x1",
            "baseFeePerGas": ["0x3b9aca00", "0x3b9aca00"],
            "gasUsedRatio": [0.5],
            "reward": [["0x1"]]
        }),
        "eth_sendRawTransaction" => json!(TX_HASH),
        "eth_getTransactionReceipt" => receipt(script.receipt_status),
        _ => return None,
    })
}

async fn chain_node(script: Arc<Mutex<Script>>) -> Stub {
    serve_with(move |sent| {
        let id = sent.body["id"].clone();
        let method = sent.body["method"].as_str().unwrap_or_default();
        let mut script = script.lock().unwrap();
        let failure = script
            .failures
            .iter()
            .find(|(failing, _, _)| *failing == method)
            .copied();
        let body = if let Some((_, code, message)) = failure {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        } else if let Some(result) = rpc_result(&mut script, method) {
            json!({ "jsonrpc": "2.0", "id": id, "result": result })
        } else {
            script.unexpected.push(method.to_owned());
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "not stubbed" } })
        };
        Reply::new(200, body.to_string())
    })
    .await
}

struct Fixture {
    db: Arc<TestDb>,
    world: Stub,
    world_reply: Arc<Mutex<(u16, String)>>,
    node: Stub,
    script: Arc<Mutex<Script>>,
    /// Points `ARC_RPC_URL` at a port nothing listens on, keeping the keyed
    /// path: a real transport failure, whose message quotes the URL.
    unreachable: Mutex<bool>,
}

impl Fixture {
    async fn new() -> Self {
        let db = Arc::new(TestDb::fresh().await);
        seed(&db, &challenge(TOKEN, "commercial")).await;
        let world_reply = Arc::new(Mutex::new((200, WORLD_ACCEPTS.to_owned())));
        let answer = world_reply.clone();
        let world = serve_with(move |sent| {
            if sent.path != VERIFY_PATH {
                return Reply::new(404, "");
            }
            let (status, body) = answer.lock().unwrap().clone();
            Reply::new(status, body)
        })
        .await;
        let script = Arc::new(Mutex::new(Script::default()));
        Self {
            db,
            world,
            world_reply,
            node: chain_node(script.clone()).await,
            script,
            unreachable: Mutex::new(false),
        }
    }

    fn script(&self) -> std::sync::MutexGuard<'_, Script> {
        self.script.lock().unwrap()
    }

    fn human_until(&self, answers: &[u64]) {
        self.script().human_until = answers.to_vec();
    }

    fn fail_rpc(&self, method: &'static str, code: i64, message: &'static str) {
        self.script().failures.push((method, code, message));
    }

    fn chain_unreachable(&self, unreachable: bool) {
        *self.unreachable.lock().unwrap() = unreachable;
    }

    fn world_answers(&self, status: u16, body: &str) {
        *self.world_reply.lock().unwrap() = (status, body.to_owned());
    }

    fn rpc_url(&self) -> String {
        let base = if *self.unreachable.lock().unwrap() {
            closed_port()
        } else {
            self.node.base.clone()
        };
        format!("{base}{RPC_PATH}")
    }

    fn env(&self, mode: &str) -> Env {
        let attester = format!("0x{}", "11".repeat(32));
        let relayer = format!("0x{}", "22".repeat(32));
        Env::fixed([
            ("IDENTITY_MODE", mode.to_owned()),
            ("WORLD_RP_ID", "app_test_rp_id".to_owned()),
            ("ARC_RPC_URL", self.rpc_url()),
            ("ATTESTER_PRIVATE_KEY", attester),
            ("RELAYER_PRIVATE_KEY", relayer),
        ])
    }

    async fn verify(&self, mode: &str, body: &Value) -> Answer {
        let state = AppState::builder(self.env(mode))
            .clock(clock_at(NOW))
            .db(self.db.clone())
            .world(WorldVerify::default().with_base(&self.world.base))
            .build();
        send(router(state), post("/api/world/verify", &body.to_string())).await
    }

    async fn live(&self, token: &str, proof: &Value) -> Answer {
        self.verify("live", &json!({ "token": token, "proof": proof }))
            .await
    }

    async fn mock(&self, token: &str) -> Answer {
        self.verify("mock", &json!({ "token": token })).await
    }

    /// Simulates the `/api/world/context` call every live proof follows.
    async fn issue_context(&self, token: &str, nonce: &str) {
        let issued = record_issued_context(&self.db, token, nonce, NOW, NOW + 300, NOW)
            .await
            .unwrap();
        assert!(issued, "test setup: the context has to be recorded");
    }

    /// Puts the nullifier in the first sender's hands the honest way.
    async fn bind_to_first_sender(&self) {
        let proof = proof_for(TOKEN);
        self.issue_context(TOKEN, &nonce_of(&proof)).await;
        let bound = self.live(TOKEN, &proof).await;
        assert_eq!(bound.status, StatusCode::OK, "test setup: {}", bound.text);
    }

    async fn create_challenge(&self, token: &str, sender: &str) {
        seed(
            &self.db,
            &NewChallenge {
                sender: sender.to_owned(),
                ..challenge(token, "commercial")
            },
        )
        .await;
    }

    /// A second sender with a challenge and a signing context of their own,
    /// set up legitimately so only the proof is ever in question.
    async fn challenge_for_other_sender(&self) -> Value {
        self.create_challenge(OTHER_TOKEN, OTHER_SENDER).await;
        let proof = proof_for(OTHER_TOKEN);
        self.issue_context(OTHER_TOKEN, &nonce_of(&proof)).await;
        proof
    }

    async fn holder(&self, nullifier_hash: &str) -> Option<String> {
        sender_holding_nullifier(&self.db, nullifier_hash)
            .await
            .unwrap()
    }

    async fn hops(&self, nullifier_hash: &str) -> Vec<(String, String)> {
        rebinds_of(&self.db, nullifier_hash)
            .await
            .unwrap()
            .into_iter()
            .map(|hop| (hop.from_sender, hop.to_sender))
            .collect()
    }

    async fn has_pass(&self, sender: &str) -> bool {
        has_live_pass(&self.db, HANDLE, sender, NOW).await.unwrap()
    }

    async fn resolved_at(&self, token: &str) -> Option<i64> {
        challenge_by_token(&self.db, token)
            .await
            .unwrap()
            .unwrap()
            .resolved_at
    }

    fn rpc_calls(&self, method: &str) -> usize {
        self.rpc_methods().iter().filter(|m| *m == method).count()
    }

    fn rpc_methods(&self) -> Vec<String> {
        self.node
            .sent()
            .iter()
            .map(|sent| sent.body["method"].as_str().unwrap_or_default().to_owned())
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let unexpected = self.script.lock().unwrap().unexpected.clone();
        assert!(
            unexpected.is_empty(),
            "the chain stub saw methods nobody taught it: {unexpected:?}"
        );
    }
}

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

fn nonce_of(proof: &Value) -> String {
    proof["nonce"].as_str().unwrap().to_owned()
}

fn error_of(answer: &Answer) -> String {
    answer.body["error"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase()
}

fn line_starting<'a>(lines: &'a [String], event: &str) -> &'a str {
    lines
        .iter()
        .find(|line| line.starts_with(event))
        .unwrap_or_else(|| panic!("no {event:?} line in {lines:#?}"))
}

// --- What a cleared verification answers ------------------------------------

#[tokio::test]
async fn a_live_verification_answers_the_gate_with_the_record_behind_it() {
    let fixture = Fixture::new().await;
    let proof = proof_for(TOKEN);
    fixture.issue_context(TOKEN, &nonce_of(&proof)).await;

    let answer = fixture.live(TOKEN, &proof).await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let hash = to_bytes32("world-nullifier");
    assert_eq!(
        answer.body,
        json!({
            "status": "cleared",
            "reason": "human",
            "delivered": false,
            "identity": identity_for(&hash).to_string(),
            "nullifierHash": nullifier_hash(),
            "expiresAt": expires_at(),
        })
    );
    assert_eq!(
        keys_in_order(&answer.text),
        [
            "status",
            "reason",
            "delivered",
            "identity",
            "nullifierHash",
            "expiresAt"
        ]
    );
}

#[tokio::test]
async fn the_identity_is_a_checksummed_address_derived_from_the_nullifier() {
    let fixture = Fixture::new().await;
    let proof = proof_for(TOKEN);
    fixture.issue_context(TOKEN, &nonce_of(&proof)).await;

    let live = fixture.live(TOKEN, &proof).await;

    let identity = live.body["identity"].as_str().unwrap();
    assert_eq!(identity.len(), 42);
    assert!(identity.starts_with("0x"));
    assert!(identity[2..].bytes().all(|b| b.is_ascii_hexdigit()));
    // viem's getAddress of the same nullifier, pinned in postage-core.
    assert_eq!(identity, "0x8654dB70bEE64d31098Ab01cE780e538fAF5B081");
}

#[tokio::test]
async fn mock_mode_answers_with_the_identity_of_the_senders_stand_in_nullifier() {
    let fixture = Fixture::new().await;

    let answer = fixture.mock(TOKEN).await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let hash = to_bytes32(&super::ledger::mock_nullifier(SENDER));
    assert_eq!(answer.body["identity"], identity_for(&hash).to_string());
    assert_eq!(answer.body["nullifierHash"], hash.to_string());
    assert_eq!(
        fixture.holder(&hash.to_string()).await.as_deref(),
        Some(SENDER)
    );
}

// --- A second sender takes a binding over ------------------------------------

#[tokio::test]
async fn a_second_sender_presenting_one_persons_nullifier_takes_the_binding_over() {
    let fixture = Fixture::new().await;
    fixture.bind_to_first_sender().await;
    let other_proof = fixture.challenge_for_other_sender().await;

    // Properly signed for the second sender's own challenge. Refusing it made
    // a nullifier spent on somebody else's token lost for good; the person it
    // names moves it back by doing exactly this.
    let moved = fixture.live(OTHER_TOKEN, &other_proof).await;

    assert_eq!(moved.status, StatusCode::OK, "{}", moved.text);
    assert_eq!(
        fixture.holder(&nullifier_hash()).await.as_deref(),
        Some(OTHER_SENDER)
    );
}

#[tokio::test]
async fn a_takeover_releases_the_sender_who_held_the_nullifier_before_it() {
    let fixture = Fixture::new().await;
    fixture.bind_to_first_sender().await;
    let other_proof = fixture.challenge_for_other_sender().await;

    fixture.live(OTHER_TOKEN, &other_proof).await;

    assert_ne!(
        fixture.holder(&nullifier_hash()).await.as_deref(),
        Some(SENDER)
    );
}

#[tokio::test]
async fn a_takeover_is_recorded_so_the_move_is_visible_after_the_fact() {
    let fixture = Fixture::new().await;
    fixture.bind_to_first_sender().await;
    let other_proof = fixture.challenge_for_other_sender().await;

    let (_, lines) = captured::during(fixture.live(OTHER_TOKEN, &other_proof)).await;

    assert_eq!(
        fixture.hops(&nullifier_hash()).await,
        [(SENDER.to_owned(), OTHER_SENDER.to_owned())]
    );
    assert_eq!(
        line_starting(&lines, "world id nullifier rebound to a new sender"),
        format!(
            "world id nullifier rebound to a new sender released_from={SENDER} claimed_by={OTHER_SENDER}"
        )
    );
}

#[tokio::test]
async fn a_takeover_does_not_tell_the_new_sender_who_held_the_nullifier_before_them() {
    let fixture = Fixture::new().await;
    fixture.bind_to_first_sender().await;
    let other_proof = fixture.challenge_for_other_sender().await;

    let moved = fixture.live(OTHER_TOKEN, &other_proof).await;

    assert_eq!(moved.status, StatusCode::OK);
    assert!(!moved.text.contains(SENDER), "{}", moved.text);
}

#[tokio::test]
async fn a_takeover_revokes_the_earned_pass_the_released_sender_was_standing_on() {
    let fixture = Fixture::new().await;
    fixture.bind_to_first_sender().await;
    assert!(
        fixture.has_pass(SENDER).await,
        "test setup: clearing must earn a pass"
    );
    let other_proof = fixture.challenge_for_other_sender().await;

    fixture.live(OTHER_TOKEN, &other_proof).await;

    assert!(!fixture.has_pass(SENDER).await);
}

#[tokio::test]
async fn a_takeover_never_revokes_a_paid_pass_on_the_released_sender() {
    let fixture = Fixture::new().await;
    // Paid first, so the human grant folds onto a row still carrying a paid,
    // unspent use, which must survive both the merge and the takeover.
    add_paid_use(&fixture.db, HANDLE, SENDER, NOW)
        .await
        .unwrap();
    fixture.bind_to_first_sender().await;
    let other_proof = fixture.challenge_for_other_sender().await;

    fixture.live(OTHER_TOKEN, &other_proof).await;

    assert!(fixture.has_pass(SENDER).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_senders_verifying_at_the_same_moment_leave_one_holder_and_one_recorded_move() {
    let fixture = Fixture::new().await;
    fixture.create_challenge(OTHER_TOKEN, OTHER_SENDER).await;
    let proof = proof_for(TOKEN);
    fixture.issue_context(TOKEN, &nonce_of(&proof)).await;
    let other_proof = proof_for(OTHER_TOKEN);
    fixture
        .issue_context(OTHER_TOKEN, &nonce_of(&other_proof))
        .await;

    let (first, second) = tokio::join!(
        fixture.live(TOKEN, &proof),
        fixture.live(OTHER_TOKEN, &other_proof)
    );

    assert_eq!(first.status, StatusCode::OK, "{}", first.text);
    assert_eq!(second.status, StatusCode::OK, "{}", second.text);
    // Whichever landed second holds it, and the trail says so: one hop,
    // ending where the ledger now points.
    let hops = fixture.hops(&nullifier_hash()).await;
    assert_eq!(hops.len(), 1, "{hops:?}");
    assert_eq!(
        Some(hops[0].1.clone()),
        fixture.holder(&nullifier_hash()).await
    );
}

// --- What cannot move a binding ------------------------------------------------

/// The binding the first sender holds, unmoved and with no hop recorded.
async fn assert_still_held_by_first_sender(fixture: &Fixture) {
    assert_eq!(
        fixture.holder(&nullifier_hash()).await.as_deref(),
        Some(SENDER)
    );
    assert!(fixture.hops(&nullifier_hash()).await.is_empty());
}

#[tokio::test]
async fn a_proof_world_rejects_cannot_move_a_binding() {
    let fixture = Fixture::new().await;
    fixture.bind_to_first_sender().await;
    let other_proof = fixture.challenge_for_other_sender().await;
    fixture.world_answers(400, r#"{"success":false,"detail":"invalid rp signature"}"#);

    let answer = fixture.live(OTHER_TOKEN, &other_proof).await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_still_held_by_first_sender(&fixture).await;
}

#[tokio::test]
async fn a_proof_made_for_another_challenge_cannot_move_a_binding() {
    let fixture = Fixture::new().await;
    fixture.bind_to_first_sender().await;
    fixture.challenge_for_other_sender().await;

    let answer = fixture.live(OTHER_TOKEN, &proof_for(TOKEN)).await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_still_held_by_first_sender(&fixture).await;
}

#[tokio::test]
async fn a_proof_carrying_a_context_this_server_never_issued_cannot_move_a_binding() {
    let fixture = Fixture::new().await;
    fixture.bind_to_first_sender().await;
    // No context for the second sender: the single-use nonce gate has to
    // hold the ledger still on its own.
    fixture.create_challenge(OTHER_TOKEN, OTHER_SENDER).await;

    let answer = fixture.live(OTHER_TOKEN, &proof_for(OTHER_TOKEN)).await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_still_held_by_first_sender(&fixture).await;
}

#[tokio::test]
async fn the_same_person_clearing_a_second_challenge_of_their_own_moves_nothing() {
    let fixture = Fixture::new().await;
    fixture.create_challenge(OTHER_TOKEN, SENDER).await;
    let proof = proof_for(TOKEN);
    fixture.issue_context(TOKEN, &nonce_of(&proof)).await;
    let other_proof = proof_for(OTHER_TOKEN);
    fixture
        .issue_context(OTHER_TOKEN, &nonce_of(&other_proof))
        .await;

    let first = fixture.live(TOKEN, &proof).await;
    let second = fixture.live(OTHER_TOKEN, &other_proof).await;

    assert_eq!(first.status, StatusCode::OK, "{}", first.text);
    // A pass lasts fifteen minutes, so proving again is the design.
    assert_eq!(second.status, StatusCode::OK, "{}", second.text);
    assert!(fixture.hops(&nullifier_hash()).await.is_empty());
}

// --- A failed attestation: nothing moves -------------------------------------

/// A live verification whose chain cannot be reached at all.
async fn live_with_chain_down(fixture: &Fixture) -> (Answer, Vec<String>) {
    let proof = proof_for(TOKEN);
    fixture.issue_context(TOKEN, &nonce_of(&proof)).await;
    fixture.chain_unreachable(true);
    captured::during(fixture.live(TOKEN, &proof)).await
}

#[tokio::test]
async fn an_rpc_failure_recording_personhood_does_not_leak_the_rpc_endpoint() {
    let fixture = Fixture::new().await;

    let (answer, _) = live_with_chain_down(&fixture).await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert!(!answer.text.contains(RPC_KEY), "{}", answer.text);
    assert!(!answer.text.contains("127.0.0.1"), "{}", answer.text);
    assert!(error_of(&answer).contains("could not record the attestation"));
}

#[tokio::test]
async fn a_keyed_rpc_endpoint_does_not_survive_into_the_log_line_even_nested_under_a_cause_chain() {
    let fixture = Fixture::new().await;

    let (answer, lines) = live_with_chain_down(&fixture).await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    let line = line_starting(&lines, "world id attestation failed");
    let reason = line.split_once(" reason=").unwrap().1;
    assert!(reason.contains(": "), "a joined chain: {reason}");
    assert!(!line.contains(RPC_KEY), "the key leaked: {line}");
    assert!(!line.contains(RPC_PATH), "the URL leaked: {line}");
    assert!(reason.contains("[redacted ARC_RPC_URL]"), "{reason}");
    assert!(line.contains("tokenRef=0x"), "{line}");
}

#[tokio::test]
async fn a_failed_attestation_leaves_no_nullifier_bound_to_anyone() {
    let fixture = Fixture::new().await;

    let (answer, _) = live_with_chain_down(&fixture).await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(fixture.holder(&nullifier_hash()).await, None);
    assert!(fixture.hops(&nullifier_hash()).await.is_empty());
}

#[tokio::test]
async fn a_failed_attestation_grants_no_pass_and_leaves_the_challenge_unsettled() {
    let fixture = Fixture::new().await;

    let (answer, _) = live_with_chain_down(&fixture).await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert!(!fixture.has_pass(SENDER).await);
    assert_eq!(fixture.resolved_at(TOKEN).await, None);
}

/// Binding before attesting stripped a third party's free lane behind a 502
/// saying nothing had happened. No concurrent variant: a failed attestation
/// never reaches the ledger, so two of them share nothing to race over.
#[tokio::test]
async fn a_failed_attestation_cannot_take_a_binding_off_the_sender_who_holds_it() {
    let fixture = Fixture::new().await;
    fixture.bind_to_first_sender().await;
    assert!(
        fixture.has_pass(SENDER).await,
        "test setup: standing on a pass"
    );
    let other_proof = fixture.challenge_for_other_sender().await;
    fixture.chain_unreachable(true);

    let answer = fixture.live(OTHER_TOKEN, &other_proof).await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_still_held_by_first_sender(&fixture).await;
    assert!(fixture.has_pass(SENDER).await);
}

#[tokio::test]
async fn a_failed_attestation_tells_the_sender_nothing_was_saved() {
    let fixture = Fixture::new().await;

    let (answer, _) = live_with_chain_down(&fixture).await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        answer.body,
        json!({ "error": "Could not record the attestation. Nothing was saved. Try again, or pay instead" })
    );
}

/// Mock mode reaches the chain write with no proof and no context, which is
/// the configuration the failure was found under on a deployed preview.
#[tokio::test]
async fn a_failed_attestation_under_mock_mode_leaves_no_binding_either() {
    let fixture = Fixture::new().await;
    fixture.chain_unreachable(true);

    let answer = fixture.mock(TOKEN).await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(fixture.holder(&mock_nullifier_hash(SENDER)).await, None);
    assert!(!fixture.has_pass(SENDER).await);
}

#[tokio::test]
async fn a_retry_after_a_failed_attestation_clears_the_challenge_and_records_one_binding() {
    let fixture = Fixture::new().await;
    let (failed, _) = live_with_chain_down(&fixture).await;
    assert_eq!(failed.status, StatusCode::BAD_GATEWAY);

    // A fresh Selfie Check is a fresh signing context: the spent nonce is
    // not presentable again, by design.
    fixture.chain_unreachable(false);
    let mut retry = proof_for(TOKEN);
    retry["nonce"] = json!("nonce-retry");
    fixture.issue_context(TOKEN, "nonce-retry").await;

    let retried = fixture.live(TOKEN, &retry).await;

    assert_eq!(retried.status, StatusCode::OK, "{}", retried.text);
    assert_eq!(
        fixture.holder(&nullifier_hash()).await.as_deref(),
        Some(SENDER)
    );
    assert!(fixture.hops(&nullifier_hash()).await.is_empty());
    assert!(fixture.has_pass(SENDER).await);
}

// --- Sent, but never confirmed -----------------------------------------------

/// A live verification whose attestation is sent and whose receipt read
/// fails: the one case where "nothing was saved" would be a guess.
async fn live_with_receipt_unreadable(fixture: &Fixture) -> (Answer, Vec<String>) {
    let proof = proof_for(TOKEN);
    fixture.issue_context(TOKEN, &nonce_of(&proof)).await;
    fixture.human_until(&[HUMAN_UNTIL_NEVER]);
    fixture.fail_rpc(
        "eth_getTransactionReceipt",
        -32000,
        "simulated: receipt temporarily unavailable",
    );
    captured::during(fixture.live(TOKEN, &proof)).await
}

#[tokio::test]
async fn send_succeeded_but_the_receipt_wait_failed_the_sender_is_told_the_outcome_is_unknown() {
    let fixture = Fixture::new().await;

    let (answer, _) = live_with_receipt_unreadable(&fixture).await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        fixture.rpc_calls("eth_sendRawTransaction"),
        1,
        "test setup: the send has to have gone out"
    );
    assert_eq!(
        answer.body,
        json!({ "error": "Could not confirm the attestation in time. It may still complete on its own. Try again, or pay instead" })
    );
    assert!(!error_of(&answer).contains("nothing was saved"));
    assert_eq!(fixture.holder(&nullifier_hash()).await, None);
    assert!(!fixture.has_pass(SENDER).await);
}

#[tokio::test]
async fn an_unknown_attestation_outcome_logs_what_actually_failed_not_just_the_hash() {
    let fixture = Fixture::new().await;

    let (answer, lines) = live_with_receipt_unreadable(&fixture).await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    let line = line_starting(&lines, "world id attestation failed");
    assert!(line.contains(TX_HASH), "the hash to reconcile by: {line}");
    assert!(
        line.contains("receipt temporarily unavailable"),
        "and which failure it was: {line}"
    );
}

// --- A receipt is not a success ----------------------------------------------

/// A live verification whose attestation is mined and reverted, with the
/// registry reading `human_until` before sending and after the revert.
async fn live_with_revert(fixture: &Fixture, human_until: &[u64]) -> (Answer, Vec<String>) {
    let proof = proof_for(TOKEN);
    fixture.issue_context(TOKEN, &nonce_of(&proof)).await;
    fixture.human_until(human_until);
    fixture.script().receipt_status = "0x0";
    captured::during(fixture.live(TOKEN, &proof)).await
}

#[tokio::test]
async fn an_attestation_that_reverts_onchain_is_not_personhood() {
    let fixture = Fixture::new().await;

    let (answer, _) = live_with_revert(&fixture, &[HUMAN_UNTIL_NEVER]).await;

    // The receipt confirmed the revert: "nothing was saved" is known here.
    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert!(error_of(&answer).contains("nothing was saved"));
    assert!(error_of(&answer).contains("try again"));
    assert!(error_of(&answer).contains("pay instead"));
    assert_eq!(fixture.holder(&nullifier_hash()).await, None);
    assert!(!fixture.has_pass(SENDER).await);
}

/// The race this exists for: two verifications of one nullifier both pass
/// the pre-send read, and the second to land reverts against the record the
/// first just wrote.
#[tokio::test]
async fn a_revert_against_an_identity_already_fresh_on_chain_is_granted_the_lane_anyway() {
    let fixture = Fixture::new().await;

    let (answer, _) = live_with_revert(&fixture, &[HUMAN_UNTIL_NEVER, HUMAN_UNTIL_MAX]).await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(
        fixture.holder(&nullifier_hash()).await.as_deref(),
        Some(SENDER)
    );
    assert!(fixture.has_pass(SENDER).await);
    assert_eq!(
        fixture.script().human_until_reads,
        2,
        "one read before sending, one after the revert"
    );
}

/// Asking "is there any live record" instead of "one at least this fresh"
/// excused every revert for anyone attested in the last ninety days.
#[tokio::test]
async fn a_revert_against_an_identity_whose_record_is_live_but_older_than_this_attempt_is_not_personhood()
 {
    let fixture = Fixture::new().await;
    let live_for_a_day = u64::try_from(NOW + 86_400).unwrap();

    let (answer, _) = live_with_revert(&fixture, &[HUMAN_UNTIL_NEVER, live_for_a_day]).await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert!(error_of(&answer).contains("nothing was saved"));
    assert_eq!(fixture.holder(&nullifier_hash()).await, None);
    assert!(!fixture.has_pass(SENDER).await);
}

#[tokio::test]
async fn a_revert_granted_the_lane_anyway_is_still_logged_with_the_reading_that_justified_it() {
    let fixture = Fixture::new().await;

    let (answer, lines) = live_with_revert(&fixture, &[HUMAN_UNTIL_NEVER, HUMAN_UNTIL_MAX]).await;

    assert_eq!(answer.status, StatusCode::OK);
    let identity = identity_for(&to_bytes32("world-nullifier"));
    assert_eq!(
        line_starting(&lines, "attestation transaction reverted"),
        format!(
            "attestation transaction reverted hash={TX_HASH} identity={identity} humanUntil={HUMAN_UNTIL_MAX} expiresAt={} grantedAnyway=true",
            expires_at()
        )
    );
}

#[tokio::test]
async fn an_attestation_that_mines_successfully_clears_the_challenge() {
    let fixture = Fixture::new().await;
    let proof = proof_for(TOKEN);
    fixture.issue_context(TOKEN, &nonce_of(&proof)).await;
    fixture.human_until(&[HUMAN_UNTIL_NEVER]);

    let answer = fixture.live(TOKEN, &proof).await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(fixture.rpc_calls("eth_sendRawTransaction"), 1);
    assert_eq!(
        fixture.holder(&nullifier_hash()).await.as_deref(),
        Some(SENDER)
    );
    assert!(fixture.has_pass(SENDER).await);
    assert_eq!(fixture.resolved_at(TOKEN).await, Some(NOW));
}

/// Sending through one provider and waiting through another can broadcast a
/// transaction the wait never sees. Every call of the leg reaches the one
/// configured endpoint, the keyed `ARC_RPC_URL`, and nothing else is asked.
#[tokio::test]
async fn the_attestation_write_and_its_receipt_wait_share_one_rpc_endpoint() {
    let fixture = Fixture::new().await;
    let proof = proof_for(TOKEN);
    fixture.issue_context(TOKEN, &nonce_of(&proof)).await;
    fixture.human_until(&[HUMAN_UNTIL_NEVER]);

    let answer = fixture.live(TOKEN, &proof).await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let sent = fixture.node.sent();
    assert!(sent.iter().all(|call| call.path == RPC_PATH), "{sent:?}");
    assert_eq!(fixture.rpc_calls("eth_call"), 1);
    assert_eq!(fixture.rpc_calls("eth_sendRawTransaction"), 1);
    assert!(fixture.rpc_calls("eth_getTransactionReceipt") >= 1);
}

// --- Mock mode and the chain ---------------------------------------------------

#[tokio::test]
async fn mock_mode_stops_reaching_the_chain_once_one_token_has_spent_its_verification_attempts() {
    let fixture = Fixture::new().await;
    fixture.fail_rpc("eth_call", -32000, "simulated node failure");
    for attempt in 1..=MAX_LIVE_CONTEXTS_PER_TOKEN {
        let answer = fixture.mock(TOKEN).await;
        assert_eq!(answer.status, StatusCode::BAD_GATEWAY, "attempt {attempt}");
    }
    let spent = fixture.rpc_methods().len();
    assert!(
        spent > 0,
        "the attempts above have to have reached the chain"
    );

    let refused = fixture.mock(TOKEN).await;

    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(error_of(&refused).contains("too many verification attempts"));
    assert_eq!(
        fixture.rpc_methods().len(),
        spent,
        "a refusal reaches no chain"
    );
}

/// Live mode refuses a settled token at `/api/world/context`. Mock mode
/// skips the write instead and still opens the gate, which reports what
/// happened; the relayer pays nothing either way.
#[tokio::test]
async fn a_mock_verification_of_an_already_answered_challenge_buys_no_second_attestation() {
    let fixture = Fixture::new().await;
    fixture.human_until(&[HUMAN_UNTIL_NEVER]);
    let first = fixture.mock(TOKEN).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.text);
    assert_eq!(
        fixture.rpc_calls("eth_sendRawTransaction"),
        1,
        "test setup: the first attempt pays for one attestation"
    );
    let reached_chain = fixture.rpc_methods().len();

    let second = fixture.mock(TOKEN).await;

    assert_eq!(second.status, StatusCode::OK, "{}", second.text);
    assert_eq!(second.body["status"], "cleared");
    assert_eq!(fixture.rpc_calls("eth_sendRawTransaction"), 1);
    assert_eq!(
        fixture.rpc_methods().len(),
        reached_chain,
        "not even the pre-send read"
    );
}

#[tokio::test]
async fn a_mock_verification_repairs_a_settled_sender_who_holds_nothing_and_still_spends_no_gas() {
    let fixture = Fixture::new().await;
    assert!(
        claim_challenge(&fixture.db, TOKEN, "human", NOW)
            .await
            .unwrap()
    );
    assert!(
        !fixture.has_pass(SENDER).await,
        "test setup: holding nothing"
    );
    fixture.human_until(&[HUMAN_UNTIL_NEVER]);

    let answer = fixture.mock(TOKEN).await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert!(fixture.has_pass(SENDER).await);
    assert!(fixture.rpc_methods().is_empty(), "not even a read");
}

#[tokio::test]
async fn mock_mode_still_lets_a_sender_retry_after_a_failed_attestation() {
    let fixture = Fixture::new().await;
    fixture.chain_unreachable(true);
    let failed = fixture.mock(TOKEN).await;
    assert_eq!(failed.status, StatusCode::BAD_GATEWAY);

    fixture.chain_unreachable(false);
    let retried = fixture.mock(TOKEN).await;

    assert_eq!(retried.status, StatusCode::OK, "{}", retried.text);
    assert!(fixture.has_pass(SENDER).await);
}

/// A context minted before the challenge settled and presented after: the
/// gate's recover path, which a settled check on the shared path would have
/// turned into a refusal.
#[tokio::test]
async fn live_mode_still_clears_a_challenge_that_was_settled_while_its_context_was_outstanding() {
    let fixture = Fixture::new().await;
    fixture.bind_to_first_sender().await;
    let mut outstanding = proof_for(TOKEN);
    outstanding["nonce"] = json!("nonce-outstanding");
    fixture.issue_context(TOKEN, "nonce-outstanding").await;

    let answer = fixture.live(TOKEN, &outstanding).await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(answer.body["status"], "cleared");
    assert_eq!(answer.body["reason"], "human");
}

// --- The ledger ------------------------------------------------------------------

/// Nothing in the `nullifiers` table identifies anyone on its own; a sender
/// next to the nullifier hash bound to it is the one pairing that would.
#[tokio::test]
async fn a_ledger_failure_while_binding_a_nullifier_never_logs_the_sender_and_the_nullifier_hash_together()
 {
    let fixture = Fixture::new().await;
    let proof = proof_for(TOKEN);
    fixture.issue_context(TOKEN, &nonce_of(&proof)).await;
    fixture.db.run("DROP TABLE nullifiers", ()).await.unwrap();

    let (answer, lines) = captured::during(fixture.live(TOKEN, &proof)).await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        answer.body,
        json!({ "error": "Could not check this World ID" })
    );
    let hash = nullifier_hash();
    let line = line_starting(&lines, "nullifier ledger unavailable");
    assert!(
        line.contains(&hash),
        "diagnosable by nullifier hash: {line}"
    );
    assert!(
        !lines
            .iter()
            .any(|line| line.contains(SENDER) && line.contains(&hash)),
        "{lines:#?}"
    );
    assert_eq!(
        fixture.resolved_at(TOKEN).await,
        None,
        "the gate never opened"
    );
}

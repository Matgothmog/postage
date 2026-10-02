//! `POST /api/challenge/resolve` (`web/src/app/api/challenge/resolve/route.ts`):
//! settles the paying half of a challenge. Prove the payment landed, then hand
//! off to the gate, so both lanes answer to one description of clearing.
//!
//! Personhood is not handled here. Proving it needs no wallet and lives in
//! `/api/world/verify`, which checks a proof rather than reading a credential
//! off an address the caller named.

use alloy_primitives::{Address, B256};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use super::js::{TypeError, is_truthy, lookup_text, property, request_json};
use super::{Exit, RouteResult, json, refuse};
use crate::app::AppState;
use crate::chain::ChainError;
use crate::db::challenges::challenge_by_token;
use crate::db::sender_wallets::link_sender_wallet;
use crate::faults::{reason_chain, redact};
use crate::gate::{GateResult, Lane, open_gate};
use crate::log;

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum Resolution {
    Pending,
    Charged { reason: &'static str },
    Cleared { reason: String, delivered: bool },
}

pub(crate) async fn post(State(state): State<AppState>, body: Bytes) -> RouteResult {
    // Unguarded in the TypeScript: a body that is not JSON threw, which
    // Next.js answered with a bare 500.
    let body = request_json(&body)?;
    let token = property(&body, "token")?;
    let Some(token) = token.filter(|token| is_truthy(Some(token))) else {
        return Err(refuse(
            StatusCode::BAD_REQUEST,
            "A challenge token is required",
        ));
    };
    let token = lookup_text(token)?;

    let db = state.db().await?;
    let Some(challenge) = challenge_by_token(db, &token).await? else {
        return Err(refuse(StatusCode::NOT_FOUND, "Unknown challenge"));
    };

    let payer = payer_of(&state, &challenge.message_id).await?;

    // Recorded even when the challenge turns out to have been settled some
    // other way. The money left their wallet either way, and forgetting it
    // prices their next message as a stranger's.
    let now = state.now();
    if let Some(payer) = payer {
        link_sender_wallet(db, &challenge.sender, &payer.to_string(), now).await?;
    }

    // No payment, no paid lane, settled or not. Letting an already-settled
    // challenge through here would hand a delivery to anyone who knew a token
    // that had been answered, without paying for anything.
    if payer.is_none() {
        return Ok(json(StatusCode::OK, &Resolution::Pending));
    }

    let resolution = match open_gate(db, state.mail_worker(), &token, Lane::Paid, now).await? {
        GateResult::Unknown => {
            return Err(refuse(StatusCode::NOT_FOUND, "Unknown challenge"));
        }
        GateResult::Charged => Resolution::Charged {
            reason: "dangerous",
        },
        GateResult::Cleared { reason, delivered } => Resolution::Cleared { reason, delivered },
    };
    Ok(json(StatusCode::OK, &resolution))
}

/// The address the escrow recorded as having paid, or `None` if nobody has.
///
/// A chain that cannot be reached right now is a payment we cannot see yet,
/// which is what "pending" means. Misconfigured is not: a wrong address, an
/// ABI that drifted from the deployed contract, or a call the node refuses
/// would otherwise read exactly like nobody having paid, and every sender
/// whose money had already left their wallet would be told to keep waiting.
/// Those surface as a failure instead.
async fn payer_of(state: &AppState, message_id: &str) -> Result<Option<Address>, Exit> {
    let message_id: B256 = message_id
        .parse()
        .map_err(|_| TypeError(format!("message id {message_id} is not 32 bytes of hex")))?;
    match state.chain()?.settlement_of(message_id).await {
        Ok(settlement) => Ok(settlement.payer()),
        Err(error) if error.is_transient() => {
            log_unreadable_settlement(state, &error);
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

fn log_unreadable_settlement(state: &AppState, error: &ChainError) {
    log::error(
        "Could not read the escrow settlement",
        &[("reason", &redact(state.env(), &reason_chain(error)))],
    );
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use reqwest::Url;
    use serde_json::{Value, json};

    use super::*;
    use crate::chain::Chain;
    use crate::config::Env;
    use crate::db::challenges::claim_challenge;
    use crate::db::inboxes::create_inbox;
    use crate::db::passes::has_live_pass;
    use crate::db::sender_wallets::wallet_for_sender;
    use crate::db::testing::TestDb;
    use crate::hold::MailWorker;
    use crate::http_stub::{Reply, Stub, closed_port, serve_with};
    use crate::log::captured;
    use crate::routes::router;
    use crate::routes::testing::{Answer, NOW, challenge, clock_at, post, seed, send};

    const PATH: &str = "/api/challenge/resolve";
    const TOKEN: &str = "tok";
    const PAYER: &str = "0x00000000000000000000000000000000000000Aa";
    const ZERO: &str = "0x0000000000000000000000000000000000000000";

    /// What the node says to `eth_call`: a result, or a JSON-RPC error.
    type Answered = Result<String, (i64, &'static str)>;

    /// A JSON-RPC node answering every `eth_call` the same way.
    async fn node(answer: Answered) -> Stub {
        serve_with(move |sent| {
            let id = sent.body["id"].clone();
            let body = match &answer {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err((code, message)) => json!({
                    "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message }
                }),
            };
            Reply::new(200, body.to_string())
        })
        .await
    }

    /// `settlementOf`'s return data: inbox, reported, payer.
    fn settlement_data(payer: &str) -> String {
        let payer = payer.trim_start_matches("0x").to_lowercase();
        format!("0x{:0>64}{:0>64}{payer:0>64}", "cd".repeat(20), "0")
    }

    fn settlement(payer: &str) -> Answered {
        Ok(settlement_data(payer))
    }

    struct Fixture {
        db: Arc<TestDb>,
        chain_url: String,
        worker: MailWorker,
        _stubs: Vec<Stub>,
    }

    /// An inbox and a commercial challenge, and a node answering `answer`.
    /// The worker accepts every release.
    async fn fixture(answer: Answered) -> Fixture {
        let db = Arc::new(TestDb::fresh().await);
        create_inbox(
            &db,
            "demo",
            "demo@example.com",
            Some(&format!("0x{}", "11".repeat(20))),
            NOW,
        )
        .await
        .unwrap();
        seed(&db, &challenge(TOKEN, "commercial")).await;
        let chain = node(answer).await;
        let worker_stub = serve_with(|_| Reply::new(200, "{}")).await;
        Fixture {
            db,
            chain_url: chain.base.clone(),
            worker: MailWorker::new(reqwest::Client::default(), &worker_stub.base, "secret"),
            _stubs: vec![chain, worker_stub],
        }
    }

    impl Fixture {
        async fn resolve(&self, body: &str) -> Answer {
            let app = router(
                AppState::builder(Env::empty())
                    .clock(clock_at(NOW))
                    .db(self.db.clone())
                    .chain(Chain::new(Url::parse(&self.chain_url).unwrap()))
                    .mail_worker(self.worker.clone())
                    .build(),
            );
            send(app, post(PATH, body)).await
        }
    }

    fn token_body(token: &str) -> String {
        json!({ "token": token }).to_string()
    }

    #[tokio::test]
    async fn a_paid_challenge_clears_on_the_paid_lane_and_releases_the_held_message() {
        let fixture = fixture(settlement(PAYER)).await;

        let answer = fixture.resolve(&token_body(TOKEN)).await;

        assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
        assert_eq!(
            answer.body,
            json!({ "status": "cleared", "reason": "paid", "delivered": true })
        );
        assert_eq!(
            answer.text,
            r#"{"status":"cleared","reason":"paid","delivered":true}"#
        );
    }

    #[tokio::test]
    async fn the_payer_is_remembered_against_the_sender() {
        let fixture = fixture(settlement(PAYER)).await;

        fixture.resolve(&token_body(TOKEN)).await;

        assert_eq!(
            wallet_for_sender(&fixture.db, "sender@x.com")
                .await
                .unwrap(),
            Some(PAYER.to_lowercase())
        );
    }

    /// The money left their wallet either way, so a challenge some other lane
    /// already settled still records who paid.
    #[tokio::test]
    async fn the_payer_is_remembered_even_when_the_challenge_was_already_settled() {
        let fixture = fixture(settlement(PAYER)).await;
        claim_challenge(&fixture.db, TOKEN, "human", NOW)
            .await
            .unwrap();

        let answer = fixture.resolve(&token_body(TOKEN)).await;

        assert_eq!(answer.body["status"], "cleared");
        assert_eq!(answer.body["reason"], "human");
        assert!(
            wallet_for_sender(&fixture.db, "sender@x.com")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn nobody_having_paid_is_pending() {
        let fixture = fixture(settlement(ZERO)).await;

        let answer = fixture.resolve(&token_body(TOKEN)).await;

        assert_eq!(answer.status, StatusCode::OK);
        assert_eq!(answer.text, r#"{"status":"pending"}"#);
        assert!(
            !has_live_pass(&fixture.db, "demo", "sender@x.com", NOW)
                .await
                .unwrap()
        );
    }

    /// Settled or not: an already-answered challenge must not hand a delivery
    /// to anyone who knows its token without paying.
    #[tokio::test]
    async fn a_settled_challenge_nobody_paid_for_is_still_pending() {
        let fixture = fixture(settlement(ZERO)).await;
        claim_challenge(&fixture.db, TOKEN, "human", NOW)
            .await
            .unwrap();

        let answer = fixture.resolve(&token_body(TOKEN)).await;

        assert_eq!(answer.body, json!({ "status": "pending" }));
    }

    #[tokio::test]
    async fn dangerous_mail_is_charged_rather_than_cleared() {
        let fixture = fixture(settlement(PAYER)).await;
        seed(&fixture.db, &challenge("danger", "dangerous")).await;

        let answer = fixture.resolve(&token_body("danger")).await;

        assert_eq!(answer.text, r#"{"status":"charged","reason":"dangerous"}"#);
    }

    #[tokio::test]
    async fn a_request_with_no_token_is_refused() {
        let fixture = fixture(settlement(PAYER)).await;

        for body in [
            "{}",
            r#"{"token":""}"#,
            r#"{"token":null}"#,
            r#"{"token":0}"#,
        ] {
            let answer = fixture.resolve(body).await;
            assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(
                answer.body,
                json!({ "error": "A challenge token is required" })
            );
        }
    }

    #[tokio::test]
    async fn an_unknown_token_is_refused() {
        let fixture = fixture(settlement(PAYER)).await;

        let answer = fixture.resolve(&token_body("bogus")).await;

        assert_eq!(answer.status, StatusCode::NOT_FOUND);
        assert_eq!(answer.body, json!({ "error": "Unknown challenge" }));
    }

    /// Unreachable is a fine reason to wait: the sender who just paid is told
    /// "pending", not that something is wrong with them.
    #[tokio::test]
    async fn an_unreachable_chain_is_pending_and_logged() {
        let fixture = fixture(settlement(PAYER)).await;
        let app = router(
            AppState::builder(Env::empty())
                .clock(clock_at(NOW))
                .db(fixture.db.clone())
                .chain(Chain::new(Url::parse(&closed_port()).unwrap()))
                .mail_worker(fixture.worker.clone())
                .build(),
        );

        let (answer, lines) = captured::during(send(app, post(PATH, &token_body(TOKEN)))).await;

        assert_eq!(answer.body, json!({ "status": "pending" }));
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].starts_with("Could not read the escrow settlement reason="),
            "{lines:?}"
        );
    }

    #[tokio::test]
    async fn a_node_that_is_struggling_is_pending() {
        let fixture = fixture(Err((-32005, "limit exceeded"))).await;

        let answer = fixture.resolve(&token_body(TOKEN)).await;

        assert_eq!(answer.body, json!({ "status": "pending" }));
    }

    /// The one place this port answers differently from the TypeScript, on
    /// purpose. Its `looksTransient` walked viem's cause chain for
    /// `RpcRequestError`, which viem puts under *every* JSON-RPC error, so a
    /// revert read as "pending" forever, against its own comment. A refused
    /// call is a misconfiguration and surfaces as a failure.
    #[tokio::test]
    async fn a_reverted_call_is_a_failure_rather_than_pending() {
        let fixture = fixture(Err((3, "execution reverted"))).await;

        let (answer, lines) = captured::during(fixture.resolve(&token_body(TOKEN))).await;

        assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(answer.text, "");
        assert!(lines[0].contains("execution reverted"), "{lines:?}");
    }

    #[tokio::test]
    async fn an_answer_that_does_not_decode_is_a_failure_rather_than_pending() {
        let fixture = fixture(Ok("0x".to_owned())).await;

        let answer = fixture.resolve(&token_body(TOKEN)).await;

        assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// Unguarded in the TypeScript, so Next.js answered with a bare 500.
    #[tokio::test]
    async fn a_body_that_is_not_json_is_the_bare_500_the_typescript_gave() {
        let fixture = fixture(settlement(PAYER)).await;

        for body in ["not json", "null"] {
            let answer = fixture.resolve(body).await;
            assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
            assert_eq!(answer.text, "");
        }
    }

    /// The node is asked about the message this challenge names.
    #[tokio::test]
    async fn the_escrow_is_asked_about_this_challenges_message() {
        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let recorded = seen.clone();
        let chain = serve_with(move |sent| {
            recorded.lock().unwrap().push(sent.body.clone());
            let result = settlement_data(ZERO);
            Reply::new(
                200,
                json!({ "jsonrpc": "2.0", "id": sent.body["id"], "result": result }).to_string(),
            )
        })
        .await;
        let fixture = fixture(settlement(ZERO)).await;
        let app = router(
            AppState::builder(Env::empty())
                .clock(clock_at(NOW))
                .db(fixture.db.clone())
                .chain(Chain::new(Url::parse(&chain.base).unwrap()))
                .build(),
        );

        send(app, post(PATH, &token_body(TOKEN))).await;

        let calls = seen.lock().unwrap();
        let input = calls[0]["params"][0]["input"]
            .as_str()
            .or(calls[0]["params"][0]["data"].as_str())
            .unwrap()
            .to_owned();
        assert!(input.ends_with(&"ab".repeat(32)), "{input}");
    }
}

//! The product loop walked through its own public entry points, the way the
//! mail worker and a sender's browser drive it (`web/src/app/api/pipeline.test.ts`).
//!
//! What is real: the inbound gateway, the challenge page's read, the
//! resolve/gate wiring and the database. What is stubbed, each a loopback
//! server: the chain, the classifier's model, and the mail worker's
//! `/release` endpoint.
//!
//! The TypeScript walks the human lane, through `/api/world/verify` under
//! `IDENTITY_MODE=mock`; that walk is here, and so is the paid lane's,
//! through `/api/challenge/resolve`: the same gate, the same release, the
//! same settled row.

use std::sync::Arc;

use alloy_sol_types::SolCall;
use axum::Router;
use axum::http::StatusCode;
use postage_core::contracts::{HUMAN_REGISTRY, PostageEscrow};
use reqwest::Url;
use serde_json::{Value, json};

use super::mail_inbound::tests_support::{WALLET, classifier, floor_word, model, test_env};
use crate::app::AppState;
use crate::chain::Chain;
use crate::config::Env;
use crate::db::challenges::challenge_by_token;
use crate::db::inboxes::create_inbox;
use crate::db::passes::has_live_pass;
use crate::db::sender_wallets::wallet_for_sender;
use crate::db::testing::TestDb;
use crate::hold::MailWorker;
use crate::http_stub::{Reply, Stub, serve_with};
use crate::routes::router;
use crate::routes::testing::{NOW, clock_at, get, post, post_with, send};

const HANDLE: &str = "demo";
const SENDER: &str = "Matgothmog.Angelux@gmail.com";
const DESTINATION: &str = "demo-owner@example.com";
const PAYER: &str = "00000000000000000000000000000000000000aa";

/// The largest `uint40`: every identity is already fresher on chain than any
/// attempt, so the human lane's attestation returns before sending anything.
const HUMAN_UNTIL_MAX: u64 = (1 << 40) - 1;

/// A node that tells the reads these walks make apart: `effectiveFloor` for
/// the price, `settlementOf` for the payment, which it reports as made by
/// [`PAYER`], and the registry's `humanUntil`, answered with
/// [`HUMAN_UNTIL_MAX`].
async fn chain() -> Stub {
    serve_with(|sent| {
        let id = sent.body["id"].clone();
        let call = &sent.body["params"][0];
        let input = call["input"]
            .as_str()
            .or(call["data"].as_str())
            .unwrap_or_default();
        let to = call["to"].as_str().unwrap_or_default();
        let settlement = format!(
            "0x{:0>64}{:0>64}{PAYER:0>64}",
            WALLET.trim_start_matches("0x"),
            "0"
        );
        let result = if to.eq_ignore_ascii_case(&HUMAN_REGISTRY.to_string()) {
            format!("0x{HUMAN_UNTIL_MAX:064x}")
        } else if input.starts_with(&selector(PostageEscrow::settlementOfCall::SELECTOR)) {
            settlement
        } else {
            floor_word()
        };
        Reply::new(
            200,
            json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
        )
    })
    .await
}

fn selector(bytes: [u8; 4]) -> String {
    format!("0x{}", alloy_primitives::hex::encode(bytes))
}

struct Loop {
    db: Arc<TestDb>,
    stubs: [Stub; 3],
    env: Env,
}

impl Loop {
    async fn new() -> Self {
        let db = Arc::new(TestDb::fresh().await);
        create_inbox(&db, HANDLE, DESTINATION, Some(WALLET), NOW)
            .await
            .unwrap();
        let worker = serve_with(|_| Reply::new(200, r#"{"sent":true}"#)).await;
        Self {
            db,
            stubs: [chain().await, model(Some("commercial")).await, worker],
            env: test_env(),
        }
    }

    /// The same walk with the sender proving personhood under mock mode.
    fn in_mock_identity_mode(mut self) -> Self {
        let vars = self
            .env
            .vars()
            .into_iter()
            .chain([("IDENTITY_MODE".to_owned(), "mock".to_owned())]);
        self.env = Env::fixed(vars);
        self
    }

    fn app(&self) -> Router {
        let [chain, model, worker] = &self.stubs;
        router(
            AppState::builder(self.env.clone())
                .clock(clock_at(NOW))
                .db(self.db.clone())
                .chain(Chain::new(Url::parse(&chain.base).unwrap()))
                .classifier(classifier(model))
                .mail_worker(MailWorker::new(
                    reqwest::Client::default(),
                    &worker.base,
                    "secret",
                ))
                .build(),
        )
    }

    fn releases(&self) -> Vec<Value> {
        self.stubs[2]
            .sent()
            .into_iter()
            .map(|sent| sent.body)
            .collect()
    }

    /// Stages 1 and 2 of both walks: an authenticated stranger writing to a
    /// claimed handle is held, and the challenge URL names a real held row.
    /// Returns the token, and the gateway's answer for what later stages
    /// compare against.
    async fn hold_a_strangers_mail(&self) -> (String, Value) {
        // Asserted on the decision itself, since every verdict is a 200.
        let inbound = json!({
            "from": SENDER,
            "to": format!("{HANDLE}@usepostage.com"),
            "subject": "Quick question about the project",
            "body": "Hey, do you have a minute to talk this week?",
            "spf": "pass",
            "dkim": "pass",
            "dmarc": "pass",
        })
        .to_string();
        let held = send(
            self.app(),
            post_with(
                "/api/mail/inbound",
                &inbound,
                &[("x-postage-secret", "secret")],
            ),
        )
        .await;
        assert_eq!(held.body["action"], "hold", "{}", held.text);

        let url = held.body["challenge_url"].as_str().unwrap();
        let token = url.strip_prefix("http://localhost/c/").unwrap().to_owned();
        assert_eq!(held.body["token"], token.as_str());
        let row = challenge_by_token(&self.db, &token).await.unwrap().unwrap();
        assert_eq!(row.handle, HANDLE);
        assert_eq!(row.sender, SENDER.to_lowercase());
        assert!(row.held_until.unwrap_or(0) > 0);
        (token, held.body)
    }

    /// The last stages of both walks: exactly one release reached the worker,
    /// naming this token and the inbox's real destination, and the hold is
    /// spent so it cannot be released twice.
    async fn assert_released_once(&self, token: &str) {
        assert_eq!(
            self.releases(),
            [json!({ "token": token, "to": DESTINATION })]
        );
        let settled = challenge_by_token(&self.db, token).await.unwrap().unwrap();
        assert!(settled.delivered_at.is_some());
        assert_eq!(settled.held_until, None);
    }
}

#[tokio::test]
async fn a_held_mails_challenge_clears_on_the_paid_lane_and_releases_the_message_it_was_holding() {
    let walk = Loop::new().await;
    let (token, held) = walk.hold_a_strangers_mail().await;

    // The page the URL opens reads the challenge as open and held, with the
    // quote the gateway signed.
    let page = send(walk.app(), get(&format!("/api/challenge/{token}"))).await;
    assert_eq!(page.body["state"], "open");
    assert_eq!(page.body["held"], true);
    assert_eq!(page.body["quote"]["signature"], held["quote"]["signature"]);

    // The payment lands and the paid lane clears the gate.
    let resolved = send(
        walk.app(),
        post(
            "/api/challenge/resolve",
            &json!({ "token": token }).to_string(),
        ),
    )
    .await;
    assert_eq!(resolved.status, StatusCode::OK);
    assert_eq!(
        resolved.body,
        json!({ "status": "cleared", "reason": "paid", "delivered": true })
    );

    walk.assert_released_once(&token).await;
    // The payer is remembered for pricing the sender's next message.
    assert_eq!(
        wallet_for_sender(&walk.db, SENDER).await.unwrap(),
        Some(format!("0x{PAYER}"))
    );
}

#[tokio::test]
async fn a_held_mails_challenge_clears_on_the_mock_human_lane_and_releases_the_message_it_was_holding()
 {
    let walk = Loop::new().await.in_mock_identity_mode();
    let (token, _) = walk.hold_a_strangers_mail().await;

    // Mock mode takes no World ID proof at all: the nullifier is derived
    // from the sender address.
    let verified = send(
        walk.app(),
        post("/api/world/verify", &json!({ "token": token }).to_string()),
    )
    .await;
    assert_eq!(verified.status, StatusCode::OK, "{}", verified.text);
    assert_eq!(verified.body["status"], "cleared");
    assert_eq!(verified.body["reason"], "human");
    assert_eq!(verified.body["delivered"], true);
    let identity = verified.body["identity"].as_str().unwrap();
    assert!(
        identity.len() == 42 && identity.starts_with("0x"),
        "{identity}"
    );
    assert!(
        has_live_pass(&walk.db, HANDLE, SENDER, NOW).await.unwrap(),
        "clearing on the human lane grants a live pass"
    );

    walk.assert_released_once(&token).await;
}

//! The product loop walked through its own public entry points, the way the
//! mail worker and a sender's browser drive it (`web/src/app/api/pipeline.test.ts`).
//!
//! What is real: the inbound gateway, the challenge page's read, the
//! resolve/gate wiring and the database. What is stubbed, each a loopback
//! server: the chain, the classifier's model, and the mail worker's
//! `/release` endpoint.
//!
//! The TypeScript clears the challenge on the human lane through
//! `/api/world/verify` under `IDENTITY_MODE=mock`. That route is not ported
//! yet, so this walk clears it on the paid lane through
//! `/api/challenge/resolve` instead: the same gate, the same release, the
//! same settled row. The human-lane walk waits for `/api/world/verify`.

use std::sync::Arc;

use alloy_sol_types::SolCall;
use axum::Router;
use axum::http::StatusCode;
use postage_core::contracts::PostageEscrow;
use reqwest::Url;
use serde_json::{Value, json};

use super::mail_inbound::tests_support::{WALLET, classifier, floor_word, model, test_env};
use crate::app::AppState;
use crate::chain::Chain;
use crate::db::challenges::challenge_by_token;
use crate::db::inboxes::create_inbox;
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

/// A node that tells the two reads this walk makes apart by selector:
/// `effectiveFloor` for the price, `settlementOf` for the payment, which it
/// reports as made by [`PAYER`].
async fn chain() -> Stub {
    serve_with(|sent| {
        let id = sent.body["id"].clone();
        let call = &sent.body["params"][0];
        let input = call["input"]
            .as_str()
            .or(call["data"].as_str())
            .unwrap_or_default();
        let settlement = format!(
            "0x{:0>64}{:0>64}{PAYER:0>64}",
            WALLET.trim_start_matches("0x"),
            "0"
        );
        let result = if input.starts_with(&selector(PostageEscrow::settlementOfCall::SELECTOR)) {
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
}

impl Loop {
    fn app(&self) -> Router {
        let [chain, model, worker] = &self.stubs;
        router(
            AppState::builder(test_env())
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
}

#[tokio::test]
async fn a_held_mails_challenge_clears_on_the_paid_lane_and_releases_the_message_it_was_holding() {
    let db = Arc::new(TestDb::fresh().await);
    create_inbox(&db, HANDLE, DESTINATION, Some(WALLET), NOW)
        .await
        .unwrap();
    let worker = serve_with(|_| Reply::new(200, r#"{"sent":true}"#)).await;
    let walk = Loop {
        db,
        stubs: [chain().await, model(Some("commercial")).await, worker],
    };

    // Stage 1: an authenticated stranger writing to a claimed handle is held,
    // asserted on the decision itself, since every verdict is a 200.
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
        walk.app(),
        post_with(
            "/api/mail/inbound",
            &inbound,
            &[("x-postage-secret", "secret")],
        ),
    )
    .await;
    assert_eq!(held.body["action"], "hold", "{}", held.text);

    // Stage 2: the challenge URL names the same challenge as the wire token,
    // and that token resolves to a real held row.
    let url = held.body["challenge_url"].as_str().unwrap();
    let token = url.strip_prefix("http://localhost/c/").unwrap();
    assert_eq!(held.body["token"], token);
    let row = challenge_by_token(&walk.db, token).await.unwrap().unwrap();
    assert_eq!(row.handle, HANDLE);
    assert_eq!(row.sender, SENDER.to_lowercase());
    assert!(row.held_until.unwrap_or(0) > 0);

    // Stage 3: the page the URL opens reads the challenge as open and held,
    // with the quote the gateway signed.
    let page = send(walk.app(), get(&format!("/api/challenge/{token}"))).await;
    assert_eq!(page.body["state"], "open");
    assert_eq!(page.body["held"], true);
    assert_eq!(
        page.body["quote"]["signature"],
        held.body["quote"]["signature"]
    );

    // Stage 4: the payment lands and the paid lane clears the gate.
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

    // Stage 5: exactly one release reached the worker, naming this token and
    // the inbox's real destination.
    assert_eq!(
        walk.releases(),
        [json!({ "token": token, "to": DESTINATION })]
    );

    // Stage 6: the challenge is delivered and the hold spent, and the payer
    // is remembered for pricing the sender's next message.
    let settled = challenge_by_token(&walk.db, token).await.unwrap().unwrap();
    assert!(settled.delivered_at.is_some());
    assert_eq!(settled.held_until, None);
    assert_eq!(
        wallet_for_sender(&walk.db, SENDER).await.unwrap(),
        Some(format!("0x{PAYER}"))
    );
}

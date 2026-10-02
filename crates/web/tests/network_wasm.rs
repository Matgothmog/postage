//! Mounts the real ledger page over a scripted `fetch`, in a real browser:
//! the loading, loaded, empty and error states, and the timed re-read (it
//! updates the view in place, never stacks requests, and drops an answer a
//! newer read has replaced). Run:
//! `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!  cargo test -p postage-web --target wasm32-unknown-unknown --test network_wasm`

#![cfg(target_arch = "wasm32")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use leptos::prelude::*;
use leptos_router::components::Router;
use postage_web::network_page::Ledger;
use serde_json::{Value, json};
use support::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const PATH: &str = "/api/network";
/// Short enough for a test to wait through a few ticks (the countdown checks
/// every 250 ms).
const QUICK: Duration = Duration::from_millis(400);
const SLOW: Duration = Duration::from_secs(3600);

const CENT: u128 = 10_u128.pow(16);

fn usdc(cents: u128) -> String {
    (cents * CENT).to_string()
}

fn address(byte: &str) -> String {
    format!("0x{}", byte.repeat(20))
}

/// What `GET /api/network` answers for a ledger with something in every part.
fn full_ledger(earned_cents: u128) -> Value {
    json!({
        "earned": usdc(earned_cents),
        "verifiedCount": 7,
        "delivered": 12,
        "sponsored": "34",
        "hasActiveSigner": true,
        "vault": {
            "totalFunded": usdc(1000), "toSponsorship": usdc(700),
            "refilledToRelayer": usdc(25), "fundingEvents": 9,
        },
        "feed": [
            {"kind": "payment", "at": 3, "since": "14m ago", "id": "p1", "tier": "COMMERCIAL",
             "sender": address("ab"), "inbox": address("cd"), "tx": format!("0x{}", "ef".repeat(32)),
             "total": usdc(5)},
            {"kind": "verification", "at": 2, "since": "3h ago", "wallet": address("12")},
        ],
        "senders": [
            {"id": address("11"), "paidCount": 3, "spamRate": "0.1", "stillHuman": true},
            {"id": address("22"), "paidCount": 1, "spamRate": "0.75", "stillHuman": false},
            {"id": address("33"), "paidCount": 8, "spamRate": "0.25", "stillHuman": false},
        ],
        "inboxes": [{"id": address("cd"), "floorPrice": usdc(1), "earned": usdc(40)}],
        "enclaves": [
            {"id": address("a1"), "revoked": false, "measurementPrefix": "0xaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"id": address("a2"), "revoked": true, "measurementPrefix": "0xbbbbbbbbbbbbbbbbbbbbbbbbbb"},
        ],
    })
}

/// Nothing on chain yet: no payments, no inboxes, no vault, no keys.
fn empty_ledger() -> Value {
    json!({
        "earned": "0", "verifiedCount": 0, "delivered": 0, "sponsored": "0",
        "hasActiveSigner": false, "vault": null,
        "feed": [], "senders": [], "inboxes": [], "enclaves": [],
    })
}

fn open_page(refresh_every: Duration, responses: Vec<Value>) -> Mount {
    install_routes(vec![route("GET", PATH, responses)]);
    mount(move || {
        view! {
            <Router>
                <Ledger refresh_every />
            </Router>
        }
    })
}

fn is_open(page: &Mount, selector: &str) -> bool {
    page.query(selector).unwrap().has_attribute("open")
}

#[wasm_bindgen_test]
async fn the_page_says_it_is_reading_until_the_ledger_arrives() {
    quiesce().await;
    let page = open_page(
        SLOW,
        vec![json!({"status": 200, "body": full_ledger(60).to_string(), "gate": "ledger"})],
    );
    page.shows("Reading the chain").await;
    assert!(page.query("[aria-busy='true']").is_some());
    page.assert_absent("Every cent");

    open_gate("ledger");
    page.shows("Every cent,").await;
    page.assert_absent("Reading the chain");
}

#[wasm_bindgen_test]
async fn a_full_ledger_shows_its_figures_feed_and_proof() {
    quiesce().await;
    let page = open_page(SLOW, vec![reply(200, full_ledger(60))]);
    page.shows("Every cent,").await;
    let text = page.text();

    // The four figures across the top, formatted as the Next page did.
    for expected in [
        "$0.60",
        "earned by inboxes",
        "Verified free",
        "World ID, no wallet",
        "Held, then paid",
        "Sponsor pool",
        "$7.00",
        "funds ~34 more",
    ] {
        assert!(text.contains(expected), "{expected:?} missing: {text}");
    }

    // The feed: server-clock times, tiers, short addresses, explorer links.
    for expected in [
        "commercial",
        "0xabab...abab",
        "paid",
        "0xcdcd...cdcd",
        "14m ago",
        "$0.05",
        "verified",
        "proved human",
        "3h ago",
        "free",
    ] {
        assert!(text.contains(expected), "{expected:?} missing: {text}");
    }
    let tx = format!("https://testnet.arcscan.app/tx/0x{}", "ef".repeat(32));
    assert!(page.query(&format!("a[href='{tx}']")).is_some());
    let sender = format!("https://testnet.arcscan.app/address/{}", address("ab"));
    assert!(page.query(&format!("a[href='{sender}']")).is_some());

    // The proof panel: senders' standing, inboxes, vault, keys, contracts.
    for expected in [
        "verified person",
        "75% reported",
        "25% reported",
        "$0.01 floor",
        "$0.40",
        "$10.00",
        "from 9 payments",
        "$0.25",
        "active",
        "revoked",
        "measurement 0xaaaaaaaaaaaaaaaaaaaaaaaaaa…",
        "The live key's measurement matches an ordinary server",
        "PostageEscrow",
        "0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7",
        "EnclaveRegistry",
    ] {
        assert!(text.contains(expected), "{expected:?} missing: {text}");
    }
    assert!(page.query("a[href='/']").is_some());
    assert_eq!(text.matches("% reported").count(), 2);
}

#[wasm_bindgen_test]
async fn a_ledger_with_nothing_on_it_says_so_in_every_part() {
    quiesce().await;
    let page = open_page(SLOW, vec![reply(200, empty_ledger())]);
    page.shows("Nothing yet.").await;

    for expected in [
        "No sender has a history yet.",
        "No inbox has been paid yet.",
        "No signing key is registered.",
        "Nothing can be priced until a key is registered.",
        "from 0 payments",
        "funds ~0 more",
        "$0.00",
    ] {
        assert!(page.text().contains(expected), "{expected:?} missing");
    }
    // The contracts are constants: listed whatever the ledger says.
    assert!(page.text().contains("PostageVault"));
    page.assert_absent("Can't reach the chain.");
}

#[wasm_bindgen_test]
async fn a_refused_read_shows_the_reason_and_try_again_recovers() {
    quiesce().await;
    let page = open_page(
        SLOW,
        vec![
            reply(502, json!({"error": "subgraph timed out"})),
            reply(200, full_ledger(60)),
        ],
    );
    page.shows("Can't reach the chain.").await;
    assert!(page.text().contains("subgraph timed out"));
    page.assert_absent("Every cent");

    page.click("Try again");
    page.shows("Every cent,").await;
    page.assert_absent("Can't reach the chain.");
    assert_eq!(requests_to("GET", PATH).len(), 2);
}

#[wasm_bindgen_test]
async fn an_unreachable_server_and_a_gateway_page_each_get_a_plain_reason() {
    quiesce().await;
    let offline = open_page(SLOW, vec![json!({"throw": "offline"})]);
    offline.shows("Can't reach the chain.").await;
    assert!(offline.text().contains("Could not reach the server"));
    drop(offline);

    quiesce().await;
    let gateway = open_page(SLOW, vec![html_reply(502)]);
    gateway.shows("Can't reach the chain.").await;
    assert!(gateway.text().contains("The ledger could not be read."));
}

#[wasm_bindgen_test]
async fn the_countdown_re_reads_the_ledger_and_updates_it_in_place() {
    quiesce().await;
    let page = open_page(
        QUICK,
        vec![reply(200, full_ledger(60)), reply(200, full_ledger(90))],
    );
    page.shows("$0.60").await;
    assert!(page.text().contains("re-reading in"));

    // The visitor opens the proof panel; a re-read must not close it.
    page.query("details")
        .unwrap()
        .set_attribute("open", "")
        .unwrap();

    page.shows("$0.90").await;
    page.assert_absent("$0.60");
    assert!(is_open(&page, "details"), "the refresh closed the panel");
    assert_eq!(requests_to("GET", PATH).len(), 2);
}

#[wasm_bindgen_test]
async fn a_re_read_that_fails_keeps_the_ledger_and_says_so() {
    quiesce().await;
    let page = open_page(
        QUICK,
        vec![
            reply(200, full_ledger(60)),
            reply(502, json!({"error": "subgraph timed out"})),
            reply(200, full_ledger(90)),
        ],
    );
    page.shows("$0.60").await;
    page.shows("Couldn't re-read just now.").await;
    assert!(page.text().contains("$0.60"), "the ledger went away");
    page.assert_absent("Can't reach the chain.");

    // The next good read clears the note.
    page.shows("$0.90").await;
    page.assert_absent("Couldn't re-read just now.");
}

#[wasm_bindgen_test]
async fn a_tick_is_skipped_while_a_read_is_still_out() {
    quiesce().await;
    let page = open_page(
        QUICK,
        vec![
            reply(200, full_ledger(60)),
            json!({"status": 200, "body": full_ledger(90).to_string(), "gate": "slow"}),
            reply(200, full_ledger(120)),
        ],
    );
    page.shows("$0.60").await;
    eventually("the timed read to go out", || {
        requests_to("GET", PATH).len() == 2
    })
    .await;

    // Several periods pass with that read still out: no further request.
    sleep(1300).await;
    assert_eq!(
        requests_to("GET", PATH).len(),
        2,
        "a read stacked on a read"
    );
    page.assert_absent("$0.90");

    // Once it lands, the next tick reads again.
    open_gate("slow");
    page.shows("$0.90").await;
    page.shows("$1.20").await;
    assert_eq!(requests_to("GET", PATH).len(), 3);
}

#[wasm_bindgen_test]
async fn an_answer_a_newer_read_replaced_is_dropped() {
    quiesce().await;
    let page = open_page(
        QUICK,
        vec![
            reply(502, json!({"error": "subgraph timed out"})),
            // The timed read that goes out while the error is up, held back.
            json!({"status": 200, "body": full_ledger(90).to_string(), "gate": "late"}),
            // "Try again" in the meantime.
            reply(200, full_ledger(60)),
        ],
    );
    page.shows("Can't reach the chain.").await;
    eventually("the timed read to go out", || {
        requests_to("GET", PATH).len() == 2
    })
    .await;

    page.click("Try again");
    page.shows("$0.60").await;

    // The older answer lands now, and must not overwrite the newer one.
    open_gate("late");
    settle().await;
    assert!(page.text().contains("$0.60"), "{}", page.text());
    page.assert_absent("$0.90");
}

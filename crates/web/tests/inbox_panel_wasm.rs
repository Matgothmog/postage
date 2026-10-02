//! Mounts the real `InboxPanel` over a scripted Privy SDK and a scripted
//! chain RPC (`fetch`), in a real browser: the loading, loaded, error and
//! transaction states of the owner's dashboard. Run:
//! `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!  cargo test -p postage-web --target wasm32-unknown-unknown --test inbox_panel_wasm`

#![cfg(target_arch = "wasm32")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use alloy_primitives::{Address, hex};
use alloy_sol_types::SolCall;
use leptos::prelude::*;
use leptos_router::components::Router;
use postage_core::contracts::PostageEscrow;
use postage_web::api::Inbox;
use postage_web::inbox_panel::InboxPanel;
use serde_json::{Value, json};
use support::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const RPC: &str = "https://rpc.testnet.arc.network";
const CENT: u128 = 10_u128.pow(16);

fn word(value: u128) -> Value {
    json!({"status": 200, "body": format!(r#"{{"jsonrpc":"2.0","id":1,"result":"0x{value:064x}"}}"#)})
}

fn selector_hex(selector: [u8; 4]) -> String {
    hex::encode(selector)
}

/// The three reads the panel makes, each scripted with its own responses.
fn chain(earned: Vec<Value>, chosen: Vec<Value>, effective: Vec<Value>) -> Vec<Value> {
    vec![
        route_with_body(
            "POST",
            RPC,
            &selector_hex(PostageEscrow::earningsCall::SELECTOR),
            earned,
        ),
        route_with_body(
            "POST",
            RPC,
            &selector_hex(PostageEscrow::floorPriceCall::SELECTOR),
            chosen,
        ),
        route_with_body(
            "POST",
            RPC,
            &selector_hex(PostageEscrow::effectiveFloorCall::SELECTOR),
            effective,
        ),
    ]
}

fn open(sdk: Value, routes: Vec<Value>) -> Mount {
    clear_storage();
    install_routes(routes);
    let wallet: Address = WALLET.parse().unwrap();
    mount(move || {
        start_privy(sdk);
        view! {
            <Router>
                <InboxPanel
                    inbox=Inbox {
                        handle: "demo".to_owned(),
                        destination: "demo@example.com".to_owned(),
                    }
                    wallet
                />
            </Router>
        }
    })
}

fn sdk() -> Value {
    json!({"snapshot": signed_in(WALLET, Some("t@example.com"), Some("tok"))})
}

fn chip_is_active(page: &Mount, label: &str) -> bool {
    page.button(label)
        .unwrap()
        .class_name()
        .contains("bg-accent")
}

#[wasm_bindgen_test]
async fn while_the_chain_is_being_read_the_numbers_are_dashes_and_cash_out_waits() {
    quiesce().await;
    let held = json!({"gate": "earned", "status": 200,
        "body": format!(r#"{{"result":"0x{:064x}"}}"#, 50 * CENT)});
    let page = open(sdk(), chain(vec![held], vec![word(0)], vec![word(CENT)]));
    page.shows("demo@usepostage.com").await;
    assert_eq!(
        page.query("[data-testid=earned]")
            .unwrap()
            .text_content()
            .unwrap(),
        "—"
    );
    assert!(page.is_disabled("Cash out"));
    assert_eq!(
        page.query("section")
            .unwrap()
            .get_attribute("aria-busy")
            .as_deref(),
        Some("true")
    );

    open_gate("earned");
    page.shows("$0.50").await;
    assert!(!page.is_disabled("Cash out"));
}

#[wasm_bindgen_test]
async fn an_owner_who_never_chose_a_price_sees_the_default_and_no_active_chip() {
    quiesce().await;
    let page = open(sdk(), chain(vec![word(0)], vec![word(0)], vec![word(CENT)]));
    page.shows("Charging $0.01 by default.").await;
    for label in ["1¢", "5¢", "25¢", "$1", "Custom"] {
        assert!(!chip_is_active(&page, label), "{label}");
    }
    assert!(
        page.is_disabled("Cash out"),
        "nothing earned, nothing to cash out"
    );
    assert!(page.query("[aria-label='Custom price']").is_none());
}

#[wasm_bindgen_test]
async fn a_chosen_price_lights_its_chip_and_a_price_off_the_presets_opens_custom() {
    quiesce().await;
    let page = open(
        sdk(),
        chain(vec![word(CENT)], vec![word(5 * CENT)], vec![word(5 * CENT)]),
    );
    eventually("the chosen chip to light", || chip_is_active(&page, "5¢")).await;
    assert!(!chip_is_active(&page, "1¢"));
    drop(page);

    let page = open(
        sdk(),
        chain(vec![word(CENT)], vec![word(7 * CENT)], vec![word(7 * CENT)]),
    );
    eventually("Custom to light", || chip_is_active(&page, "Custom")).await;
    assert_eq!(page.input_value("[aria-label='Custom price']"), "0.07");
}

#[wasm_bindgen_test]
async fn cash_out_sends_claim_earnings_for_the_owner_then_reads_the_chain_again() {
    quiesce().await;
    let page = open(
        sdk(),
        chain(
            vec![word(50 * CENT), word(0)],
            vec![word(0)],
            vec![word(CENT)],
        ),
    );
    page.shows("$0.50").await;
    page.click("Cash out");
    eventually("the earned figure to be read afresh", || {
        page.query("[data-testid=earned]")
            .and_then(|earned| earned.text_content())
            .as_deref()
            == Some("free")
    })
    .await;

    let sent = calls_to("sendTransaction");
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0]["to"].as_str().unwrap().to_lowercase(),
        "0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7"
    );
    assert_eq!(sent[0]["chainId"], 5_042_002);
    let data = sent[0]["data"].as_str().unwrap().to_lowercase();
    assert!(data.starts_with("0x9ab891ba"), "{data}");
    assert!(data.ends_with(&WALLET[2..].to_lowercase()), "{data}");
    assert!(sent[0]["value"].is_null());
}

#[wasm_bindgen_test]
async fn a_preset_chip_sends_set_floor_price_in_whole_base_units() {
    quiesce().await;
    let page = open(
        sdk(),
        chain(
            vec![word(0)],
            vec![word(0), word(25 * CENT)],
            vec![word(CENT), word(25 * CENT)],
        ),
    );
    page.shows("Charging $0.01 by default.").await;
    page.click("25¢");
    eventually("the chip to light", || chip_is_active(&page, "25¢")).await;

    let sent = calls_to("sendTransaction");
    assert_eq!(sent.len(), 1);
    let data = sent[0]["data"].as_str().unwrap().to_lowercase();
    assert!(data.starts_with("0x"), "{data}");
    assert!(data.starts_with(&format!(
        "0x{}",
        selector_hex(PostageEscrow::setFloorPriceCall::SELECTOR)
    )));
    assert!(data.ends_with(&format!("{:064x}", 25 * CENT)), "{data}");
}

#[wasm_bindgen_test]
async fn a_custom_price_that_is_not_a_number_is_a_message_and_sends_nothing() {
    quiesce().await;
    let page = open(sdk(), chain(vec![word(0)], vec![word(0)], vec![word(CENT)]));
    page.shows("Charging $0.01 by default.").await;
    page.click("Custom");
    page.shows("Set").await;
    assert_eq!(page.input_value("[aria-label='Custom price']"), "0.01");
    page.type_into("[aria-label='Custom price']", "free");
    eventually("Set to enable", || !page.is_disabled("Set")).await;
    page.click("Set");
    page.shows("is not a non-negative decimal number").await;
    assert!(calls_to("sendTransaction").is_empty());
    assert!(
        !page.is_disabled("1¢"),
        "buttons come back after the failure"
    );
}

#[wasm_bindgen_test]
async fn a_chain_read_that_fails_says_so_and_try_again_reads_it_afresh() {
    quiesce().await;
    let failing =
        json!({"status": 200, "body": r#"{"error":{"code":-32000,"message":"rpc down"}}"#});
    let page = open(
        sdk(),
        chain(
            vec![failing, word(50 * CENT)],
            vec![word(0)],
            vec![word(CENT)],
        ),
    );
    page.shows("The network refused the read: rpc down").await;
    assert_eq!(
        page.query("[data-testid=earned]")
            .unwrap()
            .text_content()
            .unwrap(),
        "—"
    );
    page.click("Try again");
    page.shows("$0.50").await;
    eventually("the error to clear", || {
        !page.text().contains("The network refused the read")
    })
    .await;
}

#[wasm_bindgen_test]
async fn a_rejected_transaction_shows_the_wallets_words_and_frees_the_buttons() {
    quiesce().await;
    let page = open(
        json!({
            "snapshot": signed_in(WALLET, Some("t@example.com"), Some("tok")),
            "sendError": {"code": 4001, "message": "User rejected the request."},
        }),
        chain(vec![word(50 * CENT)], vec![word(0)], vec![word(CENT)]),
    );
    page.shows("$0.50").await;
    page.click("Cash out");
    page.shows("User rejected the request.").await;
    assert!(!page.is_disabled("Cash out"));
    assert_eq!(
        page.buttons_labelled("Try again"),
        0,
        "a send failure has nothing to re-read"
    );
}

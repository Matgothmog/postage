//! Drives the typed bridge API against `js/src/bridge-core.js` (the adapter the
//! shipped bundle is built from) over a scripted SDK (`tests/mock_sdk.js`), in
//! a real browser. Run with:
//! `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!  cargo test -p postage-web --target wasm32-unknown-unknown --test bridge_wasm`

#![cfg(target_arch = "wasm32")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use alloy_primitives::{Address, B256, U256, address, bytes};
use leptos::prelude::*;
use postage_core::world_id_messages::IdKitErrorCode;
use postage_web::bridge::idkit::{
    IdKitFailure, RpContext, SelfieCheckCompletion, SelfieCheckRequest, open_selfie_check,
};
use postage_web::bridge::privy::{Privy, PrivyConfig, TransactionRequest};
use postage_web::bridge::{self, BridgeError};
use postage_web::config::WorldEnvironment;
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(module = "/js/src/bridge-core.js")]
extern "C" {
    #[wasm_bindgen(js_name = installBridge)]
    fn install_bridge(target: &JsValue, sdk: &JsValue) -> JsValue;
}

#[wasm_bindgen(module = "/tests/mock_sdk.js")]
extern "C" {
    #[wasm_bindgen(js_name = createMockSdk)]
    fn create_mock_sdk(options_json: &str) -> JsValue;
    #[wasm_bindgen(js_name = mockCalls)]
    fn mock_calls() -> String;
}

const WALLET: Address = address!("0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7");

fn install(options: Value) {
    install_bridge(&js_sys::global(), &create_mock_sdk(&options.to_string()));
}

fn uninstall() {
    js_sys::Reflect::delete_property(&js_sys::global(), &"__postageBridge".into()).unwrap();
}

fn calls() -> Vec<Value> {
    serde_json::from_str(&mock_calls()).unwrap()
}

fn call(name: &str) -> Value {
    calls()
        .into_iter()
        .find(|call| call["fn"] == name)
        .unwrap_or_else(|| panic!("no {name} call in {:?}", calls()))
}

/// Lets queued microtasks (the mock's snapshots and login callbacks) run.
async fn tick() {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let set_timeout: js_sys::Function =
            js_sys::Reflect::get(&js_sys::global(), &"setTimeout".into())
                .unwrap()
                .into();
        set_timeout
            .call2(&JsValue::NULL, &resolve, &0.into())
            .unwrap();
    });
    JsFuture::from(promise).await.unwrap();
}

async fn started(options: Value) -> Privy {
    install(options);
    let privy = Privy::start(&PrivyConfig::new("test-app")).unwrap();
    tick().await;
    privy
}

fn rp_context() -> RpContext {
    RpContext {
        rp_id: "rp_1".to_owned(),
        nonce: "n".to_owned(),
        created_at: 1_000,
        expires_at: 1_300,
        signature: "0xsig".to_owned(),
        action: "clear-challenge".to_owned(),
    }
}

#[wasm_bindgen_test]
fn without_the_bridge_script_everything_reports_not_loaded() {
    uninstall();
    assert!(!bridge::is_loaded());
    assert_eq!(
        Privy::start(&PrivyConfig::new("x")).unwrap_err(),
        BridgeError::NotLoaded
    );
}

#[wasm_bindgen_test]
async fn start_mounts_privy_on_arc_and_mirrors_its_state_into_signals() {
    let privy = started(json!({})).await;
    let mount = call("mountPrivy");
    assert_eq!(mount["appId"], "test-app");
    assert_eq!(mount["chainId"], 5_042_002);
    assert!(privy.ready().get_untracked());
    assert!(privy.authenticated().get_untracked());
    assert_eq!(privy.wallet().get_untracked(), Some(WALLET));
    assert_eq!(
        privy.email().get_untracked().as_deref(),
        Some("tester@example.com")
    );
    assert_eq!(
        privy.identity_token().get_untracked().as_deref(),
        Some("id-token-1")
    );
}

#[wasm_bindgen_test]
async fn starting_twice_is_refused() {
    let _privy = started(json!({})).await;
    let error = Privy::start(&PrivyConfig::new("test-app")).unwrap_err();
    assert_eq!(error.code(), Some("already_started"));
}

#[wasm_bindgen_test]
async fn login_resolves_when_privy_reports_completion() {
    let privy = started(json!({})).await;
    privy.login().await.unwrap();
    call("login");
}

#[wasm_bindgen_test]
async fn closing_the_login_modal_is_a_cancel_with_privys_code() {
    let privy = started(json!({ "loginError": "exited_auth_flow" })).await;
    let error = privy.login().await.unwrap_err();
    assert_eq!(error.code(), Some("exited_auth_flow"));
    assert!(error.is_user_cancel());
}

#[wasm_bindgen_test]
async fn calls_before_privy_is_ready_fail_as_not_ready() {
    let privy = started(json!({ "ready": false })).await;
    assert!(!privy.ready().get_untracked());
    assert!(privy.login().await.unwrap_err().is_not_ready());
    assert!(
        privy
            .sign_message("m", WALLET)
            .await
            .unwrap_err()
            .is_not_ready()
    );
}

#[wasm_bindgen_test]
async fn sign_message_targets_the_wallet_with_the_ui_hidden() {
    let privy = started(json!({})).await;
    let signature = privy.sign_message("statement", WALLET).await.unwrap();
    assert_eq!(signature, "0xsigned");
    let sign = call("signMessage");
    assert_eq!(sign["input"]["message"], "statement");
    assert_eq!(sign["options"]["address"], WALLET.to_string());
    assert_eq!(sign["options"]["uiOptions"]["showWalletUIs"], false);
}

#[wasm_bindgen_test]
async fn a_wallet_rejection_keeps_its_eip1193_code() {
    let privy = started(json!({})).await;
    let error = privy.sign_message("reject", WALLET).await.unwrap_err();
    assert_eq!(
        error,
        BridgeError::Sdk {
            code: "4001".to_owned(),
            message: "User rejected request".to_owned()
        }
    );
    assert!(error.is_user_cancel());
}

#[wasm_bindgen_test]
async fn send_transaction_goes_to_arc_with_a_bigint_value_and_returns_the_hash() {
    let privy = started(json!({})).await;
    let tx = TransactionRequest {
        to: WALLET,
        data: bytes!("c0ffee"),
        value: Some(U256::from(12_345_u64)),
    };
    let hash = privy.send_transaction(&tx).await.unwrap();
    assert_eq!(hash, B256::repeat_byte(0xab));
    let sent = call("sendTransaction");
    assert_eq!(sent["chainId"], 5_042_002);
    assert_eq!(sent["data"], "0xc0ffee");
    assert_eq!(sent["valueType"], "bigint");
    assert_eq!(sent["value"], "12345");
}

#[wasm_bindgen_test]
async fn fresh_identity_token_asks_privy_directly() {
    let privy = started(json!({})).await;
    assert_eq!(
        privy.fresh_identity_token().await.unwrap().as_deref(),
        Some("id-token-fresh")
    );
}

fn request<'a>(context: &'a RpContext, signal: &'a str) -> SelfieCheckRequest<'a> {
    SelfieCheckRequest {
        app_id: "app_test",
        environment: WorldEnvironment::Sandbox,
        rp_context: context,
        signal,
    }
}

#[wasm_bindgen_test]
async fn a_selfie_check_opens_with_the_signed_action_and_returns_the_proof() {
    install(json!({}));
    let context = rp_context();
    let handle = open_selfie_check(&request(&context, "token-1"))
        .await
        .unwrap();
    assert_eq!(handle.connector_uri(), "https://world.org/verify?t=mock");
    let opened = call("idkit");
    assert_eq!(opened["config"]["action"], "clear-challenge");
    assert_eq!(opened["config"]["allow_legacy_proofs"], true);
    assert_eq!(opened["config"]["environment"], "sandbox");
    assert_eq!(opened["preset"]["signal"], "token-1");

    let completion = handle
        .poll_until_completion(context.poll_timeout_ms())
        .await;
    assert_eq!(
        completion,
        SelfieCheckCompletion::Verified(json!({"protocol_version": "3.0", "nullifier": "0x01"}))
    );
    assert_eq!(call("poll")["timeout"], 270_000);
}

#[wasm_bindgen_test]
async fn a_rejected_selfie_check_carries_idkits_code() {
    install(json!({}));
    let context = rp_context();
    let handle = open_selfie_check(&request(&context, "rejected"))
        .await
        .unwrap();
    assert_eq!(
        handle.poll_until_completion(1_000).await,
        SelfieCheckCompletion::Failed(IdKitFailure::Known(IdKitErrorCode::UserRejected))
    );
}

#[wasm_bindgen_test]
async fn a_poll_that_throws_is_a_generic_failure() {
    install(json!({}));
    let context = rp_context();
    let handle = open_selfie_check(&request(&context, "poll-throws"))
        .await
        .unwrap();
    assert_eq!(
        handle.poll_until_completion(1_000).await,
        SelfieCheckCompletion::Failed(IdKitFailure::Known(IdKitErrorCode::GenericError))
    );
}

#[wasm_bindgen_test]
async fn idkit_refusing_to_open_is_an_sdk_error() {
    install(json!({}));
    let context = rp_context();
    let error = open_selfie_check(&request(&context, "throw"))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        BridgeError::Sdk {
            code: "request_failed".to_owned(),
            message: "IDKit could not start".to_owned()
        }
    );
}

#[wasm_bindgen_test]
async fn an_unbound_selfie_check_never_reaches_idkit() {
    install(json!({}));
    let context = rp_context();
    let error = open_selfie_check(&request(&context, "")).await.unwrap_err();
    assert!(matches!(error, BridgeError::InvalidInput(_)));
    assert!(calls().is_empty());
}

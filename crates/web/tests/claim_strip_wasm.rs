//! Mounts the real `ClaimStrip` over a scripted Privy SDK and a scripted
//! `fetch`, in a real browser. Re-expresses the behaviours the source-regex
//! tests in `web/src/app/ClaimStrip.test.ts` pinned. Run:
//! `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!  cargo test -p postage-web --target wasm32-unknown-unknown --test claim_strip_wasm`

#![cfg(target_arch = "wasm32")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alloy_primitives::Address;
use leptos::prelude::*;
use postage_web::claim_strip::ClaimStrip;
use postage_web::pending_claim::ClaimProgress;
use serde_json::{Value, json};
use support::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const VERIFY: &str = "/api/inbox/verify";

/// What the strip told its owner.
struct Probe {
    claim: RwSignal<ClaimProgress>,
    advanced: Arc<Mutex<Vec<ClaimProgress>>>,
    lives: Arc<AtomicU32>,
    restarts: Arc<AtomicU32>,
}

fn claim(handle: &str, code_verified: bool, cloudflare_verified: bool) -> ClaimProgress {
    ClaimProgress {
        handle: handle.to_owned(),
        destination: "demo@example.com".to_owned(),
        code_verified,
        cloudflare_verified,
    }
}

fn poll_url(handle: &str) -> String {
    format!("{VERIFY}?handle={handle}")
}

fn quiet_poll() -> Value {
    reply(
        200,
        json!({"codeVerified": false, "cloudflareVerified": false, "live": false, "stalled": false}),
    )
}

/// Mounts a strip. `poll_ms` is how often it asks the server; tests that are
/// not about polling pass something long so it stays out of the log.
fn mount_strip(
    sdk: Value,
    routes: Vec<Value>,
    initial: ClaimProgress,
    poll_ms: u64,
) -> (Mount, Probe) {
    clear_storage();
    install_routes(routes);
    let probe = Probe {
        claim: RwSignal::new(initial),
        advanced: Arc::default(),
        lives: Arc::default(),
        restarts: Arc::default(),
    };
    let (claim_signal, advanced, lives, restarts) = (
        probe.claim,
        Arc::clone(&probe.advanced),
        Arc::clone(&probe.lives),
        Arc::clone(&probe.restarts),
    );
    let wallet_address: Address = WALLET.parse().unwrap();
    let page = mount(move || {
        start_privy(sdk);
        view! {
            <ClaimStrip
                claim=claim_signal
                wallet=wallet_address
                on_claim=Callback::new(move |next: ClaimProgress| {
                    advanced.lock().unwrap().push(next.clone());
                    claim_signal.set(next);
                })
                on_live=Callback::new(move |()| {
                    lives.fetch_add(1, Ordering::SeqCst);
                })
                on_restart=Callback::new(move |()| {
                    restarts.fetch_add(1, Ordering::SeqCst);
                })
                poll_interval=Duration::from_millis(poll_ms)
            />
        }
    });
    (page, probe)
}

fn with_token() -> Value {
    json!({"snapshot": signed_in(WALLET, Some("t@example.com"), Some("tok"))})
}

const SLOW: u64 = 600_000;

// --- the way out (H2) ------------------------------------------------------

#[wasm_bindgen_test]
async fn the_strips_way_out_is_one_control_wired_to_restart() {
    quiesce().await;
    let (page, probe) = mount_strip(
        with_token(),
        vec![route("GET", &poll_url("demo"), vec![quiet_poll()])],
        claim("demo", false, false),
        SLOW,
    );
    page.shows("One click left").await;
    assert_eq!(page.buttons_labelled("Start over"), 1);
    page.click("Start over");
    assert_eq!(probe.restarts.load(Ordering::SeqCst), 1);
}

#[wasm_bindgen_test]
async fn the_way_out_is_rendered_whatever_state_the_claim_is_in() {
    quiesce().await;
    for state in [(false, false), (true, false), (true, true)] {
        let (page, _probe) = mount_strip(
            with_token(),
            vec![route("GET", &poll_url("demo"), vec![quiet_poll()])],
            claim("demo", state.0, state.1),
            SLOW,
        );
        page.shows("One click left").await;
        assert_eq!(page.buttons_labelled("Start over"), 1, "{state:?}");
    }

    // A claim the server has stopped watching.
    let (page, _probe) = mount_strip(
        with_token(),
        vec![route(
            "GET",
            &poll_url("demo"),
            vec![reply(
                200,
                json!({"codeVerified": true, "cloudflareVerified": false, "live": false, "stalled": true}),
            )],
        )],
        claim("demo", true, false),
        20,
    );
    page.shows("We stopped watching.").await;
    assert_eq!(page.buttons_labelled("Start over"), 1);
}

// --- the poll --------------------------------------------------------------

#[wasm_bindgen_test]
async fn the_poll_builds_its_url_with_the_shared_encoder_not_by_interpolating_the_handle() {
    quiesce().await;
    let (_page, _probe) = mount_strip(
        with_token(),
        vec![route(
            "GET",
            "/api/inbox/verify?handle=a%26b",
            vec![quiet_poll()],
        )],
        claim("a&b", false, false),
        20,
    );
    eventually("the encoded poll", || {
        !requests_to("GET", "/api/inbox/verify?handle=a%26b").is_empty()
    })
    .await;
    assert!(
        requests()
            .iter()
            .all(|request| request["unmatched"].is_null())
    );
}

#[wasm_bindgen_test]
async fn a_404_from_the_poll_restarts_the_claim_the_server_has_dropped() {
    quiesce().await;
    let (_page, probe) = mount_strip(
        with_token(),
        vec![route(
            "GET",
            &poll_url("demo"),
            vec![reply(404, json!({"error": "gone"}))],
        )],
        claim("demo", false, false),
        20,
    );
    eventually("a restart", || probe.restarts.load(Ordering::SeqCst) >= 1).await;
}

#[wasm_bindgen_test]
async fn a_fault_the_poll_cannot_read_leaves_the_claim_alone_and_keeps_polling() {
    quiesce().await;
    let (page, probe) = mount_strip(
        with_token(),
        vec![route(
            "GET",
            &poll_url("demo"),
            vec![reply(503, json!({})), html_reply(502)],
        )],
        claim("demo", false, false),
        20,
    );
    eventually("several polls", || {
        requests_to("GET", &poll_url("demo")).len() >= 3
    })
    .await;
    assert_eq!(probe.restarts.load(Ordering::SeqCst), 0);
    assert!(probe.advanced.lock().unwrap().is_empty());
    assert!(page.text().contains("One click left"));
}

#[wasm_bindgen_test]
async fn a_poll_answer_merges_the_confirmed_step_and_reports_only_a_step_that_moved() {
    quiesce().await;
    let (_page, probe) = mount_strip(
        with_token(),
        vec![route(
            "GET",
            &poll_url("demo"),
            vec![reply(
                200,
                json!({"codeVerified": true, "cloudflareVerified": false, "live": false}),
            )],
        )],
        claim("demo", false, false),
        20,
    );
    eventually("the step to be reported", || {
        !probe.advanced.lock().unwrap().is_empty()
    })
    .await;
    settle().await;
    let advanced = probe.advanced.lock().unwrap();
    assert_eq!(
        advanced.len(),
        1,
        "an unchanged answer must not be written back"
    );
    assert_eq!(advanced[0], claim("demo", true, false));
}

#[wasm_bindgen_test]
async fn a_poll_that_says_live_hands_over_to_the_inbox() {
    quiesce().await;
    let (_page, probe) = mount_strip(
        with_token(),
        vec![route(
            "GET",
            &poll_url("demo"),
            vec![reply(
                200,
                json!({"codeVerified": true, "cloudflareVerified": true, "live": true}),
            )],
        )],
        claim("demo", true, false),
        20,
    );
    eventually("live", || probe.lives.load(Ordering::SeqCst) >= 1).await;
}

#[wasm_bindgen_test]
async fn once_cloudflare_has_confirmed_the_strip_stops_polling() {
    quiesce().await;
    let (_page, _probe) = mount_strip(
        with_token(),
        vec![route("GET", &poll_url("demo"), vec![quiet_poll()])],
        claim("demo", true, true),
        20,
    );
    settle().await;
    assert!(requests_to("GET", &poll_url("demo")).is_empty());
}

// --- the code field --------------------------------------------------------

fn confirm_route(responses: Vec<Value>) -> Value {
    route("POST", VERIFY, responses)
}

#[wasm_bindgen_test]
async fn the_sixth_digit_sends_the_code_once_with_the_identity_token() {
    quiesce().await;
    let (page, probe) = mount_strip(
        with_token(),
        vec![
            route("GET", &poll_url("demo"), vec![quiet_poll()]),
            confirm_route(vec![reply(
                200,
                json!({"codeVerified": true, "cloudflareVerified": false, "live": false}),
            )]),
        ],
        claim("demo", false, false),
        SLOW,
    );
    page.type_into("#code", "12345");
    settle().await;
    assert!(
        requests_to("POST", VERIFY).is_empty(),
        "five digits send nothing"
    );

    page.type_into("#code", "123456");
    eventually("the code to be sent", || {
        !requests_to("POST", VERIFY).is_empty()
    })
    .await;
    let sent = requests_to("POST", VERIFY);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        request_body(&sent[0]),
        json!({"handle": "demo", "code": "123456"})
    );
    assert_eq!(sent[0]["headers"]["privy-id-token"], "tok");
    eventually("the step to be reported", || {
        !probe.advanced.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(
        probe.advanced.lock().unwrap()[0],
        claim("demo", true, false)
    );
}

#[wasm_bindgen_test]
async fn typing_keeps_digits_only_and_a_seventh_keystroke_does_not_resend() {
    quiesce().await;
    let (page, _probe) = mount_strip(
        with_token(),
        vec![
            route("GET", &poll_url("demo"), vec![quiet_poll()]),
            confirm_route(vec![reply(400, json!({"error": "That code is wrong"}))]),
        ],
        claim("demo", false, false),
        SLOW,
    );
    page.type_into("#code", "12a3");
    assert_eq!(page.input_value("#code"), "123");

    page.type_into("#code", "123456");
    page.shows("That code is wrong").await;
    page.type_into("#code", "1234567");
    assert_eq!(page.input_value("#code"), "123456");
    settle().await;
    assert_eq!(requests_to("POST", VERIFY).len(), 1);
}

#[wasm_bindgen_test]
async fn after_a_wrong_code_the_field_stops_sending_itself_and_a_confirm_button_appears() {
    quiesce().await;
    let (page, _probe) = mount_strip(
        with_token(),
        vec![
            route("GET", &poll_url("demo"), vec![quiet_poll()]),
            confirm_route(vec![
                reply(400, json!({"error": "That code is wrong"})),
                reply(
                    200,
                    json!({"codeVerified": true, "cloudflareVerified": false, "live": false}),
                ),
            ]),
        ],
        claim("demo", false, false),
        SLOW,
    );
    assert!(
        page.button("Confirm").is_none(),
        "no button until an attempt fails"
    );
    page.type_into("#code", "111111");
    page.shows("That code is wrong").await;

    page.type_into("#code", "222222");
    settle().await;
    assert_eq!(
        requests_to("POST", VERIFY).len(),
        1,
        "no second automatic send"
    );

    eventually("the Confirm button", || {
        page.button("Confirm")
            .is_some_and(|b| !b.has_attribute("disabled"))
    })
    .await;
    page.click("Confirm");
    eventually("the second attempt", || {
        requests_to("POST", VERIFY).len() == 2
    })
    .await;
    assert_eq!(
        request_body(&requests_to("POST", VERIFY)[1])["code"],
        "222222"
    );
}

#[wasm_bindgen_test]
async fn without_an_identity_token_the_code_is_sent_with_a_signed_confirmation() {
    quiesce().await;
    let (page, _probe) = mount_strip(
        json!({"snapshot": signed_in(WALLET, None, None), "signature": "0xsig"}),
        vec![
            route("GET", &poll_url("demo"), vec![quiet_poll()]),
            route(
                "POST",
                "/api/wallet-nonce",
                vec![reply(200, json!({"nonce": "n7"}))],
            ),
            confirm_route(vec![reply(
                200,
                json!({"codeVerified": true, "cloudflareVerified": false, "live": false}),
            )]),
        ],
        claim("demo", false, false),
        SLOW,
    );
    page.type_into("#code", "123456");
    eventually("the code to be sent", || {
        !requests_to("POST", VERIFY).is_empty()
    })
    .await;
    let headers = &requests_to("POST", VERIFY)[0]["headers"];
    assert_eq!(headers["x-postage-wallet"], WALLET);
    assert_eq!(headers["x-postage-signature"], "0xsig");
    assert_eq!(headers["x-postage-nonce"], "n7");
    assert!(headers["x-postage-issued"].is_string());
    assert!(headers["privy-id-token"].is_null());
    let signed = calls_to("signMessage");
    let message = signed[0]["input"]["message"].as_str().unwrap();
    assert!(message.starts_with("Postage: confirm my code\nHandle: demo@usepostage.com"));
    assert!(message.ends_with("Nonce: n7"));
}

#[wasm_bindgen_test]
async fn a_code_that_cannot_be_signed_for_is_an_error_not_a_send() {
    quiesce().await;
    let (page, _probe) = mount_strip(
        json!({"snapshot": signed_in(WALLET, None, None)}),
        vec![
            route("GET", &poll_url("demo"), vec![quiet_poll()]),
            route("POST", "/api/wallet-nonce", vec![reply(503, json!({}))]),
        ],
        claim("demo", false, false),
        SLOW,
    );
    page.type_into("#code", "123456");
    page.shows("Could not start a signature.").await;
    assert!(requests_to("POST", VERIFY).is_empty());
}

// --- what the strip says ---------------------------------------------------

#[wasm_bindgen_test]
async fn the_strip_says_what_it_is_waiting_for_in_each_stage() {
    quiesce().await;
    let (page, _probe) = mount_strip(
        with_token(),
        vec![route("GET", &poll_url("demo"), vec![quiet_poll()])],
        claim("demo", false, false),
        SLOW,
    );
    page.shows("We emailed a code to").await;
    assert!(page.query("#code").is_some());

    let (waiting, _probe) = mount_strip(
        with_token(),
        vec![route("GET", &poll_url("demo"), vec![quiet_poll()])],
        claim("demo", true, false),
        SLOW,
    );
    waiting.shows("Waiting on Cloudflare. Check spam.").await;
    assert!(waiting.query("#code").is_none());
    assert!(waiting.text().contains("Pending"));
}

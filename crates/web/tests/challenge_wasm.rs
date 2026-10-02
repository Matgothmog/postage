//! Mounts the real challenge page and its actions over a scripted Privy and
//! IDKit SDK and a scripted `fetch`, in a real browser: every state the page
//! can be in, the three ways through it (prove a person, pay, paste the
//! message again) and the loading, empty and error state of each async step.
//! Run:
//! `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!  cargo test -p postage-web --target wasm32-unknown-unknown --test challenge_wasm`

#![cfg(target_arch = "wasm32")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use alloy_primitives::{Address, B256, hex};
use alloy_sol_types::SolCall;
use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::hooks::use_navigate;
use leptos_router::path;
use postage_core::contracts::{POSTAGE_ESCROW, PostageEscrow};
use postage_core::quote_types::{QuoteFields, StoredQuote};
use postage_web::app::AppRoutes;
use postage_web::challenge_actions::{ChallengeActions, Lane, Settlement};
use postage_web::challenge_api::{IdentityMode, OpenChallenge};
use postage_web::challenge_page::ChallengePage;
use postage_web::config::WorldEnvironment;
use postage_web::screens::NotFound;
use postage_web::world_id::WorldApp;
use serde_json::{Value, json};
use support::*;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

/// Five cents, in the token's 18 decimals.
const FIVE_CENTS: u128 = 50_000_000_000_000_000;
const TX_HASH: &str = "0xabababababababababababababababababababababababababababababababab";
const CONNECTOR: &str = "https://world.org/verify?t=mock";

fn go_to(path: &str) {
    leptos::tachys::dom::window()
        .history()
        .unwrap()
        .push_state_with_url(&JsValue::NULL, "", Some(path))
        .unwrap();
}

fn current_url() -> String {
    leptos::tachys::dom::window().location().href().unwrap()
}

fn quote_fields() -> QuoteFields {
    QuoteFields {
        message_id: format!("0x{}", "ab".repeat(32)),
        inbox: format!("0x{}", "cd".repeat(20)),
        tier: "commercial".to_owned(),
        amount: FIVE_CENTS.to_string(),
        expires_at: 1_800_000_000,
        signature: format!("0x{}", "ef".repeat(65)),
    }
}

fn challenge(dangerous: bool, held: bool, mode: IdentityMode) -> OpenChallenge {
    OpenChallenge {
        handle: "demo".to_owned(),
        dangerous,
        held,
        quote: StoredQuote {
            quote: quote_fields(),
            reasons: vec!["looked automated".to_owned(), "no prior pass".to_owned()],
        },
        amount: FIVE_CENTS,
        identity_mode: mode,
    }
}

/// What `GET /api/challenge/{token}` answers for an open challenge.
fn open_view(handle: &str, dangerous: bool, held: bool, mode: &str) -> Value {
    json!({
        "state": "open", "handle": handle, "dangerous": dangerous, "held": held,
        "quote": {
            "messageId": format!("0x{}", "ab".repeat(32)),
            "inbox": format!("0x{}", "cd".repeat(20)),
            "tier": "commercial",
            "amount": FIVE_CENTS.to_string(),
            "expiresAt": 1_800_000_000u64,
            "signature": format!("0x{}", "ef".repeat(65)),
            "reasons": ["looked automated", "no prior pass"],
        },
        "identityMode": mode,
    })
}

fn world() -> WorldApp {
    WorldApp {
        app_id: Some("app_test"),
        environment: WorldEnvironment::Sandbox,
    }
}

fn quick() -> Settlement {
    Settlement {
        attempts: 2,
        interval: Duration::from_millis(15),
    }
}

fn rp_context() -> Value {
    json!({
        "rp_id": "rp_test", "nonce": "ctx-nonce", "created_at": 1000, "expires_at": 1300,
        "signature": "0xsig", "action": "ctx-action",
    })
}

fn context_route() -> Value {
    route("POST", "/api/world/context", vec![reply(200, rp_context())])
}

fn verify_route(delivered: bool) -> Value {
    route(
        "POST",
        "/api/world/verify",
        vec![reply(
            200,
            json!({"status": "cleared", "delivered": delivered}),
        )],
    )
}

fn signed_in_sdk() -> Value {
    json!({"snapshot": signed_in(WALLET, Some("t@example.com"), Some("tok"))})
}

/// The actions alone, over `routes`, in a signed-out world unless `sdk` says
/// otherwise.
fn open_actions(
    sdk: Value,
    routes: Vec<Value>,
    challenge: OpenChallenge,
    token: &'static str,
    lane: Lane,
) -> Mount {
    clear_storage();
    install_routes(routes);
    mount(move || {
        start_privy(sdk);
        view! {
            <Router>
                <ChallengeActions token challenge lane settlement=quick() world=world() />
            </Router>
        }
    })
}

fn human_only(routes: Vec<Value>, token: &'static str, mode: IdentityMode) -> Mount {
    open_actions(
        json!({"snapshot": signed_out()}),
        routes,
        challenge(false, true, mode),
        token,
        Lane::Choosing,
    )
}

/// The page route over a signed-in wallet, at `path`.
fn open_page(path: &str, sdk: Value, routes: Vec<Value>) -> Mount {
    go_to(path);
    clear_storage();
    install_routes(routes);
    mount(move || {
        start_privy(sdk);
        view! { <AppRoutes /> }
    })
}

fn view_route(token: &str, responses: Vec<Value>) -> Value {
    route("GET", &format!("/api/challenge/{token}"), responses)
}

fn resolve_route(responses: Vec<Value>) -> Value {
    route("POST", "/api/challenge/resolve", responses)
}

/// Waits for the automatic polling to give up and hand the checking to the
/// sender.
async fn window_ran_out(page: &Mount) {
    eventually("the window to run out", || {
        page.button("Check again")
            .is_some_and(|button| !button.has_attribute("disabled"))
    })
    .await;
}

fn pending() -> Value {
    reply(200, json!({"status": "pending"}))
}

fn cleared(delivered: bool) -> Value {
    reply(
        200,
        json!({"status": "cleared", "reason": "paid", "delivered": delivered}),
    )
}

// ---------------------------------------------------------------------------
// The page's states.
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
async fn an_open_held_challenge_shows_why_the_price_and_both_ways_through() {
    quiesce().await;
    let page = open_page(
        "/c/tok",
        signed_in_sdk(),
        vec![view_route(
            "tok",
            vec![reply(200, open_view("demo", false, true, "mock"))],
        )],
    );

    page.shows("Held at the door.").await;
    let text = page.text();
    assert!(text.contains("HELD"), "{text}");
    assert!(text.contains("Your mail to demo@usepostage.com is safe. One click sends it."));
    assert!(
        text.contains("Why") && text.contains("looked automated") && text.contains("no prior pass")
    );
    assert!(text.contains("Humans go free. Machines pay $0.05 — to them, not us."));
    assert!(page.button("I'm human — free").is_some());
    assert!(page.button("I'm a bot — pay $0.05").is_some());
    assert!(text.contains("World ID. No wallet, no account."));
    assert!(text.contains("Goes to them, not us."));
    assert!(text.contains("Someone just got paid for this."));
    assert!(page.query("a[href='/']").is_some());
    assert_eq!(requests_to("GET", "/api/challenge/tok").len(), 1);
}

#[wasm_bindgen_test]
async fn a_bounced_challenge_says_so() {
    quiesce().await;
    let page = open_page(
        "/c/tok",
        signed_in_sdk(),
        vec![view_route(
            "tok",
            vec![reply(200, open_view("demo", false, false, "mock"))],
        )],
    );

    page.shows("BOUNCED").await;
    assert!(
        page.text()
            .contains("That one bounced. Clear this and the next goes straight through.")
    );
    page.assert_absent("Your mail to");
}

#[wasm_bindgen_test]
async fn a_dangerous_challenge_offers_no_way_to_pay() {
    quiesce().await;
    let page = open_page(
        "/c/tok",
        signed_in_sdk(),
        vec![view_route(
            "tok",
            vec![reply(200, open_view("demo", true, true, "mock"))],
        )],
    );

    page.shows("Blocked. For good.").await;
    assert!(
        page.text()
            .contains("Reads as an attempt to deceive. Money won't fix that.")
    );
    page.assert_absent("Humans go free");
    page.assert_absent("pay $");
    assert!(page.button("I'm human — free").is_some());
}

#[wasm_bindgen_test]
async fn a_settled_challenge_says_it_is_dealt_with() {
    quiesce().await;
    let page = open_page(
        "/c/tok",
        signed_in_sdk(),
        vec![view_route(
            "tok",
            vec![reply(200, json!({"state": "resolved"}))],
        )],
    );

    page.shows("Done.").await;
    assert!(
        page.text()
            .contains("That one's dealt with. A pass lasts 15 minutes.")
    );
    assert!(page.text().contains("Get paid too"));
    assert!(page.button("I'm human — free").is_none());
}

#[wasm_bindgen_test]
async fn a_challenge_whose_quote_no_longer_reads_is_a_dead_link() {
    quiesce().await;
    let page = open_page(
        "/c/tok",
        signed_in_sdk(),
        vec![view_route(
            "tok",
            vec![reply(200, json!({"state": "dead"}))],
        )],
    );

    page.shows("Dead link.").await;
    assert!(page.text().contains("Write again for a fresh one."));
    assert!(page.text().contains("Get paid too"));
}

#[wasm_bindgen_test]
async fn an_unknown_token_is_the_not_found_screen() {
    quiesce().await;
    let page = open_page(
        "/c/nope",
        signed_in_sdk(),
        vec![view_route(
            "nope",
            vec![reply(404, json!({"error": "Unknown challenge"}))],
        )],
    );

    page.shows("Nothing here.").await;
    page.assert_absent("Held at the door.");
}

#[wasm_bindgen_test]
async fn while_the_challenge_is_being_read_the_page_says_loading_and_offers_nothing() {
    quiesce().await;
    let held = json!({"gate": "view", "status": 200,
        "body": open_view("demo", false, true, "mock").to_string()});
    let page = open_page(
        "/c/tok",
        signed_in_sdk(),
        vec![view_route("tok", vec![held])],
    );

    page.shows("Loading").await;
    assert!(page.button("I'm human — free").is_none());
    assert!(
        page.query("a[href='/']").is_some(),
        "the header is already there"
    );

    open_gate("view");
    page.shows("Held at the door.").await;
    page.assert_absent("Loading");
}

#[wasm_bindgen_test]
async fn a_failed_read_is_an_error_screen_whose_retry_reads_again() {
    quiesce().await;
    let page = open_page(
        "/c/tok",
        signed_in_sdk(),
        vec![view_route(
            "tok",
            vec![
                json!({"throw": "offline"}),
                reply(500, json!({"error": "boom"})),
                reply(200, open_view("demo", false, true, "mock")),
            ],
        )],
    );

    page.shows("Lost in transit.").await;
    page.assert_absent("Nothing here.");

    page.click("Try again");
    eventually("the second read", || {
        requests_to("GET", "/api/challenge/tok").len() == 2
    })
    .await;
    page.shows("Lost in transit.").await;
    page.assert_absent("Nothing here.");

    page.click("Try again");
    page.shows("Held at the door.").await;
    assert_eq!(
        requests_to("GET", "/api/challenge/tok").len(),
        3,
        "a 500 is a fault, not a verdict"
    );
}

#[wasm_bindgen_test]
async fn an_unreadable_answer_is_an_error_not_a_dead_link() {
    quiesce().await;
    let page = open_page(
        "/c/tok",
        signed_in_sdk(),
        vec![view_route("tok", vec![html_reply(200)])],
    );

    page.shows("Lost in transit.").await;
    page.assert_absent("Dead link.");
}

#[wasm_bindgen_test]
async fn the_as_bot_link_lands_on_the_pay_lane_and_as_human_on_the_human_one() {
    quiesce().await;
    let routes = || {
        vec![view_route(
            "tok",
            vec![reply(200, open_view("demo", false, true, "mock"))],
        )]
    };

    let bot = open_page("/c/tok?as=bot", signed_in_sdk(), routes());
    bot.shows("Pay $0.05").await;
    assert!(
        bot.button("I'm human — free").is_none(),
        "the choice is collapsed"
    );
    assert!(bot.button("I'm human").is_some(), "the way back stays");
    drop(bot);

    let human = open_page("/c/tok?as=human", signed_in_sdk(), routes());
    human.shows("I'm human — free").await;
    assert!(human.button("I'm a bot — pay $0.05").is_none());
    assert!(human.button("Pay $0.05 instead").is_some());
}

#[wasm_bindgen_test]
async fn the_identity_mode_the_server_sends_decides_what_the_human_button_does() {
    quiesce().await;
    let live = open_page(
        "/c/tok",
        signed_in_sdk(),
        vec![
            view_route(
                "tok",
                vec![reply(200, open_view("demo", false, true, "live"))],
            ),
            context_route(),
            verify_route(true),
        ],
    );
    live.shows("Held at the door.").await;
    live.click("I'm human — free");
    // This build has no World app id, so the live branch stops at its config
    // check, which a bare-token post to /api/world/verify never would.
    live.shows("World ID isn't configured yet.").await;
    assert!(requests_to("POST", "/api/world/verify").is_empty());
    drop(live);

    let mock = open_page(
        "/c/tok",
        signed_in_sdk(),
        vec![
            view_route(
                "tok",
                vec![reply(200, open_view("demo", false, true, "mock"))],
            ),
            verify_route(true),
        ],
    );
    mock.shows("Held at the door.").await;
    mock.click("I'm human — free");
    mock.shows("Sent.").await;
    assert!(
        requests_to("POST", "/api/world/context").is_empty(),
        "mock posts straight away"
    );
}

#[wasm_bindgen_test]
async fn an_answer_for_a_token_the_page_has_left_is_not_shown() {
    quiesce().await;

    #[component]
    fn Jump() -> impl IntoView {
        let navigate = use_navigate();
        view! {
            <button type="button" on:click=move |_| navigate("/c/b", Default::default())>
                "jump"
            </button>
        }
    }

    go_to("/c/a");
    clear_storage();
    let slow = json!({"gate": "slow-a", "status": 200,
        "body": open_view("alpha", false, true, "mock").to_string()});
    install_routes(vec![
        view_route("a", vec![slow]),
        view_route(
            "b",
            vec![reply(200, open_view("bravo", false, true, "mock"))],
        ),
    ]);
    let page = mount(|| {
        start_privy(signed_in_sdk());
        view! {
            <Router>
                <Jump />
                <Routes fallback=NotFound>
                    <Route path=path!("/c/:token") view=ChallengePage />
                </Routes>
            </Router>
        }
    });
    page.shows("Loading").await;

    page.click("jump");
    page.shows("bravo@usepostage.com").await;
    open_gate("slow-a");
    settle().await;

    page.assert_absent("alpha@usepostage.com");
    assert!(page.text().contains("bravo@usepostage.com"));
}

#[wasm_bindgen_test]
async fn a_token_with_awkward_characters_is_escaped_in_the_request_path() {
    quiesce().await;
    let page = open_page(
        "/c/a%20b%2Fc",
        signed_in_sdk(),
        vec![view_route(
            "a%20b%2Fc",
            vec![reply(200, json!({"state": "dead"}))],
        )],
    );

    page.shows("Dead link.").await;
    assert_eq!(requests_to("GET", "/api/challenge/a%20b%2Fc").len(), 1);
}

// ---------------------------------------------------------------------------
// "I'm human": World ID, no wallet.
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
async fn under_mock_identity_the_human_button_posts_a_bare_token_and_a_delivered_message_is_sent() {
    quiesce().await;
    let page = human_only(vec![verify_route(true)], "tok", IdentityMode::Mock);

    page.click("I'm human — free");

    page.shows("Sent.").await;
    assert!(
        page.text()
            .contains("Same words, same sender. Already in their inbox.")
    );
    let posts = requests_to("POST", "/api/world/verify");
    assert_eq!(posts.len(), 1);
    assert_eq!(request_body(&posts[0]), json!({"token": "tok"}));
    assert!(requests_to("POST", "/api/world/context").is_empty());
    assert!(
        calls_to("idkit").is_empty(),
        "no World App on the mock path"
    );
    assert!(
        calls_to("login").is_empty() && calls_to("sendTransaction").is_empty(),
        "no wallet"
    );
}

#[wasm_bindgen_test]
async fn a_cleared_human_whose_hold_ran_out_is_asked_to_paste_the_message_again() {
    quiesce().await;
    let page = human_only(vec![verify_route(false)], "tok", IdentityMode::Mock);

    page.click("I'm human — free");

    page.shows("Cleared.").await;
    assert!(page.text().contains("The hold expired. Paste it again."));
    assert!(page.query("textarea").is_some());
}

#[wasm_bindgen_test]
async fn under_live_identity_the_selfie_check_runs_first_and_its_proof_is_forwarded() {
    quiesce().await;
    let page = human_only(
        vec![context_route(), verify_route(true)],
        "tok",
        IdentityMode::Live,
    );

    page.click("I'm human — free");

    page.shows("Sent.").await;
    let context = requests_to("POST", "/api/world/context");
    assert_eq!(request_body(&context[0]), json!({"token": "tok"}));

    let opened = &calls_to("idkit")[0];
    assert_eq!(
        opened["preset"]["signal"], "tok",
        "the proof is bound to this challenge"
    );
    assert_eq!(opened["config"]["app_id"], "app_test");
    assert_eq!(
        opened["config"]["action"], "ctx-action",
        "the action comes from the signed context"
    );
    assert_eq!(opened["config"]["rp_context"]["signature"], "0xsig");
    assert_eq!(opened["config"]["allow_legacy_proofs"], true);
    assert_eq!(opened["config"]["environment"], "sandbox");
    assert_eq!(
        calls_to("poll")[0]["timeout"],
        270_000,
        "the signed window less its margin"
    );

    let posts = requests_to("POST", "/api/world/verify");
    assert_eq!(
        request_body(&posts[0]),
        json!({"token": "tok", "proof": {"protocol_version": "3.0", "nullifier": "0x01"}})
    );
}

#[wasm_bindgen_test]
async fn a_sender_who_closes_world_app_sees_why_and_can_try_again_and_nothing_is_posted() {
    quiesce().await;
    let page = human_only(
        vec![context_route(), verify_route(true)],
        "rejected",
        IdentityMode::Live,
    );

    page.click("I'm human — free");

    page.shows("You closed the World App before finishing.")
        .await;
    assert!(requests_to("POST", "/api/world/verify").is_empty());
    assert!(
        !page.is_disabled("I'm human — free"),
        "a failure must not strand the button"
    );
    page.assert_absent("Open World App");
}

#[wasm_bindgen_test]
async fn the_context_endpoints_own_refusal_is_shown_and_world_app_is_never_opened() {
    quiesce().await;
    let refused = route(
        "POST",
        "/api/world/context",
        vec![reply(
            403,
            json!({"error": "This challenge cannot be verified"}),
        )],
    );
    let page = human_only(vec![refused], "tok", IdentityMode::Live);

    page.click("I'm human — free");

    page.shows("This challenge cannot be verified").await;
    assert!(calls_to("idkit").is_empty());
}

#[wasm_bindgen_test]
async fn an_unreachable_context_endpoint_gets_the_generic_connection_message() {
    quiesce().await;
    let offline = route(
        "POST",
        "/api/world/context",
        vec![json!({"throw": "offline"})],
    );
    let page = human_only(vec![offline], "tok", IdentityMode::Live);

    page.click("I'm human — free");

    page.shows("Could not reach World ID. Check your connection and try again.")
        .await;
}

#[wasm_bindgen_test]
async fn a_context_that_is_not_one_is_refused_before_idkit_hears_of_it() {
    quiesce().await;
    let portal = route(
        "POST",
        "/api/world/context",
        vec![reply(200, json!({"rp_id": "rp_test", "nonce": "n"}))],
    );
    let page = human_only(vec![portal], "tok", IdentityMode::Live);

    page.click("I'm human — free");

    page.shows("World ID sent back something we didn't understand. Try again.")
        .await;
    assert!(calls_to("idkit").is_empty());
}

#[wasm_bindgen_test]
async fn idkit_refusing_to_open_the_request_is_reported() {
    quiesce().await;
    let page = human_only(vec![context_route()], "throw", IdentityMode::Live);

    page.click("I'm human — free");

    page.shows("Could not start World ID verification. Try again.")
        .await;
}

#[wasm_bindgen_test]
async fn a_poll_that_breaks_its_word_ends_as_a_message_not_a_stuck_button() {
    quiesce().await;
    let page = human_only(vec![context_route()], "poll-throws", IdentityMode::Live);

    page.click("I'm human — free");

    page.shows("Verification failed. Try again, or pay instead.")
        .await;
    assert!(!page.is_disabled("I'm human — free"));
}

#[wasm_bindgen_test]
async fn a_rejected_proof_shows_the_servers_message() {
    quiesce().await;
    let rejected = route(
        "POST",
        "/api/world/verify",
        vec![reply(
            200,
            json!({"status": "rejected", "error": "World ID rejected the proof"}),
        )],
    );
    let page = human_only(vec![context_route(), rejected], "tok", IdentityMode::Live);

    page.click("I'm human — free");

    page.shows("World ID rejected the proof").await;
    assert!(!page.is_disabled("I'm human — free"));
}

#[wasm_bindgen_test]
async fn a_verify_that_cannot_be_read_says_verification_failed() {
    quiesce().await;
    let garbled = route("POST", "/api/world/verify", vec![html_reply(502)]);
    let page = human_only(vec![garbled], "tok", IdentityMode::Mock);

    page.click("I'm human — free");

    page.shows("Verification failed").await;
}

#[wasm_bindgen_test]
async fn a_build_without_a_world_app_id_says_it_is_not_configured_and_never_calls_out() {
    quiesce().await;
    clear_storage();
    install_routes(vec![context_route()]);
    let page = mount(|| {
        start_privy(json!({"snapshot": signed_out()}));
        view! {
            <Router>
                <ChallengeActions
                    token="tok"
                    challenge=challenge(false, true, IdentityMode::Live)
                    world={WorldApp { app_id: None, environment: WorldEnvironment::Production }}
                />
            </Router>
        }
    });

    page.click("I'm human — free");

    page.shows("World ID isn't configured yet. Pay instead, or try again shortly.")
        .await;
    assert!(requests().is_empty());
}

#[wasm_bindgen_test]
async fn while_world_app_is_pending_the_link_and_code_show_nothing_navigates_and_it_all_goes_after()
{
    quiesce().await;
    let page = human_only(
        vec![context_route(), verify_route(true)],
        "hang",
        IdentityMode::Live,
    );
    let before = current_url();
    assert!(
        page.query("a[target=_blank]").is_none(),
        "no link before a check starts"
    );

    page.click("I'm human — free");

    eventually("the World App link", || {
        page.query(&format!("a[href='{CONNECTOR}']")).is_some()
    })
    .await;
    let link = page.query("a[target=_blank]").unwrap();
    assert_eq!(link.get_attribute("rel").as_deref(), Some("noreferrer"));
    assert_eq!(link.text_content().unwrap().trim(), "Open World App");
    assert!(
        page.text()
            .contains("Leave this page open — it finishes on its own when you do.")
    );
    assert!(
        page.text()
            .contains("No World App on this device? Scan this with the phone that has it.")
    );
    let code = page.query("svg[role=img]").unwrap();
    assert!(
        code.get_attribute("aria-label")
            .unwrap()
            .contains("scan with the World App")
    );
    let drawn = page
        .query("svg[role=img] path")
        .unwrap()
        .get_attribute("d")
        .unwrap();
    assert!(
        drawn.starts_with("M4 4h1v1h-1z"),
        "finder pattern at the quiet-zone margin: {drawn}"
    );
    assert!(page.is_disabled("Checking…"));
    assert!(
        page.is_disabled("I'm a bot — pay $0.05"),
        "no paying mid-check"
    );
    assert_eq!(
        current_url(),
        before,
        "nothing may unload the page the poll runs on"
    );

    complete_poll(&json!({"success": true, "result": {"protocol_version": "3.0"}}));
    page.shows("Sent.").await;
    page.assert_absent("Open World App");
    assert_eq!(current_url(), before);
}

#[wasm_bindgen_test]
async fn the_link_goes_when_the_check_fails_too_and_the_button_comes_back() {
    quiesce().await;
    let page = human_only(vec![context_route()], "hang", IdentityMode::Live);
    page.click("I'm human — free");
    eventually("the World App link", || {
        page.query("a[target=_blank]").is_some()
    })
    .await;

    complete_poll(&json!({"success": false, "error": "max_verifications_reached"}));

    page.shows("This World ID has already been used.").await;
    assert!(page.query("a[target=_blank]").is_none());
    assert!(page.query("svg[role=img]").is_none());
    assert!(!page.is_disabled("I'm human — free"));
    assert!(!page.is_disabled("I'm a bot — pay $0.05"));
}

#[wasm_bindgen_test]
async fn a_dangerous_challenge_offers_only_the_human_path() {
    quiesce().await;
    let page = open_actions(
        json!({"snapshot": signed_out()}),
        vec![],
        challenge(true, true, IdentityMode::Mock),
        "tok",
        Lane::Paying,
    );

    page.shows("I'm human — free").await;
    assert!(page.button("I'm a bot — pay $0.05").is_none());
    page.assert_absent("Goes to them, not us.");
}

#[wasm_bindgen_test]
async fn arriving_by_the_human_link_keeps_a_way_to_pay_that_firms_up_after_a_failure() {
    quiesce().await;
    let page = open_actions(
        json!({"snapshot": signed_in(WALLET, None, Some("tok"))}),
        vec![context_route()],
        challenge(false, true, IdentityMode::Live),
        "rejected",
        Lane::Human,
    );

    assert!(page.button("I'm a bot — pay $0.05").is_none());
    let quiet = page.button("Pay $0.05 instead").unwrap();
    assert!(
        quiet.class_name().contains("text-muted"),
        "quiet while the free path is worth a try"
    );

    page.click("I'm human — free");
    page.shows("You closed the World App").await;
    let firm = page.button("Pay $0.05 instead").unwrap();
    assert!(
        firm.class_name().contains("border-line-strong"),
        "a real button once it failed"
    );

    page.click("Pay $0.05 instead");
    page.shows("Pay $0.05").await;
}

#[wasm_bindgen_test]
async fn the_way_back_from_the_pay_lane_shows_both_choices_again() {
    quiesce().await;
    let page = open_actions(
        json!({"snapshot": signed_in(WALLET, None, Some("tok"))}),
        vec![],
        challenge(false, true, IdentityMode::Mock),
        "tok",
        Lane::Choosing,
    );

    page.click("I'm a bot — pay $0.05");
    page.shows("Pay $0.05").await;
    assert!(page.button("I'm human — free").is_none());

    page.click("I'm human");
    page.shows("I'm human — free").await;
    assert!(page.button("I'm a bot — pay $0.05").is_some());
}

// ---------------------------------------------------------------------------
// "I'm a bot": pay.
// ---------------------------------------------------------------------------

fn pay_lane(sdk: Value, routes: Vec<Value>) -> Mount {
    open_actions(
        sdk,
        routes,
        challenge(false, true, IdentityMode::Mock),
        "tok",
        Lane::Paying,
    )
}

#[wasm_bindgen_test]
async fn paying_sends_the_escrow_call_the_server_signed_then_the_cleared_message_is_sent() {
    quiesce().await;
    let page = pay_lane(signed_in_sdk(), vec![resolve_route(vec![cleared(true)])]);
    page.shows("Pay $0.05").await;

    page.click("Pay $0.05");

    page.shows("Sent.").await;
    let sent = calls_to("sendTransaction");
    assert_eq!(sent.len(), 1);
    let call = &sent[0];
    assert_eq!(
        call["to"].as_str().unwrap().to_lowercase(),
        POSTAGE_ESCROW.to_string().to_lowercase()
    );
    assert_eq!(call["chainId"], 5_042_002);
    assert_eq!(
        call["valueType"], "bigint",
        "the bridge turns the decimal string into a BigInt"
    );
    assert_eq!(call["value"], FIVE_CENTS.to_string());
    let data = hex::decode(call["data"].as_str().unwrap()).unwrap();
    let decoded = PostageEscrow::payToSendCall::abi_decode(&data).unwrap();
    assert_eq!(decoded.messageId, B256::repeat_byte(0xab));
    assert_eq!(decoded.inbox, Address::repeat_byte(0xcd));
    assert_eq!(decoded.tier, 2);
    assert_eq!(decoded.amount.to_string(), FIVE_CENTS.to_string());
    assert_eq!(decoded.expiresAt.to::<u64>(), 1_800_000_000);
    let resolves = requests_to("POST", "/api/challenge/resolve");
    assert_eq!(request_body(&resolves[0]), json!({"token": "tok"}));
}

#[wasm_bindgen_test]
async fn a_payment_the_server_cannot_see_yet_is_asked_about_again_until_it_clears() {
    quiesce().await;
    let page = pay_lane(
        signed_in_sdk(),
        vec![resolve_route(vec![
            pending(),
            json!({"throw": "offline"}),
            cleared(false),
        ])],
    );
    page.shows("Pay $0.05").await;

    page.click("Pay $0.05");

    page.shows("Cleared.").await;
    assert_eq!(requests_to("POST", "/api/challenge/resolve").len(), 3);
    assert_eq!(calls_to("sendTransaction").len(), 1);
}

#[wasm_bindgen_test]
async fn a_charge_is_shown_as_the_penalty_it_is() {
    quiesce().await;
    let charged = reply(200, json!({"status": "charged", "reason": "dangerous"}));
    let page = pay_lane(signed_in_sdk(), vec![resolve_route(vec![charged])]);
    page.shows("Pay $0.05").await;

    page.click("Pay $0.05");

    page.shows("Charged. Still blocked.").await;
    assert!(
        page.text()
            .contains("Paying is the penalty here, not a price.")
    );
}

#[wasm_bindgen_test]
async fn a_payment_that_broadcast_but_is_not_seen_keeps_its_hash_and_is_never_offered_again() {
    quiesce().await;
    let page = pay_lane(signed_in_sdk(), vec![resolve_route(vec![pending()])]);
    page.shows("Pay $0.05").await;

    page.click("Pay $0.05");

    page.shows("Paid. Confirming.").await;
    // Never reported as a failure: the money has moved.
    settle().await;
    assert!(page.text().contains("It's on the chain. Confirming can take a few minutes, and paying again would charge you twice."));
    assert_eq!(
        page.query("[data-testid=tx-hash]")
            .unwrap()
            .text_content()
            .unwrap(),
        TX_HASH
    );
    assert!(
        page.button("Pay $0.05").is_none(),
        "a second payment must not be one click away"
    );
    assert!(page.button("I'm human").is_none());
    page.assert_absent("Still nothing");
    assert_eq!(
        requests_to("POST", "/api/challenge/resolve").len(),
        3,
        "first ask plus the window's retries"
    );

    page.click("Check again");
    page.shows("Still nothing on our side. It can take a few minutes — check again shortly.")
        .await;
    assert_eq!(
        calls_to("sendTransaction").len(),
        1,
        "checking again moves no money"
    );
    assert!(page.query("[data-testid=tx-hash]").is_some());
}

#[wasm_bindgen_test]
async fn checking_again_finds_the_payment_once_the_server_has_seen_it() {
    quiesce().await;
    let page = pay_lane(
        signed_in_sdk(),
        vec![resolve_route(vec![
            pending(),
            pending(),
            pending(),
            cleared(true),
        ])],
    );
    page.shows("Pay $0.05").await;
    page.click("Pay $0.05");
    page.shows("Paid. Confirming.").await;
    window_ran_out(&page).await;

    page.click("Check again");

    page.shows("Sent.").await;
    assert_eq!(calls_to("sendTransaction").len(), 1);
}

#[wasm_bindgen_test]
async fn the_confirmation_stays_up_while_checking_and_says_so() {
    quiesce().await;
    let page = pay_lane(signed_in_sdk(), vec![resolve_route(vec![pending()])]);
    page.shows("Pay $0.05").await;
    page.click("Pay $0.05");
    page.shows("Paid. Confirming.").await;
    window_ran_out(&page).await;

    replace_routes(vec![resolve_route(vec![
        json!({"gate": "ask", "status": 200,
        "body": json!({"status": "pending"}).to_string()}),
    ])]);
    page.click("Check again");

    page.shows("Checking…").await;
    assert!(page.is_disabled("Checking…"));
    open_gate("ask");
}

#[wasm_bindgen_test]
async fn a_wallet_that_refuses_the_payment_shows_its_words_and_offers_pay_again_with_no_request_made()
 {
    quiesce().await;
    let sdk = json!({
        "snapshot": signed_in(WALLET, None, Some("tok")),
        "sendError": {"code": 4001, "message": "User rejected request"},
    });
    let page = pay_lane(sdk, vec![resolve_route(vec![cleared(true)])]);
    page.shows("Pay $0.05").await;

    page.click("Pay $0.05");

    page.shows("User rejected request").await;
    assert!(!page.is_disabled("Pay $0.05"));
    assert!(
        requests_to("POST", "/api/challenge/resolve").is_empty(),
        "nothing was paid, nothing to ask"
    );
    assert!(page.button("I'm human").is_some());
}

#[wasm_bindgen_test]
async fn nothing_can_be_clicked_while_a_transaction_is_in_flight() {
    quiesce().await;
    let sdk = json!({"snapshot": signed_in(WALLET, None, Some("tok")), "holdSend": true});
    let page = pay_lane(sdk, vec![resolve_route(vec![cleared(true)])]);
    page.shows("Pay $0.05").await;

    page.click("Pay $0.05");

    page.shows("Paying…").await;
    assert!(page.is_disabled("Paying…"));
    assert!(
        page.is_disabled("I'm human"),
        "leaving would lose the record of a payment"
    );
    eventually("the send", || calls_to("sendTransaction").len() == 1).await;

    release_send();
    page.shows("Sent.").await;
    assert_eq!(calls_to("sendTransaction").len(), 1);
}

#[wasm_bindgen_test]
async fn the_pay_lane_says_loading_until_privy_is_ready() {
    quiesce().await;
    let page = pay_lane(json!({"ready": false}), vec![]);

    page.shows("Loading").await;
    assert!(page.button("Pay $0.05").is_none());
}

#[wasm_bindgen_test]
async fn a_signed_in_user_without_a_wallet_yet_waits_on_it() {
    quiesce().await;
    let sdk = json!({"snapshot": {
        "ready": true, "authenticated": true, "userId": "did:privy:test",
        "email": "t@example.com", "wallets": [], "identityToken": "tok",
    }});
    let page = pay_lane(sdk, vec![]);

    page.shows("Setting up your wallet…").await;
    assert!(page.is_disabled("Setting up your wallet…"));

    emit(&signed_in(WALLET, Some("t@example.com"), Some("tok")));
    page.shows("Pay $0.05").await;
    assert!(!page.is_disabled("Pay $0.05"));
    assert!(
        calls_to("sendTransaction").is_empty(),
        "the wallet appearing is not a click"
    );
}

#[wasm_bindgen_test]
async fn paying_signed_out_logs_in_then_pays_once_without_a_second_click() {
    quiesce().await;
    let sdk = json!({
        "snapshot": signed_out(),
        "afterLogin": signed_in(WALLET, Some("t@example.com"), Some("tok")),
    });
    let page = pay_lane(sdk, vec![resolve_route(vec![cleared(true)])]);
    page.shows("Pay $0.05").await;

    page.click("Pay $0.05");

    page.shows("Sent.").await;
    assert_eq!(calls_to("login").len(), 1);
    assert_eq!(calls_to("sendTransaction").len(), 1, "exactly one payment");
}

#[wasm_bindgen_test]
async fn closing_the_login_withdraws_the_click_so_a_later_sign_in_does_not_pay() {
    quiesce().await;
    let sdk = json!({"snapshot": signed_out(), "loginError": "exited_auth_flow"});
    let page = pay_lane(sdk, vec![resolve_route(vec![cleared(true)])]);
    page.shows("Pay $0.05").await;

    page.click("Pay $0.05");
    eventually("the login to open", || calls_to("login").len() == 1).await;
    settle().await;
    page.assert_absent("exited_auth_flow");

    emit(&signed_in(WALLET, Some("t@example.com"), Some("tok")));
    settle().await;

    assert!(calls_to("sendTransaction").is_empty());
    assert!(page.button("Pay $0.05").is_some());
}

#[wasm_bindgen_test]
async fn a_signed_in_wallet_pays_directly_and_a_second_payment_never_follows() {
    quiesce().await;
    let page = pay_lane(signed_in_sdk(), vec![resolve_route(vec![cleared(true)])]);
    page.shows("Pay $0.05").await;

    page.click("Pay $0.05");
    page.shows("Sent.").await;
    settle().await;

    assert_eq!(calls_to("sendTransaction").len(), 1);
    assert!(calls_to("login").is_empty());
}

// ---------------------------------------------------------------------------
// Paste the message again.
// ---------------------------------------------------------------------------

fn deliver_form(routes: Vec<Value>) -> Mount {
    let page = human_only(
        [vec![verify_route(false)], routes].concat(),
        "tok",
        IdentityMode::Mock,
    );
    page.click("I'm human — free");
    page
}

#[wasm_bindgen_test]
async fn the_paste_form_asks_for_a_body_before_it_will_send() {
    quiesce().await;
    let page = deliver_form(vec![]);
    page.shows("Cleared.").await;

    assert!(page.is_disabled("Send"));
    assert_eq!(
        page.query("textarea")
            .unwrap()
            .get_attribute("placeholder")
            .as_deref(),
        Some("Paste what you wrote to demo@usepostage.com")
    );
    assert_eq!(
        page.query("input")
            .unwrap()
            .get_attribute("placeholder")
            .as_deref(),
        Some("Subject")
    );
    page.type_into("textarea", "   \n ");
    tick().await;
    assert!(page.is_disabled("Send"), "whitespace is not a message");
    page.type_into("textarea", "hello there");
    tick().await;
    assert!(!page.is_disabled("Send"));
}

#[wasm_bindgen_test]
async fn the_pasted_message_goes_out_under_the_token_and_the_form_gives_way_to_sent() {
    quiesce().await;
    let held = json!({"gate": "deliver", "status": 200,
        "body": json!({"status": "delivered", "to": "a@b.c"}).to_string()});
    let page = deliver_form(vec![route("POST", "/api/challenge/deliver", vec![held])]);
    page.shows("Cleared.").await;
    page.type_into("input", "Re: invoice");
    page.type_into("textarea", "Please see attached.");
    tick().await;

    page.click("Send");

    page.shows("Sending…").await;
    assert!(page.is_disabled("Sending…"));
    open_gate("deliver");
    page.shows("Replies come straight to you.").await;
    assert!(page.text().contains("Sent."));
    let posts = requests_to("POST", "/api/challenge/deliver");
    assert_eq!(posts.len(), 1);
    assert_eq!(
        request_body(&posts[0]),
        json!({"token": "tok", "subject": "Re: invoice", "body": "Please see attached."})
    );
}

#[wasm_bindgen_test]
async fn a_refused_paste_shows_the_servers_words_and_keeps_what_was_typed() {
    quiesce().await;
    let refused = reply(403, json!({"error": "That pass has run out"}));
    let page = deliver_form(vec![route("POST", "/api/challenge/deliver", vec![refused])]);
    page.shows("Cleared.").await;
    page.type_into("textarea", "my words");
    tick().await;

    page.click("Send");

    page.shows("That pass has run out").await;
    assert_eq!(page.input_value("textarea"), "my words");
    assert!(!page.is_disabled("Send"));
    page.assert_absent("Replies come straight to you.");
}

#[wasm_bindgen_test]
async fn a_paste_that_cannot_reach_the_server_says_so_and_can_be_retried() {
    quiesce().await;
    let page = deliver_form(vec![route(
        "POST",
        "/api/challenge/deliver",
        vec![
            json!({"throw": "offline"}),
            reply(200, json!({"status": "delivered", "to": "a@b.c"})),
        ],
    )]);
    page.shows("Cleared.").await;
    page.type_into("textarea", "my words");
    tick().await;

    page.click("Send");
    page.shows("Could not reach the server.").await;

    page.click("Send");
    page.shows("Replies come straight to you.").await;
}

#[wasm_bindgen_test]
async fn a_200_that_is_not_the_servers_answer_is_not_a_delivery() {
    quiesce().await;
    let page = deliver_form(vec![route(
        "POST",
        "/api/challenge/deliver",
        vec![html_reply(200)],
    )]);
    page.shows("Cleared.").await;
    page.type_into("textarea", "my words");
    tick().await;

    page.click("Send");

    page.shows("Could not deliver it").await;
    page.assert_absent("Replies come straight to you.");
}

#[wasm_bindgen_test]
async fn paying_a_hold_that_ran_out_lands_on_the_same_paste_form() {
    quiesce().await;
    let page = pay_lane(signed_in_sdk(), vec![resolve_route(vec![cleared(false)])]);
    page.shows("Pay $0.05").await;

    page.click("Pay $0.05");

    page.shows("The hold expired. Paste it again.").await;
    assert!(page.query("textarea").is_some());
}

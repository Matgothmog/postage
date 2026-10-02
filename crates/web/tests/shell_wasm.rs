//! Renders the app shell in a real browser: the landing page and its claim
//! hero, the routes and the error screen. Run with:
//! `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!  cargo test -p postage-web --target wasm32-unknown-unknown --test shell_wasm`

#![cfg(target_arch = "wasm32")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use leptos::prelude::*;
use leptos_router::components::Router;
use postage_web::account::Account;
use postage_web::app::AppRoutes;
use postage_web::landing::Landing;
use postage_web::screens::ErrorGuard;
use serde_json::json;
use support::*;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

fn go_to(path: &str) {
    leptos::tachys::dom::window()
        .history()
        .unwrap()
        .push_state_with_url(&JsValue::NULL, "", Some(path))
        .unwrap();
}

/// The home page over a signed-out session.
fn open_signed_out(ready: bool) -> Mount {
    clear_storage();
    install_routes(vec![]);
    mount(move || {
        start_privy(json!({"ready": ready, "snapshot": {
            "ready": ready, "authenticated": false, "userId": null,
            "email": null, "wallets": [], "identityToken": null}}));
        view! { <Router><Account landing=ViewFn::from(|| view! { <Landing /> }) /></Router> }
    })
}

#[wasm_bindgen_test]
async fn landing_shows_its_pitch_and_the_hero_claim_opens_the_login() {
    quiesce().await;
    let page = open_signed_out(true);
    page.shows("Make spam pay.").await;

    page.type_into("#hero-handle", "demo");
    eventually("the hero to enable", || !page.is_disabled("Claim it")).await;
    page.click("Claim it");
    eventually("the login to open", || !calls_to("login").is_empty()).await;
}

#[wasm_bindgen_test]
async fn the_hero_stays_disabled_until_the_handle_is_valid_and_privy_is_ready() {
    quiesce().await;
    let page = open_signed_out(true);
    page.shows("Make spam pay.").await;
    assert!(page.is_disabled("Claim it"), "no handle yet");
    page.type_into("#hero-handle", "-bad");
    tick().await;
    assert!(
        page.is_disabled("Claim it"),
        "a handle the server would reject"
    );
    drop(page);

    let waiting = open_signed_out(false);
    waiting.shows("Make spam pay.").await;
    waiting.type_into("#hero-handle", "demo");
    tick().await;
    assert!(
        waiting.is_disabled("Claim it"),
        "Privy has not finished loading"
    );
    assert!(waiting.is_disabled("Sign in"));
    assert!(calls_to("login").is_empty());
}

#[wasm_bindgen_test]
async fn the_hero_keeps_only_characters_a_handle_may_contain() {
    quiesce().await;
    let page = open_signed_out(true);
    page.shows("Make spam pay.").await;
    page.type_into("#hero-handle", "Jo hn+x");
    assert_eq!(page.input_value("#hero-handle"), "Johnx");
}

#[wasm_bindgen_test]
async fn an_unknown_path_renders_the_not_found_screen() {
    quiesce().await;
    go_to("/no/such/page");
    let page = mount(|| view! { <AppRoutes /> });
    page.shows("Nothing here.").await;
    assert!(page.query("a[href='/']").is_some());
}

#[wasm_bindgen_test]
async fn the_challenge_and_network_routes_render_their_pages() {
    quiesce().await;
    go_to("/c/abc123");
    install_routes(vec![route(
        "GET",
        "/api/challenge/abc123",
        vec![reply(200, json!({"state": "dead"}))],
    )]);
    let challenge = mount(|| view! { <AppRoutes /> });
    challenge.shows("Dead link.").await;
    assert_eq!(requests_to("GET", "/api/challenge/abc123").len(), 1);
    drop(challenge);

    go_to("/network");
    let network = mount(|| view! { <AppRoutes /> });
    network.shows("Ledger").await;
}

#[wasm_bindgen_test]
async fn home_shows_the_owners_inbox_once_signed_in() {
    quiesce().await;
    go_to("/");
    clear_storage();
    install_routes(vec![
        route(
            "GET",
            "/api/inbox",
            vec![reply(
                200,
                json!({"inbox": {"handle": "demo", "destination": "d@example.com"}}),
            )],
        ),
        route_with_body(
            "POST",
            "https://rpc.testnet.arc.network",
            "0x",
            vec![reply(200, json!({"result": format!("0x{:064x}", 0)}))],
        ),
    ]);
    let page = mount(|| {
        start_privy(json!({}));
        view! { <AppRoutes /> }
    });
    page.shows("demo@usepostage.com").await;
    assert!(page.text().contains("Paid into 0x4469"));
    page.assert_absent("Make spam pay.");
}

#[wasm_bindgen_test]
async fn home_shows_the_landing_page_while_signed_out() {
    quiesce().await;
    go_to("/");
    install_routes(vec![]);
    let page = mount(|| view! { <AppRoutes /> });
    page.shows("Make spam pay.").await;
    assert!(page.is_disabled("Sign in"));
}

#[wasm_bindgen_test]
async fn a_failing_screen_shows_the_error_view_and_try_again_rebuilds_it() {
    quiesce().await;
    let builds = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&builds);
    let page = mount(move || {
        view! {
            <Router>
                <ErrorGuard>
                    {
                        counted.fetch_add(1, Ordering::SeqCst);
                        Err::<(), _>(std::io::Error::other("boom"))
                    }
                </ErrorGuard>
            </Router>
        }
    });
    page.shows("Lost in transit.").await;
    assert!(page.text().contains("Not your fault. Try again."));
    let before = builds.load(Ordering::SeqCst);
    page.click("Try again");
    tick().await;
    assert!(builds.load(Ordering::SeqCst) > before);
}

#[wasm_bindgen_test]
async fn a_healthy_screen_passes_through_the_error_guard() {
    quiesce().await;
    let page = mount(|| view! { <Router><ErrorGuard><p>"all fine"</p></ErrorGuard></Router> });
    page.shows("all fine").await;
    page.assert_absent("Lost in transit.");
}

//! Renders the app shell in a real browser: the landing call to action, the
//! routes and the error screen. Run with:
//! `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!  cargo test -p postage-web --target wasm32-unknown-unknown --test shell_wasm`

#![cfg(target_arch = "wasm32")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use leptos::mount::mount_to;
use leptos::prelude::*;
use leptos::tachys::dom::document;
use leptos::wasm_bindgen::JsCast;
use leptos::web_sys::{Element, HtmlElement};
use leptos_router::components::Router;
use postage_web::app::AppRoutes;
use postage_web::bridge::privy::{Privy, PrivyConfig};
use postage_web::landing::Landing;
use postage_web::screens::ErrorGuard;
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

/// A page region a test mounts into; removed again when dropped.
struct Mount {
    container: Element,
    _handle: Box<dyn std::any::Any>,
}

impl Mount {
    fn text(&self) -> String {
        self.container.text_content().unwrap_or_default()
    }

    fn click(&self, selector: &str) {
        self.container
            .query_selector(selector)
            .unwrap()
            .unwrap_or_else(|| {
                panic!(
                    "nothing matches {selector}: {}",
                    self.container.inner_html()
                )
            })
            .unchecked_into::<HtmlElement>()
            .click();
    }

    fn find_button(&self, label: &str) -> HtmlElement {
        let buttons = self.container.get_elements_by_tag_name("button");
        (0..buttons.length())
            .filter_map(|index| buttons.item(index))
            .map(|node| node.unchecked_into::<HtmlElement>())
            .find(|button| {
                button
                    .text_content()
                    .is_some_and(|text| text.trim() == label)
            })
            .unwrap_or_else(|| panic!("no {label:?} button in {}", self.container.inner_html()))
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        self.container.remove();
    }
}

fn mount<V: IntoView + 'static>(view: impl FnOnce() -> V + 'static) -> Mount {
    let container = document().create_element("div").unwrap();
    document().body().unwrap().append_child(&container).unwrap();
    let handle = mount_to(container.clone().unchecked_into(), view);
    Mount {
        container,
        _handle: Box::new(handle),
    }
}

fn go_to(path: &str) {
    leptos::tachys::dom::window()
        .history()
        .unwrap()
        .push_state_with_url(&JsValue::NULL, "", Some(path))
        .unwrap();
}

fn calls() -> Vec<Value> {
    serde_json::from_str(&mock_calls()).unwrap()
}

/// Lets queued microtasks and timers (the mock's snapshots, router updates,
/// effects) run.
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

/// Starts Privy over the scripted SDK. Must run inside the owner that will
/// render the components, so the handle is provided to them as context.
fn start_privy(options: Value) -> Privy {
    install_bridge(&js_sys::global(), &create_mock_sdk(&options.to_string()));
    Privy::start(&PrivyConfig::new("test-app")).unwrap()
}

#[wasm_bindgen_test]
async fn landing_shows_its_pitch_and_the_cta_opens_the_login() {
    let page = mount(|| {
        provide_context(start_privy(json!({ "ready": true })));
        view! { <Router><Landing /></Router> }
    });
    tick().await;

    assert!(page.text().contains("Make spam pay."));
    page.find_button("Claim your address").click();
    tick().await;

    assert!(calls().iter().any(|call| call["fn"] == "login"));
}

#[wasm_bindgen_test]
async fn the_cta_stays_disabled_until_privy_is_ready() {
    let page = mount(|| {
        provide_context(start_privy(json!({ "ready": false })));
        view! { <Router><Landing /></Router> }
    });
    tick().await;

    let button = page.find_button("Claim your address");
    assert!(button.has_attribute("disabled"));
    button.click();
    tick().await;
    assert!(!calls().iter().any(|call| call["fn"] == "login"));
}

#[wasm_bindgen_test]
async fn an_unknown_path_renders_the_not_found_screen() {
    go_to("/no/such/page");
    let page = mount(|| view! { <AppRoutes /> });
    tick().await;

    assert!(page.text().contains("Nothing here."));
    assert!(
        page.container
            .query_selector("a[href='/']")
            .unwrap()
            .is_some()
    );
}

#[wasm_bindgen_test]
async fn the_challenge_and_network_routes_render_their_pages() {
    go_to("/c/abc123");
    let challenge = mount(|| view! { <AppRoutes /> });
    tick().await;
    assert!(challenge.text().contains("Challenge"));
    assert!(
        challenge
            .container
            .query_selector("[data-token='abc123']")
            .unwrap()
            .is_some()
    );
    drop(challenge);

    go_to("/network");
    let network = mount(|| view! { <AppRoutes /> });
    tick().await;
    assert!(network.text().contains("Ledger"));
}

#[wasm_bindgen_test]
async fn home_shows_the_account_page_once_signed_in() {
    go_to("/");
    let page = mount(|| {
        provide_context(start_privy(json!({ "ready": true })));
        view! { <AppRoutes /> }
    });
    tick().await;

    // The mock reports an authenticated session with a wallet.
    assert!(page.text().contains("Signed in as"));
    assert!(page.text().contains("0x4469"));
    assert!(!page.text().contains("Make spam pay."));
}

#[wasm_bindgen_test]
async fn home_shows_the_landing_page_while_signed_out() {
    go_to("/");
    let page = mount(|| view! { <AppRoutes /> });
    tick().await;

    assert!(page.text().contains("Make spam pay."));
    assert!(!page.text().contains("Signed in as"));
    assert!(page.find_button("Sign in").has_attribute("disabled"));
}

#[wasm_bindgen_test]
async fn a_failing_screen_shows_the_error_view_and_try_again_rebuilds_it() {
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
    tick().await;

    assert!(page.text().contains("Lost in transit."));
    assert!(page.text().contains("Not your fault. Try again."));
    let before = builds.load(Ordering::SeqCst);
    page.click("button");
    tick().await;
    assert!(builds.load(Ordering::SeqCst) > before);
}

#[wasm_bindgen_test]
async fn a_healthy_screen_passes_through_the_error_guard() {
    let page = mount(|| view! { <Router><ErrorGuard><p>"all fine"</p></ErrorGuard></Router> });
    tick().await;
    assert!(page.text().contains("all fine"));
    assert!(!page.text().contains("Lost in transit."));
}

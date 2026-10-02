//! Shared harness for the browser tests that mount real components: the
//! scripted Privy SDK (`mock_sdk.js`), the scripted `fetch` (`mock_fetch.js`),
//! a place in the page to mount into, and waiting that does not depend on
//! timing luck.

#![cfg(target_arch = "wasm32")]
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use leptos::mount::mount_to;
use leptos::prelude::*;
use leptos::tachys::dom::document;
use leptos::wasm_bindgen::JsCast;
use leptos::web_sys::{Element, HtmlElement, HtmlInputElement};
use postage_web::bridge::privy::{Privy, PrivyConfig};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

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
    #[wasm_bindgen(js_name = mockEmit)]
    fn mock_emit(snapshot_json: &str);
}

#[wasm_bindgen(module = "/tests/mock_fetch.js")]
extern "C" {
    #[wasm_bindgen(js_name = installFetch)]
    fn install_fetch(routes_json: &str);
    #[wasm_bindgen(js_name = restoreFetch)]
    pub fn restore_fetch();
    #[wasm_bindgen(js_name = setRoutes)]
    fn set_routes_json(routes_json: &str);
    #[wasm_bindgen(js_name = fetchLog)]
    fn fetch_log_json() -> String;
    #[wasm_bindgen(js_name = openGate)]
    pub fn open_gate(name: &str);
}

/// The wallet the mock SDK signs in by default.
pub const WALLET: &str = "0x4469E869433Cf6Cc08DD54AFc6AC7e288b9A38f7";
pub const OTHER_WALLET: &str = "0x19E7E376E7C213B7E7e7e46cc70A5dD086DAff2A";
pub const CLAIM_KEY: &str = "postage.pending-claim";

/// A page region a test mounts into; removed again when dropped.
pub struct Mount {
    pub container: Element,
    _handle: Box<dyn std::any::Any>,
}

impl Mount {
    pub fn text(&self) -> String {
        self.container.text_content().unwrap_or_default()
    }

    pub fn html(&self) -> String {
        self.container.inner_html()
    }

    pub fn query(&self, selector: &str) -> Option<Element> {
        self.container.query_selector(selector).unwrap()
    }

    pub fn button(&self, label: &str) -> Option<HtmlElement> {
        let buttons = self.container.get_elements_by_tag_name("button");
        (0..buttons.length())
            .filter_map(|index| buttons.item(index))
            .map(|node| node.unchecked_into::<HtmlElement>())
            .find(|button| {
                button
                    .text_content()
                    .is_some_and(|text| text.trim() == label)
            })
    }

    pub fn buttons_labelled(&self, label: &str) -> usize {
        let buttons = self.container.get_elements_by_tag_name("button");
        (0..buttons.length())
            .filter_map(|index| buttons.item(index))
            .filter(|button| {
                button
                    .text_content()
                    .is_some_and(|text| text.trim() == label)
            })
            .count()
    }

    pub fn click(&self, label: &str) {
        self.button(label)
            .unwrap_or_else(|| panic!("no {label:?} button in {}", self.html()))
            .click();
    }

    pub fn is_disabled(&self, label: &str) -> bool {
        self.button(label)
            .unwrap_or_else(|| panic!("no {label:?} button in {}", self.html()))
            .has_attribute("disabled")
    }

    /// Types `value` into the input matching `selector`, the way a keystroke
    /// would: set the value, then fire a bubbling `input` event.
    pub fn type_into(&self, selector: &str, value: &str) {
        let input: HtmlInputElement = self
            .query(selector)
            .unwrap_or_else(|| panic!("nothing matches {selector}: {}", self.html()))
            .unchecked_into();
        input.set_value(value);
        let init = web_sys::EventInit::new();
        init.set_bubbles(true);
        let event = web_sys::Event::new_with_event_init_dict("input", &init).unwrap();
        input.dispatch_event(&event).unwrap();
    }

    pub fn input_value(&self, selector: &str) -> String {
        self.query(selector)
            .unwrap_or_else(|| panic!("nothing matches {selector}: {}", self.html()))
            .unchecked_into::<HtmlInputElement>()
            .value()
    }

    /// Waits until the page text contains `needle`.
    pub async fn shows(&self, needle: &str) {
        for _ in 0..300 {
            if self.text().contains(needle) {
                return;
            }
            sleep(10).await;
        }
        panic!(
            "timed out waiting for the page to show {needle:?}; it shows {:?}",
            self.text()
        );
    }

    /// Panics with the page if `needle` is on it.
    pub fn assert_absent(&self, needle: &str) {
        assert!(
            !self.text().contains(needle),
            "{needle:?} should not be on the page: {}",
            self.text()
        );
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        self.container.remove();
    }
}

pub fn mount<V: IntoView + 'static>(view: impl FnOnce() -> V + 'static) -> Mount {
    let container = document().create_element("div").unwrap();
    document().body().unwrap().append_child(&container).unwrap();
    let handle = mount_to(container.clone().unchecked_into(), view);
    Mount {
        container,
        _handle: Box::new(handle),
    }
}

pub async fn sleep(millis: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let set_timeout: js_sys::Function =
            js_sys::Reflect::get(&js_sys::global(), &"setTimeout".into())
                .unwrap()
                .into();
        set_timeout
            .call2(&JsValue::NULL, &resolve, &millis.into())
            .unwrap();
    });
    JsFuture::from(promise).await.unwrap();
}

/// Lets queued microtasks and timers (snapshots, effects, resolved fetches)
/// run.
pub async fn tick() {
    sleep(0).await;
}

/// Run first in every test: work the previous test left in flight (a read
/// that was still on its way when its page was dropped) finishes against that
/// test's script, not this one's.
pub async fn quiesce() {
    sleep(80).await;
}

/// A fixed grace period for asserting that something did *not* happen.
pub async fn settle() {
    sleep(120).await;
}

/// Polls `condition` until it holds, up to about three seconds.
pub async fn eventually(what: &str, condition: impl Fn() -> bool) {
    for _ in 0..300 {
        if condition() {
            return;
        }
        sleep(10).await;
    }
    panic!("timed out waiting for {what}");
}

/// Starts Privy over the scripted SDK. Must run inside the owner that will
/// render the components, so the handle is provided to them as context.
pub fn start_privy(options: Value) -> Privy {
    install_bridge(&js_sys::global(), &create_mock_sdk(&options.to_string()));
    let privy = Privy::start(&PrivyConfig::new("test-app")).unwrap();
    provide_context(privy);
    privy
}

/// A signed-in snapshot for `wallet` with the given session fields.
pub fn signed_in(wallet: &str, email: Option<&str>, identity_token: Option<&str>) -> Value {
    json!({
        "ready": true,
        "authenticated": true,
        "userId": "did:privy:test",
        "email": email,
        "wallets": [{ "address": wallet, "walletClientType": "privy" }],
        "identityToken": identity_token,
    })
}

pub fn signed_out() -> Value {
    json!({
        "ready": true, "authenticated": false, "userId": null,
        "email": null, "wallets": [], "identityToken": null,
    })
}

/// Pushes a new auth snapshot, as Privy does on a token rotation or sign-in.
pub fn emit(snapshot: &Value) {
    mock_emit(&snapshot.to_string());
}

pub fn calls() -> Vec<Value> {
    serde_json::from_str(&mock_calls()).unwrap()
}

pub fn calls_to(name: &str) -> Vec<Value> {
    calls()
        .into_iter()
        .filter(|call| call["fn"] == name)
        .collect()
}

/// One scripted route: see `mock_fetch.js`.
pub fn route(method: &str, url: &str, responses: Vec<Value>) -> Value {
    json!({ "method": method, "url": url, "responses": responses })
}

pub fn route_with_body(
    method: &str,
    url: &str,
    body_includes: &str,
    responses: Vec<Value>,
) -> Value {
    json!({ "method": method, "url": url, "bodyIncludes": body_includes, "responses": responses })
}

/// A JSON response.
pub fn reply(status: u16, body: Value) -> Value {
    json!({ "status": status, "body": body.to_string() })
}

/// A response whose body is not JSON (a gateway's error page).
pub fn html_reply(status: u16) -> Value {
    json!({ "status": status, "body": "<html>bad gateway</html>" })
}

pub fn install_routes(routes: Vec<Value>) {
    install_fetch(&Value::Array(routes).to_string());
}

pub fn replace_routes(routes: Vec<Value>) {
    set_routes_json(&Value::Array(routes).to_string());
}

pub fn requests() -> Vec<Value> {
    serde_json::from_str(&fetch_log_json()).unwrap()
}

/// Every recorded request with this method and URL.
pub fn requests_to(method: &str, url: &str) -> Vec<Value> {
    requests()
        .into_iter()
        .filter(|request| request["method"] == method && request["url"] == url)
        .collect()
}

pub fn request_body(request: &Value) -> Value {
    serde_json::from_str(request["body"].as_str().unwrap_or("null")).unwrap()
}

pub fn clear_storage() {
    let storage = leptos::tachys::dom::window()
        .local_storage()
        .unwrap()
        .unwrap();
    storage.clear().unwrap();
}

pub fn stored_claim() -> Option<Value> {
    let storage = leptos::tachys::dom::window()
        .local_storage()
        .unwrap()
        .unwrap();
    storage
        .get_item(CLAIM_KEY)
        .unwrap()
        .map(|raw| serde_json::from_str(&raw).unwrap())
}

pub fn store_claim(wallet: &str, handle: &str, destination: &str, code_verified: bool) {
    let storage = leptos::tachys::dom::window()
        .local_storage()
        .unwrap()
        .unwrap();
    let value = json!({
        "wallet": wallet,
        "claim": {
            "handle": handle, "destination": destination,
            "codeVerified": code_verified, "cloudflareVerified": false,
        }
    });
    storage.set_item(CLAIM_KEY, &value.to_string()).unwrap();
}

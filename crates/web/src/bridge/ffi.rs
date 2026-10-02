//! `wasm-bindgen` declarations of the object `js/src/bridge-core.js` builds.
//! Kept to the shapes that file defines; the typed API is in `privy`/`idkit`.

use wasm_bindgen::prelude::*;

/// Must match `BRIDGE_GLOBAL` in `js/src/bridge-core.js`.
pub(super) const BRIDGE_GLOBAL: &str = "__postageBridge";

#[wasm_bindgen]
extern "C" {
    #[derive(Debug, Clone)]
    pub(super) type Bridge;
    #[wasm_bindgen(method, getter)]
    pub(super) fn privy(this: &Bridge) -> PrivyBridge;
    #[wasm_bindgen(method, getter)]
    pub(super) fn idkit(this: &Bridge) -> IdkitBridge;

    #[derive(Debug, Clone)]
    pub(super) type PrivyBridge;
    #[wasm_bindgen(method, catch)]
    pub(super) fn start(
        this: &PrivyBridge,
        config_json: &str,
        on_state: &js_sys::Function,
    ) -> Result<(), JsValue>;
    #[wasm_bindgen(method)]
    pub(super) fn login(this: &PrivyBridge) -> js_sys::Promise;
    #[wasm_bindgen(method)]
    pub(super) fn logout(this: &PrivyBridge) -> js_sys::Promise;
    #[wasm_bindgen(method, js_name = signMessage)]
    pub(super) fn sign_message(this: &PrivyBridge, message: &str, address: &str)
    -> js_sys::Promise;
    #[wasm_bindgen(method, js_name = sendTransaction)]
    pub(super) fn send_transaction(this: &PrivyBridge, tx_json: &str) -> js_sys::Promise;
    #[wasm_bindgen(method, js_name = identityToken)]
    pub(super) fn identity_token(this: &PrivyBridge) -> js_sys::Promise;

    #[derive(Debug, Clone)]
    pub(super) type IdkitBridge;
    #[wasm_bindgen(method, js_name = openSelfieCheck)]
    pub(super) fn open_selfie_check(this: &IdkitBridge, request_json: &str) -> js_sys::Promise;

    #[derive(Debug, Clone)]
    pub type SelfieCheckJsHandle;
    #[wasm_bindgen(method, getter, js_name = connectorURI)]
    pub(super) fn connector_uri(this: &SelfieCheckJsHandle) -> JsValue;
    #[wasm_bindgen(method, js_name = pollJson)]
    pub(super) fn poll_json(this: &SelfieCheckJsHandle, timeout_ms: f64) -> js_sys::Promise;
}

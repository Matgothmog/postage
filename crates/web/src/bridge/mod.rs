//! Rust access to the two vendor SDKs that only exist as JavaScript: Privy
//! (`privy`) and World IDKit (`idkit`).
//!
//! `js/` bundles both behind one small object, `globalThis.__postageBridge`
//! (`js/src/bridge-core.js`), which `index.html` loads before the wasm. Every
//! value crossing the boundary is JSON text and every rejection is a
//! `{code, message}` object, so all decoding here is plain serde over `&str`
//! and runs natively under test; only the `ffi` calls need a browser.

pub mod idkit;
pub mod privy;

mod ffi;

use serde::Deserialize;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Why a bridge call did not produce its value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BridgeError {
    /// `bridge.js` never ran: missing from the page, or blocked.
    #[error("the Privy/World ID bridge script is not loaded")]
    NotLoaded,
    /// The caller passed something the SDK would only reject later.
    #[error("invalid request: {0}")]
    InvalidInput(&'static str),
    /// The SDK refused or failed. `code` is the SDK's own (a Privy error code
    /// such as `exited_auth_flow`, an EIP-1193 code such as `4001`, or one of
    /// the bridge's: `not_ready`, `superseded`, `already_started`, ...).
    #[error("{message} ({code})")]
    Sdk { code: String, message: String },
    /// The bridge answered with something this side cannot read.
    #[error("unexpected value from the bridge: {0}")]
    Decode(String),
}

impl BridgeError {
    /// The SDK's error code, when there is one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Sdk { code, .. } => Some(code),
            _ => None,
        }
    }

    /// The user closed a prompt or declined it: worth no error message.
    pub fn is_user_cancel(&self) -> bool {
        matches!(
            self.code(),
            Some("exited_auth_flow" | "exited_link_flow" | "4001" | "superseded")
        )
    }

    /// Privy was called before it finished loading (`ready` still false).
    pub fn is_not_ready(&self) -> bool {
        self.code() == Some("not_ready")
    }

    /// Reads a rejection the bridge produced (`{code, message}` as JSON).
    /// Anything else becomes an `sdk_error` carrying the raw text, so no
    /// failure is ever lost for want of the expected shape.
    pub fn from_rejection_json(json: &str) -> Self {
        #[derive(Deserialize)]
        struct Rejection {
            code: String,
            message: String,
        }
        match serde_json::from_str::<Rejection>(json) {
            Ok(Rejection { code, message }) => Self::Sdk { code, message },
            Err(_) => Self::Sdk {
                code: "sdk_error".to_owned(),
                message: json.to_owned(),
            },
        }
    }

    fn from_rejection(value: &JsValue) -> Self {
        Self::from_rejection_json(&json_text(value))
    }
}

/// `JSON.stringify` of a JS value, falling back to its debug rendering for
/// values JSON cannot express (undefined, functions, cycles).
fn json_text(value: &JsValue) -> String {
    match js_sys::JSON::stringify(value) {
        Ok(text) => text.as_string().unwrap_or_else(|| format!("{value:?}")),
        Err(_) => format!("{value:?}"),
    }
}

/// Awaits a bridge promise, mapping a rejection to `BridgeError`.
async fn settle(promise: js_sys::Promise) -> Result<JsValue, BridgeError> {
    JsFuture::from(promise)
        .await
        .map_err(|rejection| BridgeError::from_rejection(&rejection))
}

/// Awaits a bridge promise that resolves to a string.
async fn settle_string(promise: js_sys::Promise) -> Result<String, BridgeError> {
    let value = settle(promise).await?;
    value
        .as_string()
        .ok_or_else(|| BridgeError::Decode(json_text(&value)))
}

/// The installed bridge object, or `NotLoaded`.
fn bridge() -> Result<ffi::Bridge, BridgeError> {
    let value = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(ffi::BRIDGE_GLOBAL))
        .map_err(|_| BridgeError::NotLoaded)?;
    if value.is_undefined() || value.is_null() {
        return Err(BridgeError::NotLoaded);
    }
    Ok(value.unchecked_into())
}

/// True once `bridge.js` has installed itself.
pub fn is_loaded() -> bool {
    bridge().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bridge_rejection_keeps_the_sdk_code_and_message() {
        let error = BridgeError::from_rejection_json(
            r#"{"code":"exited_auth_flow","message":"User exited"}"#,
        );
        assert_eq!(
            error,
            BridgeError::Sdk {
                code: "exited_auth_flow".to_owned(),
                message: "User exited".to_owned()
            }
        );
    }

    #[test]
    fn an_unexpected_rejection_shape_keeps_its_raw_text() {
        let error = BridgeError::from_rejection_json("\"boom\"");
        assert_eq!(error.code(), Some("sdk_error"));
        assert_eq!(error.to_string(), "\"boom\" (sdk_error)");
    }

    #[test]
    fn closing_the_modal_or_rejecting_in_the_wallet_counts_as_a_cancel() {
        for code in ["exited_auth_flow", "4001", "superseded"] {
            let error = BridgeError::Sdk {
                code: code.to_owned(),
                message: String::new(),
            };
            assert!(error.is_user_cancel(), "{code}");
        }
    }

    #[test]
    fn a_real_failure_is_not_a_cancel() {
        let error = BridgeError::from_rejection_json(
            r#"{"code":"allowlist_rejected","message":"Not allowed"}"#,
        );
        assert!(!error.is_user_cancel());
        assert!(!BridgeError::NotLoaded.is_user_cancel());
    }

    #[test]
    fn not_ready_is_recognised() {
        let error = BridgeError::from_rejection_json(r#"{"code":"not_ready","message":"loading"}"#);
        assert!(error.is_not_ready());
    }
}

//! World ID Selfie Check through IDKit. Replaces the one IDKit touchpoint of
//! `web/src/lib/world-id.ts`,
//! `IDKit.request(config).preset(selfieCheckLegacy({ signal }))`, and the
//! handle it resolves to (`connectorURI`, `pollUntilCompletion`).
//!
//! The bridge uses `@worldcoin/idkit-core`, IDKit's framework-free package
//! (the React `@worldcoin/idkit` wraps the same namespace), so no React is
//! involved on this path.

use postage_core::rp_context::{RpContextWindow, poll_timeout_ms};
use postage_core::world_id_messages::{IdKitErrorCode, describe_world_id_failure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use wasm_bindgen::JsCast;

use super::ffi::SelfieCheckJsHandle;
use super::{BridgeError, bridge, settle, settle_string};
use crate::config::{WorldEnvironment, is_world_app_id};

/// Selfie Check issues World ID 3.0 proofs, which IDKit's v4 default refuses
/// unless legacy proofs are allowed explicitly.
const ALLOW_LEGACY_PROOFS: bool = true;

/// What `POST /api/world/context` returns: IDKit's `RpContext` plus the
/// `action` the server signed it for, which is the action IDKit is asked for.
/// Deserializing it is the shape check `isRpContext` did in TS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpContext {
    pub rp_id: String,
    pub nonce: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub signature: String,
    pub action: String,
}

impl RpContext {
    pub fn from_json(json: &str) -> Result<Self, BridgeError> {
        serde_json::from_str(json).map_err(|error| BridgeError::Decode(error.to_string()))
    }

    /// How long to poll for the World App under this context's signature.
    pub fn poll_timeout_ms(&self) -> u64 {
        poll_timeout_ms(&RpContextWindow::new(self.created_at, self.expires_at))
    }
}

/// Everything one Selfie Check request needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfieCheckRequest<'a> {
    /// `config::WORLD_APP_ID`.
    pub app_id: &'a str,
    pub environment: WorldEnvironment,
    pub rp_context: &'a RpContext,
    /// The challenge token the proof is bound to. Required: an unbound proof
    /// clears any challenge it is pasted into.
    pub signal: &'a str,
}

impl SelfieCheckRequest<'_> {
    /// The `{config, signal}` JSON the bridge feeds to
    /// `IDKit.request(config).preset(selfieCheckLegacy({signal}))`.
    pub fn to_json(&self) -> Result<String, BridgeError> {
        if !is_world_app_id(self.app_id) {
            return Err(BridgeError::InvalidInput(
                "the World app id must start with app_",
            ));
        }
        if self.signal.is_empty() {
            return Err(BridgeError::InvalidInput(
                "a Selfie Check needs a signal to bind the proof to",
            ));
        }
        Ok(json!({
            "config": {
                "app_id": self.app_id,
                "action": self.rp_context.action,
                "rp_context": self.rp_context,
                "allow_legacy_proofs": ALLOW_LEGACY_PROOFS,
                "environment": self.environment,
            },
            "signal": self.signal,
        })
        .to_string())
    }
}

/// Asks IDKit for a Selfie Check. Resolves once the request exists, with the
/// URI the World App must open; the proof comes from `poll_until_completion`.
pub async fn open_selfie_check(
    request: &SelfieCheckRequest<'_>,
) -> Result<SelfieCheckHandle, BridgeError> {
    let request_json = request.to_json()?;
    let handle: SelfieCheckJsHandle = settle(bridge()?.idkit().open_selfie_check(&request_json))
        .await?
        .unchecked_into();
    let connector_uri = handle
        .connector_uri()
        .as_string()
        .ok_or_else(|| BridgeError::Decode("the Selfie Check has no connector URI".to_owned()))?;
    Ok(SelfieCheckHandle {
        connector_uri,
        handle,
    })
}

/// A Selfie Check waiting on the World App.
#[derive(Debug, Clone)]
pub struct SelfieCheckHandle {
    connector_uri: String,
    handle: SelfieCheckJsHandle,
}

impl SelfieCheckHandle {
    /// The link (and QR code payload) that opens this request in World App.
    pub fn connector_uri(&self) -> &str {
        &self.connector_uri
    }

    /// Waits for the World App's answer, up to `timeout_ms` (use
    /// `RpContext::poll_timeout_ms`). Never fails: anything unexpected comes
    /// back as `Failed(generic_error)`, like IDKit's own completion type.
    pub async fn poll_until_completion(&self, timeout_ms: u64) -> SelfieCheckCompletion {
        // Exact for any timeout below 2^53 ms.
        let promise = self.handle.poll_json(timeout_ms as f64);
        match settle_string(promise).await {
            Ok(json) => SelfieCheckCompletion::from_json(&json),
            Err(error) => {
                leptos::logging::error!("Selfie Check polling failed: {error}");
                SelfieCheckCompletion::Failed(IdKitFailure::Known(IdKitErrorCode::GenericError))
            }
        }
    }
}

/// How a Selfie Check ended.
#[derive(Debug, Clone, PartialEq)]
pub enum SelfieCheckCompletion {
    /// IDKit's `IDKitResult`, to forward to `POST /api/world/verify` as
    /// `proof` with World's own field names (key order is not preserved).
    Verified(serde_json::Value),
    Failed(IdKitFailure),
}

impl SelfieCheckCompletion {
    /// Reads IDKit's `{success: true, result} | {success: false, error}`.
    /// An unreadable completion is a `generic_error` failure.
    pub fn from_json(json: &str) -> Self {
        #[derive(Deserialize)]
        struct Completion {
            success: bool,
            result: Option<serde_json::Value>,
            error: Option<String>,
        }
        match serde_json::from_str::<Completion>(json) {
            Ok(Completion {
                success: true,
                result: Some(result),
                ..
            }) => Self::Verified(result),
            Ok(Completion {
                success: false,
                error: Some(code),
                ..
            }) => Self::Failed(IdKitFailure::from_code(&code)),
            _ => Self::Failed(IdKitFailure::Known(IdKitErrorCode::GenericError)),
        }
    }
}

/// An IDKit error code, kept even when this build does not know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdKitFailure {
    Known(IdKitErrorCode),
    Unknown(String),
}

impl IdKitFailure {
    pub fn from_code(code: &str) -> Self {
        IdKitErrorCode::from_wire(code).map_or_else(|| Self::Unknown(code.to_owned()), Self::Known)
    }

    pub fn code(&self) -> &str {
        match self {
            Self::Known(code) => code.as_wire(),
            Self::Unknown(code) => code,
        }
    }

    /// Copy a sender can act on (`describeWorldIdFailure`).
    pub fn message(&self) -> &'static str {
        describe_world_id_failure(self.code())
    }
}

impl std::fmt::Display for IdKitFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postage_core::world_id_messages::GENERIC_WORLD_ID_FAILURE_MESSAGE;

    fn context() -> RpContext {
        RpContext {
            rp_id: "rp_1".to_owned(),
            nonce: "n".to_owned(),
            created_at: 1_000,
            expires_at: 1_300,
            signature: "0xsig".to_owned(),
            action: "clear-challenge".to_owned(),
        }
    }

    #[test]
    fn rp_context_reads_the_world_context_response() {
        let parsed = RpContext::from_json(
            r#"{"rp_id":"rp_1","nonce":"n","created_at":1000,"expires_at":1300,
                "signature":"0xsig","action":"clear-challenge"}"#,
        )
        .unwrap();
        assert_eq!(parsed, context());
    }

    #[test]
    fn rp_context_without_an_action_is_refused() {
        let error = RpContext::from_json(
            r#"{"rp_id":"rp_1","nonce":"n","created_at":1000,"expires_at":1300,"signature":"s"}"#,
        )
        .unwrap_err();
        assert!(matches!(error, BridgeError::Decode(_)));
    }

    #[test]
    fn rp_context_with_a_text_timestamp_is_refused() {
        let error = RpContext::from_json(
            r#"{"rp_id":"r","nonce":"n","created_at":"soon","expires_at":1300,
                "signature":"s","action":"a"}"#,
        )
        .unwrap_err();
        assert!(matches!(error, BridgeError::Decode(_)));
    }

    #[test]
    fn poll_timeout_is_the_signed_window_less_the_margin() {
        assert_eq!(context().poll_timeout_ms(), 270_000);
    }

    #[test]
    fn request_json_names_the_signed_action_and_allows_legacy_proofs() {
        let rp_context = context();
        let request = SelfieCheckRequest {
            app_id: "app_123",
            environment: WorldEnvironment::Sandbox,
            rp_context: &rp_context,
            signal: "token-1",
        };
        let json: serde_json::Value = serde_json::from_str(&request.to_json().unwrap()).unwrap();
        assert_eq!(json["signal"], "token-1");
        assert_eq!(json["config"]["app_id"], "app_123");
        assert_eq!(json["config"]["action"], "clear-challenge");
        assert_eq!(json["config"]["allow_legacy_proofs"], true);
        assert_eq!(json["config"]["environment"], "sandbox");
        assert_eq!(json["config"]["rp_context"]["expires_at"], 1_300);
    }

    #[test]
    fn a_request_without_a_signal_is_refused_before_reaching_idkit() {
        let rp_context = context();
        let request = SelfieCheckRequest {
            app_id: "app_123",
            environment: WorldEnvironment::Production,
            rp_context: &rp_context,
            signal: "",
        };
        assert!(matches!(
            request.to_json(),
            Err(BridgeError::InvalidInput(_))
        ));
    }

    #[test]
    fn a_request_with_a_malformed_app_id_is_refused() {
        let rp_context = context();
        let request = SelfieCheckRequest {
            app_id: "123",
            environment: WorldEnvironment::Production,
            rp_context: &rp_context,
            signal: "t",
        };
        assert!(matches!(
            request.to_json(),
            Err(BridgeError::InvalidInput(_))
        ));
    }

    #[test]
    fn a_successful_completion_carries_the_raw_result() {
        let completion = SelfieCheckCompletion::from_json(
            r#"{"success":true,"result":{"protocol_version":"3.0","nullifier":"0x1"}}"#,
        );
        assert_eq!(
            completion,
            SelfieCheckCompletion::Verified(json!({"protocol_version": "3.0", "nullifier": "0x1"}))
        );
    }

    #[test]
    fn a_failed_completion_maps_to_the_curated_message() {
        let completion =
            SelfieCheckCompletion::from_json(r#"{"success":false,"error":"user_rejected"}"#);
        let SelfieCheckCompletion::Failed(failure) = completion else {
            panic!("expected a failure");
        };
        assert_eq!(failure, IdKitFailure::Known(IdKitErrorCode::UserRejected));
        assert_eq!(
            failure.message(),
            describe_world_id_failure("user_rejected")
        );
    }

    #[test]
    fn an_unknown_error_code_is_kept_and_gets_the_generic_message() {
        let failure = IdKitFailure::from_code("brand_new_code");
        assert_eq!(failure, IdKitFailure::Unknown("brand_new_code".to_owned()));
        assert_eq!(failure.message(), GENERIC_WORLD_ID_FAILURE_MESSAGE);
        assert_eq!(failure.to_string(), "brand_new_code");
    }

    #[test]
    fn an_unreadable_completion_is_a_generic_failure() {
        for json in ["null", "{}", r#"{"success":true}"#, "not json"] {
            assert_eq!(
                SelfieCheckCompletion::from_json(json),
                SelfieCheckCompletion::Failed(IdKitFailure::Known(IdKitErrorCode::GenericError)),
                "{json}"
            );
        }
    }
}

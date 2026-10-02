//! Sender-safe copy for a World ID failure, keyed by the code that named it.
//!
//! Browser-safe on purpose: pure data and lookups, no I/O, so the web crate and
//! the server read the same sentences and a request that fails the same way is
//! described the same way whichever side caught it.

use serde::{Deserialize, Serialize};

/// What every failure outside the curated table gets. One constant, so the
/// fallbacks of every caller are the same line by construction.
pub const GENERIC_WORLD_ID_FAILURE_MESSAGE: &str =
    "Verification failed. Try again, or pay instead.";

const CLOSED_BEFORE_FINISHING: &str =
    "You closed the World App before finishing. Try again when you're ready.";
const COULD_NOT_VERIFY: &str =
    "World ID could not verify you for this. Nothing was charged or sent.";
const REQUEST_EXPIRED: &str = "That took too long and the request expired. Try again.";
const CONNECTION_FAILED: &str =
    "Could not reach the World App. Check your connection and try again.";
const SELFIE_CHECK_UNAVAILABLE: &str =
    "Selfie Check isn't available for this app yet. Pay instead, or try again later.";
// Unlike every other bucket, retrying does not merely risk failing again:
// World's own limit is spent for good the moment this fires, so offering
// "try again" would be advice this sender cannot follow. Paying is the only
// door this message may honestly point at.
const ALREADY_USED: &str =
    "This World ID has already been used. Nothing was charged or sent. Pay instead.";

/// The error codes IDKit hands a host app, spelled exactly as they arrive on
/// the wire (`IDKitErrorCodes` in `@worldcoin/idkit-core`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IdKitErrorCode {
    #[serde(rename = "user_rejected")]
    UserRejected,
    #[serde(rename = "verification_rejected")]
    VerificationRejected,
    #[serde(rename = "credential_unavailable")]
    CredentialUnavailable,
    #[serde(rename = "feature_unavailable")]
    FeatureUnavailable,
    #[serde(rename = "world_id_4_not_available")]
    WorldId4NotAvailable,
    #[serde(rename = "world_id_3_not_available")]
    WorldId3NotAvailable,
    #[serde(rename = "malformed_request")]
    MalformedRequest,
    #[serde(rename = "invalid_network")]
    InvalidNetwork,
    #[serde(rename = "inclusion_proof_pending")]
    InclusionProofPending,
    #[serde(rename = "inclusion_proof_failed")]
    InclusionProofFailed,
    #[serde(rename = "unexpected_response")]
    UnexpectedResponse,
    #[serde(rename = "connection_failed")]
    ConnectionFailed,
    #[serde(rename = "max_verifications_reached")]
    MaxVerificationsReached,
    #[serde(rename = "failed_by_host_app")]
    FailedByHostApp,
    #[serde(rename = "user_presence_failed")]
    UserPresenceFailed,
    #[serde(rename = "invalid_rp_signature")]
    InvalidRpSignature,
    #[serde(rename = "nullifier_replayed")]
    NullifierReplayed,
    #[serde(rename = "duplicate_nonce")]
    DuplicateNonce,
    #[serde(rename = "unknown_rp")]
    UnknownRp,
    #[serde(rename = "inactive_rp")]
    InactiveRp,
    #[serde(rename = "timestamp_too_old")]
    TimestampTooOld,
    #[serde(rename = "timestamp_too_far_in_future")]
    TimestampTooFarInFuture,
    #[serde(rename = "invalid_timestamp")]
    InvalidTimestamp,
    #[serde(rename = "rp_signature_expired")]
    RpSignatureExpired,
    #[serde(rename = "identity_attributes_not_matched")]
    IdentityAttributesNotMatched,
    #[serde(rename = "generic_error")]
    GenericError,
    #[serde(rename = "invalid_rp_id_format")]
    InvalidRpIdFormat,
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "cancelled")]
    Cancelled,
}

impl IdKitErrorCode {
    /// Every code, in the order IDKit declares them.
    pub const ALL: [Self; 29] = [
        Self::UserRejected,
        Self::VerificationRejected,
        Self::CredentialUnavailable,
        Self::FeatureUnavailable,
        Self::WorldId4NotAvailable,
        Self::WorldId3NotAvailable,
        Self::MalformedRequest,
        Self::InvalidNetwork,
        Self::InclusionProofPending,
        Self::InclusionProofFailed,
        Self::UnexpectedResponse,
        Self::ConnectionFailed,
        Self::MaxVerificationsReached,
        Self::FailedByHostApp,
        Self::UserPresenceFailed,
        Self::InvalidRpSignature,
        Self::NullifierReplayed,
        Self::DuplicateNonce,
        Self::UnknownRp,
        Self::InactiveRp,
        Self::TimestampTooOld,
        Self::TimestampTooFarInFuture,
        Self::InvalidTimestamp,
        Self::RpSignatureExpired,
        Self::IdentityAttributesNotMatched,
        Self::GenericError,
        Self::InvalidRpIdFormat,
        Self::Timeout,
        Self::Cancelled,
    ];

    /// The string IDKit emits for this code.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::UserRejected => "user_rejected",
            Self::VerificationRejected => "verification_rejected",
            Self::CredentialUnavailable => "credential_unavailable",
            Self::FeatureUnavailable => "feature_unavailable",
            Self::WorldId4NotAvailable => "world_id_4_not_available",
            Self::WorldId3NotAvailable => "world_id_3_not_available",
            Self::MalformedRequest => "malformed_request",
            Self::InvalidNetwork => "invalid_network",
            Self::InclusionProofPending => "inclusion_proof_pending",
            Self::InclusionProofFailed => "inclusion_proof_failed",
            Self::UnexpectedResponse => "unexpected_response",
            Self::ConnectionFailed => "connection_failed",
            Self::MaxVerificationsReached => "max_verifications_reached",
            Self::FailedByHostApp => "failed_by_host_app",
            Self::UserPresenceFailed => "user_presence_failed",
            Self::InvalidRpSignature => "invalid_rp_signature",
            Self::NullifierReplayed => "nullifier_replayed",
            Self::DuplicateNonce => "duplicate_nonce",
            Self::UnknownRp => "unknown_rp",
            Self::InactiveRp => "inactive_rp",
            Self::TimestampTooOld => "timestamp_too_old",
            Self::TimestampTooFarInFuture => "timestamp_too_far_in_future",
            Self::InvalidTimestamp => "invalid_timestamp",
            Self::RpSignatureExpired => "rp_signature_expired",
            Self::IdentityAttributesNotMatched => "identity_attributes_not_matched",
            Self::GenericError => "generic_error",
            Self::InvalidRpIdFormat => "invalid_rp_id_format",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
        }
    }

    /// The code a wire string names, or `None` for anything IDKit never emits.
    pub fn from_wire(wire: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|code| code.as_wire() == wire)
    }

    /// The curated copy for this code, or `None` when it has none. Most of the
    /// enum needs none and falls to the generic message.
    pub const fn curated_message(self) -> Option<&'static str> {
        match self {
            Self::UserRejected | Self::Cancelled => Some(CLOSED_BEFORE_FINISHING),
            Self::VerificationRejected
            | Self::NullifierReplayed
            | Self::IdentityAttributesNotMatched => Some(COULD_NOT_VERIFY),
            Self::CredentialUnavailable
            | Self::FeatureUnavailable
            | Self::WorldId4NotAvailable
            | Self::WorldId3NotAvailable => Some(SELFIE_CHECK_UNAVAILABLE),
            Self::ConnectionFailed => Some(CONNECTION_FAILED),
            Self::MaxVerificationsReached => Some(ALREADY_USED),
            Self::RpSignatureExpired | Self::Timeout => Some(REQUEST_EXPIRED),
            _ => None,
        }
    }
}

/// The curated copy for a failure `code` as it arrives on the wire, or `None`
/// when the table has never heard of it.
///
/// Deliberately not folded together with the generic message: a caller that has
/// to say whether the code was recognised can then tell the two apart outright.
/// Beyond IDKit's own codes this knows two older names World's verify endpoint
/// used for the verification-limit failure, which IDKit never issues.
pub fn world_id_failure_message(code: &str) -> Option<&'static str> {
    match code {
        "exceeded_max_verifications" | "already_verified" => Some(ALREADY_USED),
        other => IdKitErrorCode::from_wire(other)?.curated_message(),
    }
}

/// The message to show for `code`: the curated copy, or the generic fallback.
pub fn describe_world_id_failure(code: &str) -> &'static str {
    world_id_failure_message(code).unwrap_or(GENERIC_WORLD_ID_FAILURE_MESSAGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_idkit_code_round_trips_through_its_wire_string() {
        for code in IdKitErrorCode::ALL {
            assert_eq!(IdKitErrorCode::from_wire(code.as_wire()), Some(code));
            let json = serde_json::to_string(&code).unwrap();
            assert_eq!(json, format!("\"{}\"", code.as_wire()));
            assert_eq!(serde_json::from_str::<IdKitErrorCode>(&json).unwrap(), code);
        }
    }

    #[test]
    fn the_code_set_matches_the_idkit_declaration_count() {
        assert_eq!(IdKitErrorCode::ALL.len(), 29);
    }

    #[test]
    fn wire_strings_are_unique() {
        let mut wires: Vec<&str> = IdKitErrorCode::ALL.iter().map(|c| c.as_wire()).collect();
        wires.sort_unstable();
        wires.dedup();
        assert_eq!(wires.len(), IdKitErrorCode::ALL.len());
    }

    #[test]
    fn every_code_maps_to_its_curated_message_or_none() {
        let expected: &[(&str, Option<&str>)] = &[
            ("user_rejected", Some(CLOSED_BEFORE_FINISHING)),
            ("verification_rejected", Some(COULD_NOT_VERIFY)),
            ("credential_unavailable", Some(SELFIE_CHECK_UNAVAILABLE)),
            ("feature_unavailable", Some(SELFIE_CHECK_UNAVAILABLE)),
            ("world_id_4_not_available", Some(SELFIE_CHECK_UNAVAILABLE)),
            ("world_id_3_not_available", Some(SELFIE_CHECK_UNAVAILABLE)),
            ("malformed_request", None),
            ("invalid_network", None),
            ("inclusion_proof_pending", None),
            ("inclusion_proof_failed", None),
            ("unexpected_response", None),
            ("connection_failed", Some(CONNECTION_FAILED)),
            ("max_verifications_reached", Some(ALREADY_USED)),
            ("failed_by_host_app", None),
            ("user_presence_failed", None),
            ("invalid_rp_signature", None),
            ("nullifier_replayed", Some(COULD_NOT_VERIFY)),
            ("duplicate_nonce", None),
            ("unknown_rp", None),
            ("inactive_rp", None),
            ("timestamp_too_old", None),
            ("timestamp_too_far_in_future", None),
            ("invalid_timestamp", None),
            ("rp_signature_expired", Some(REQUEST_EXPIRED)),
            ("identity_attributes_not_matched", Some(COULD_NOT_VERIFY)),
            ("generic_error", None),
            ("invalid_rp_id_format", None),
            ("timeout", Some(REQUEST_EXPIRED)),
            ("cancelled", Some(CLOSED_BEFORE_FINISHING)),
            ("exceeded_max_verifications", Some(ALREADY_USED)),
            ("already_verified", Some(ALREADY_USED)),
        ];
        for (code, message) in expected {
            assert_eq!(world_id_failure_message(code), *message, "{code}");
        }
        assert_eq!(expected.len(), IdKitErrorCode::ALL.len() + 2);
    }

    #[test]
    fn curated_messages_read_as_the_sender_facing_copy() {
        assert_eq!(
            describe_world_id_failure("user_rejected"),
            "You closed the World App before finishing. Try again when you're ready."
        );
        assert_eq!(
            describe_world_id_failure("max_verifications_reached"),
            "This World ID has already been used. Nothing was charged or sent. Pay instead."
        );
        assert_eq!(
            describe_world_id_failure("timeout"),
            "That took too long and the request expired. Try again."
        );
    }

    #[test]
    fn codes_with_no_curated_copy_fall_back_to_the_generic_message() {
        for code in [
            "generic_error",
            "malformed_request",
            "never_heard_of_it",
            "",
        ] {
            assert_eq!(world_id_failure_message(code), None, "{code}");
            assert_eq!(
                describe_world_id_failure(code),
                GENERIC_WORLD_ID_FAILURE_MESSAGE
            );
        }
    }

    #[test]
    fn names_inherited_by_javascript_objects_are_just_unknown_codes() {
        for code in ["toString", "__proto__", "constructor"] {
            assert_eq!(world_id_failure_message(code), None, "{code}");
        }
    }

    #[test]
    fn lookups_are_case_sensitive_like_the_wire() {
        assert_eq!(world_id_failure_message("User_Rejected"), None);
    }
}

//! The relying party's signature over a World ID 4.0 proof request
//! (`signRequest` from `@worldcoin/idkit-server` 1.1.1), so the server can
//! sign an rp_context without the SDK.
//!
//! What is signed is a 49-byte header, plus the action's field hash when there
//! is one: a version byte, the nonce, and the validity window as two big-endian
//! `u64`s. It is signed as an EIP-191 personal message with secp256k1, RFC 6979
//! deterministic and low-s, which is what `@noble/secp256k1` produces.
//!
//! Pure: the clock and the 32 random bytes the nonce is hashed from are passed
//! in, so this runs under wasm and the golden vectors pin it exactly.

use alloy_primitives::{eip191_hash_message, keccak256};
use k256::ecdsa::SigningKey;

use crate::js_number::to_js_string;

/// The window the SDK signs when none is given, in seconds.
pub const DEFAULT_TTL_SECONDS: u64 = 300;

/// The first byte of every signed message.
const RP_SIGNATURE_MESSAGE_VERSION: u8 = 1;

/// Version byte, 32-byte nonce, then the two `u64` timestamps.
const HEADER_BYTES: usize = 1 + 32 + 8 + 8;

/// Hex digits in a 32-byte private key.
const KEY_HEX_DIGITS: usize = 64;

/// Why a request could not be signed, worded as the SDK words it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RpSignError {
    #[error("Invalid signing key: contains non-hex characters")]
    NonHexKey,
    /// The SDK reports the length in bytes, as `hex digits / 2`, which is a
    /// fraction for an odd count.
    #[error("Invalid signing key: expected 32 bytes (64 hex chars), got {0} bytes")]
    KeyLength(String),
    /// Thirty-two bytes that are not a secp256k1 scalar: zero, or at or above
    /// the curve order.
    #[error("Invalid signing key: not a valid secp256k1 private key")]
    InvalidKey,
    #[error("Could not sign the request")]
    Signing,
}

/// A signed rp_context: what the browser hands IDKit and World checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpSignature {
    /// 65 bytes, `r || s || v` with `v` as 27 or 28, `0x`-prefixed hex.
    pub sig: String,
    /// The 32-byte field element the signature covers, `0x`-prefixed hex.
    pub nonce: String,
    pub created_at: u64,
    pub expires_at: u64,
}

/// Signs a proof request for `action` (or for none), valid for `ttl` seconds
/// from `now`. `random` stands in for the SDK's `crypto.getRandomValues`.
///
/// The key is read as the SDK reads it: an optional lowercase `0x`, then
/// exactly 64 hex digits of either case.
pub fn sign_request(
    signing_key_hex: &str,
    action: Option<&str>,
    ttl: u64,
    now: u64,
    random: &[u8; 32],
) -> Result<RpSignature, RpSignError> {
    let key = signing_key(signing_key_hex)?;

    let nonce = hash_to_field(random);
    let created_at = now;
    let expires_at = created_at.saturating_add(ttl);
    let message = rp_signature_message(&nonce, created_at, expires_at, action);

    let hash = eip191_hash_message(&message);
    let (signature, recovery) = key
        .sign_prehash_recoverable(hash.as_slice())
        .map_err(|_| RpSignError::Signing)?;
    let mut sig = [0u8; 65];
    sig[..64].copy_from_slice(&signature.to_bytes());
    sig[64] = 27 + u8::from(recovery.is_y_odd());

    Ok(RpSignature {
        sig: format!("0x{}", hex::encode(sig)),
        nonce: format!("0x{}", hex::encode(nonce)),
        created_at,
        expires_at,
    })
}

/// The bytes the signature is over (`computeRpSignatureMessage`).
pub fn rp_signature_message(
    nonce: &[u8; 32],
    created_at: u64,
    expires_at: u64,
    action: Option<&str>,
) -> Vec<u8> {
    let mut message = Vec::with_capacity(HEADER_BYTES + 32);
    message.push(RP_SIGNATURE_MESSAGE_VERSION);
    message.extend_from_slice(nonce);
    message.extend_from_slice(&created_at.to_be_bytes());
    message.extend_from_slice(&expires_at.to_be_bytes());
    if let Some(action) = action {
        message.extend_from_slice(&hash_to_field(action.as_bytes()));
    }
    message
}

/// Keccak-256 shifted right by one byte, so the value fits the proof system's
/// field: a zero byte, then the hash's first 31.
pub fn hash_to_field(input: &[u8]) -> [u8; 32] {
    let hash = keccak256(input);
    let mut field = [0u8; 32];
    field[1..].copy_from_slice(&hash[..31]);
    field
}

fn signing_key(signing_key_hex: &str) -> Result<SigningKey, RpSignError> {
    let key_hex = signing_key_hex
        .strip_prefix("0x")
        .unwrap_or(signing_key_hex);
    if key_hex.is_empty() || !key_hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RpSignError::NonHexKey);
    }
    if key_hex.len() != KEY_HEX_DIGITS {
        return Err(RpSignError::KeyLength(to_js_string(
            key_hex.len() as f64 / 2.0,
        )));
    }
    let bytes = hex::decode(key_hex).map_err(|_| RpSignError::NonHexKey)?;
    SigningKey::from_slice(&bytes).map_err(|_| RpSignError::InvalidKey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    const GOLDEN: &str = include_str!("../../../fixtures/golden/rp-sign-request.json");
    const KEY: &str = "0x2222222222222222222222222222222222222222222222222222222222222222";

    #[derive(Deserialize)]
    struct Fixture {
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        name: String,
        input: Input,
        output: Output,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        signing_key_hex: String,
        action: Option<String>,
        ttl: Option<u64>,
        now: u64,
        random_bytes_hex: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Output {
        sig: String,
        nonce: String,
        created_at: u64,
        expires_at: u64,
    }

    fn random(hex_text: &str) -> [u8; 32] {
        hex::decode(hex_text).unwrap().try_into().unwrap()
    }

    #[test]
    fn every_golden_vector_is_reproduced_byte_for_byte() {
        let fixture: Fixture = serde_json::from_str(GOLDEN).unwrap();
        assert_eq!(fixture.cases.len(), 8);
        for case in fixture.cases {
            let input = case.input;
            let signed = sign_request(
                &input.signing_key_hex,
                input.action.as_deref(),
                input.ttl.unwrap_or(DEFAULT_TTL_SECONDS),
                input.now,
                &random(&input.random_bytes_hex),
            )
            .unwrap();
            assert_eq!(
                signed,
                RpSignature {
                    sig: case.output.sig,
                    nonce: case.output.nonce,
                    created_at: case.output.created_at,
                    expires_at: case.output.expires_at,
                },
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn the_nonce_is_a_field_element_with_a_leading_zero_byte() {
        let signed = sign_request(KEY, Some("a"), 300, 0, &[0xff; 32]).unwrap();
        assert!(signed.nonce.starts_with("0x00"), "{}", signed.nonce);
        assert_eq!(signed.nonce.len(), 66);
    }

    #[test]
    fn the_message_without_an_action_is_the_bare_header() {
        let message = rp_signature_message(&[7; 32], 1, 2, None);
        assert_eq!(message.len(), HEADER_BYTES);
        assert_eq!(message[0], 1);
        assert_eq!(&message[33..41], &1u64.to_be_bytes());
        assert_eq!(&message[41..49], &2u64.to_be_bytes());
    }

    #[test]
    fn the_message_with_an_action_carries_its_field_hash() {
        let message = rp_signature_message(&[7; 32], 1, 2, Some("act"));
        assert_eq!(message.len(), HEADER_BYTES + 32);
        assert_eq!(&message[HEADER_BYTES..], &hash_to_field(b"act"));
    }

    #[test]
    fn a_key_with_non_hex_characters_is_refused_as_the_sdk_words_it() {
        let error = sign_request("0xzz", None, 300, 0, &[0; 32]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Invalid signing key: contains non-hex characters"
        );
    }

    #[test]
    fn an_empty_key_is_refused_as_non_hex() {
        assert_eq!(
            sign_request("0x", None, 300, 0, &[0; 32]).unwrap_err(),
            RpSignError::NonHexKey
        );
    }

    #[test]
    fn a_short_key_reports_its_length_in_bytes_even_when_fractional() {
        let error = sign_request("0xabc", None, 300, 0, &[0; 32]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Invalid signing key: expected 32 bytes (64 hex chars), got 1.5 bytes"
        );
    }

    #[test]
    fn an_uppercase_prefix_is_not_stripped() {
        let key = format!("0X{}", "22".repeat(32));
        assert_eq!(
            sign_request(&key, None, 300, 0, &[0; 32]).unwrap_err(),
            RpSignError::NonHexKey
        );
    }

    #[test]
    fn a_zero_key_is_not_a_private_key() {
        let key = format!("0x{}", "00".repeat(32));
        assert_eq!(
            sign_request(&key, None, 300, 0, &[0; 32]).unwrap_err(),
            RpSignError::InvalidKey
        );
    }
}

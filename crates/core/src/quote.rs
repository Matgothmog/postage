//! The price a sender is offered, bound to one message and signed so the
//! escrow will honour it (`web/src/lib/quote.ts`).
//!
//! Signing is RFC 6979 deterministic ECDSA over secp256k1 with a low-s
//! signature, which is what viem's `signTypedData` produces, so the same key
//! and quote give the same bytes here as they did in TypeScript. Nothing in
//! this module reads a clock or a random source: `now` is passed in, and the
//! signature needs no randomness, which is also what keeps it buildable for
//! `wasm32-unknown-unknown`.

use std::fmt;

use alloy_primitives::{Address, B256, U256, aliases::U40, keccak256};
use alloy_sol_types::{Eip712Domain, SolStruct, eip712_domain, sol};
use hmac::Mac;
use k256::ecdsa::SigningKey;
use postage_shared::Tier;

use crate::contracts::{ARC_TESTNET, POSTAGE_ESCROW};
use crate::quote_types::QuoteFields;
use crate::secret::{MessageIdSecret, SecretError};

/// How long a sender has to act on a price before it must be requoted. Short
/// enough that a cheap quote cannot be banked, long enough to click a link and
/// find a wallet.
pub const QUOTE_TTL_SECONDS: i64 = 24 * 60 * 60;

/// The escrow takes the deadline as a `uint40`.
const MAX_EXPIRES_AT: u64 = (1 << 40) - 1;

const PRIVATE_KEY_HEX_DIGITS: usize = 64;
const ADDRESS_HEX_DIGITS: usize = 40;

sol! {
    /// The typed-data struct the escrow recovers the classifier from. Field
    /// names, order and widths are the type hash, so they are the contract's,
    /// not ours to tidy.
    #[derive(Debug, PartialEq, Eq)]
    struct Quote {
        bytes32 messageId;
        address inbox;
        uint8 tier;
        uint256 amount;
        uint40 expiresAt;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuoteError {
    /// viem refuses an address that is neither all lowercase nor a valid
    /// EIP-55 checksum, so a mistyped mixed-case inbox never gets a quote.
    #[error("Address \"{0}\" is invalid")]
    InvalidAddress(String),
    #[error("quote expiry {0} does not fit the escrow's uint40")]
    ExpiryOutOfRange(i128),
    #[error("the classifier private key is not a valid secp256k1 key")]
    InvalidPrivateKey,
    #[error("the quote could not be signed")]
    Signing,
}

/// The classifier's key. `Debug` never shows it.
#[derive(Clone)]
pub struct QuoteSigner(SigningKey);

impl QuoteSigner {
    /// Reads `0x` followed by 64 hex digits, the only spelling viem's
    /// `privateKeyToAccount` reads correctly; zero and anything at or above
    /// the curve order are refused.
    pub fn from_hex(key: &str) -> Result<Self, QuoteError> {
        let bytes = private_key_bytes(key)?;
        SigningKey::from_slice(&bytes)
            .map(Self)
            .map_err(|_| QuoteError::InvalidPrivateKey)
    }

    /// The address the escrow must have registered as the classifier for
    /// these signatures to verify.
    pub fn address(&self) -> Address {
        let point = self.0.verifying_key().to_encoded_point(false);
        // Uncompressed SEC1 is a 0x04 tag followed by x and y.
        let hash = keccak256(&point.as_bytes()[1..]);
        Address::from_slice(&hash[12..])
    }

    /// 65 bytes of r, s and v, with v as 27 or 28.
    fn sign_hash(&self, hash: &B256) -> Result<[u8; 65], QuoteError> {
        let (signature, recovery) = self
            .0
            .sign_prehash_recoverable(hash.as_slice())
            .map_err(|_| QuoteError::Signing)?;
        let mut bytes = [0u8; 65];
        bytes[..64].copy_from_slice(&signature.to_bytes());
        bytes[64] = 27 + u8::from(recovery.is_y_odd());
        Ok(bytes)
    }
}

impl fmt::Debug for QuoteSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("QuoteSigner(<redacted>)")
    }
}

/// A quote fresh out of [`sign_quote`], typed. It stops being typed the
/// moment it crosses a JSON boundary, which is what [`QuoteFields`] describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedQuote {
    pub message_id: B256,
    /// Exactly as the caller spelled it, which viem also hands back.
    pub inbox: String,
    pub tier: Tier,
    pub amount: U256,
    pub expires_at: u64,
    pub signature: [u8; 65],
}

impl SignedQuote {
    /// The stored and wire shape: hex strings and a decimal amount.
    pub fn fields(&self) -> QuoteFields {
        QuoteFields {
            message_id: self.message_id.to_string(),
            inbox: self.inbox.clone(),
            tier: self.tier.as_str().to_owned(),
            amount: self.amount.to_string(),
            expires_at: self.expires_at,
            signature: format!("0x{}", hex::encode(self.signature)),
        }
    }
}

/// The 32 bytes of a key written as `0x` and 64 hex digits, the only
/// spelling viem's `privateKeyToAccount` reads correctly: it drops the first
/// two characters whatever they are. Whether the bytes are a valid scalar is
/// the signer's to decide.
pub fn private_key_bytes(key: &str) -> Result<[u8; 32], QuoteError> {
    let digits = key
        .strip_prefix("0x")
        .filter(|digits| digits.len() == PRIVATE_KEY_HEX_DIGITS)
        .ok_or(QuoteError::InvalidPrivateKey)?;
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(digits, &mut bytes).map_err(|_| QuoteError::InvalidPrivateKey)?;
    Ok(bytes)
}

/// The domain the escrow verifies quotes under.
pub fn quote_domain() -> Eip712Domain {
    eip712_domain! {
        name: "Postage",
        version: "2",
        chain_id: ARC_TESTNET.id,
        verifying_contract: POSTAGE_ESCROW,
    }
}

/// An address as viem's `isAddress` accepts it: `0x` and 40 hex digits,
/// either all lowercase or carrying a valid EIP-55 checksum.
pub fn parse_address(text: &str) -> Result<Address, QuoteError> {
    let invalid = || QuoteError::InvalidAddress(text.to_owned());
    let digits = text
        .strip_prefix("0x")
        .filter(|digits| {
            digits.len() == ADDRESS_HEX_DIGITS && digits.bytes().all(|b| b.is_ascii_hexdigit())
        })
        .ok_or_else(invalid)?;
    if !digits.bytes().any(|b| b.is_ascii_uppercase()) {
        return text.parse().map_err(|_| invalid());
    }
    Address::parse_checksummed(text, None).map_err(|_| invalid())
}

/// Derived from the message, so a quote is bound to the mail it was issued for
/// and cannot be spent on a different one.
///
/// Keyed rather than hashed plainly, because this id is published onchain and
/// every part of the message it names is guessable: the handle is public by
/// design, the sender is a short list, the subject of paid mail is templated,
/// and the second it arrived is bounded by the block. A bare keccak of those
/// is a preimage anyone can search, which would turn the ledger into a record
/// of who writes to whom. Under HMAC it is a commitment instead.
pub fn message_id_for(
    secret: &MessageIdSecret,
    sender: &str,
    handle: &str,
    subject: &str,
    received_at: i64,
) -> Result<B256, SecretError> {
    let preimage = format!(
        "{}|{}|{subject}|{received_at}",
        sender.to_lowercase(),
        handle.to_lowercase()
    );
    let mut mac = secret.message_id_mac()?;
    mac.update(preimage.as_bytes());
    Ok(B256::from_slice(&mac.finalize().into_bytes()))
}

/// Prices `message_id` for `inbox` at `amount`, valid for a day from `now`
/// (epoch seconds).
pub fn sign_quote(
    signer: &QuoteSigner,
    message_id: B256,
    inbox: &str,
    tier: Tier,
    amount: U256,
    now: i64,
) -> Result<SignedQuote, QuoteError> {
    let inbox_address = parse_address(inbox)?;
    let expires_at = expiry_after(now)?;
    let typed = Quote {
        messageId: message_id,
        inbox: inbox_address,
        tier: tier.index(),
        amount,
        expiresAt: U40::from(expires_at),
    };
    let signature = signer.sign_hash(&typed.eip712_signing_hash(&quote_domain()))?;

    Ok(SignedQuote {
        message_id,
        inbox: inbox.to_owned(),
        tier,
        amount,
        expires_at,
        signature,
    })
}

/// `now + QUOTE_TTL_SECONDS`, refused when it falls outside the `uint40` the
/// escrow takes, as viem refuses to encode it.
fn expiry_after(now: i64) -> Result<u64, QuoteError> {
    let expiry = i128::from(now) + i128::from(QUOTE_TTL_SECONDS);
    u64::try_from(expiry)
        .ok()
        .filter(|seconds| *seconds <= MAX_EXPIRES_AT)
        .ok_or(QuoteError::ExpiryOutOfRange(expiry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
    use serde_json::Value;

    const SIGN_QUOTE: &str = include_str!("../../../fixtures/golden/sign-quote.json");
    const MESSAGE_ID: &str = include_str!("../../../fixtures/golden/message-id.json");

    fn golden(raw: &str) -> Value {
        serde_json::from_str(raw).unwrap()
    }

    fn golden_signer() -> QuoteSigner {
        let config = golden(SIGN_QUOTE)["config"].clone();
        QuoteSigner::from_hex(config["classifierPrivateKey"].as_str().unwrap()).unwrap()
    }

    fn sign_case(input: &Value) -> Result<SignedQuote, QuoteError> {
        sign_quote(
            &golden_signer(),
            input["messageId"].as_str().unwrap().parse().unwrap(),
            input["inbox"].as_str().unwrap(),
            Tier::from_name(input["tier"].as_str().unwrap()).unwrap(),
            input["amount"].as_str().unwrap().parse().unwrap(),
            input["now"].as_i64().unwrap(),
        )
    }

    fn sample(inbox: &str, now: i64) -> Result<SignedQuote, QuoteError> {
        sign_quote(
            &golden_signer(),
            B256::repeat_byte(0xab),
            inbox,
            Tier::Commercial,
            U256::from(1u8),
            now,
        )
    }

    #[test]
    fn every_sign_quote_golden_case_matches_byte_for_byte() {
        let cases = golden(SIGN_QUOTE)["cases"].as_array().unwrap().clone();
        assert_eq!(cases.len(), 8);
        for case in cases {
            let signed = sign_case(&case["input"]).unwrap();
            assert_eq!(
                serde_json::to_value(signed.fields()).unwrap(),
                case["output"],
                "{}",
                case["name"]
            );
        }
    }

    #[test]
    fn every_message_id_golden_case_matches_byte_for_byte() {
        let file = golden(MESSAGE_ID);
        let secret =
            MessageIdSecret::new(file["config"]["messageIdSecret"].as_str().unwrap()).unwrap();
        let cases = file["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 9);
        for case in cases {
            let input = &case["input"];
            let id = message_id_for(
                &secret,
                input["sender"].as_str().unwrap(),
                input["handle"].as_str().unwrap(),
                input["subject"].as_str().unwrap(),
                input["receivedAt"].as_i64().unwrap(),
            )
            .unwrap();
            assert_eq!(id.to_string(), case["output"], "{}", case["name"]);
        }
    }

    #[test]
    fn the_signer_address_matches_the_one_viem_derived() {
        let expected = golden(SIGN_QUOTE)["config"]["signerAddress"].clone();
        assert_eq!(golden_signer().address().to_checksum(None), expected);
    }

    #[test]
    fn a_signature_recovers_to_the_signer_under_the_escrow_domain() {
        let signed = sample("0x1234567890abcdef1234567890abcdef12345678", 1_800_000_000).unwrap();
        let typed = Quote {
            messageId: signed.message_id,
            inbox: parse_address(&signed.inbox).unwrap(),
            tier: signed.tier.index(),
            amount: signed.amount,
            expiresAt: U40::from(signed.expires_at),
        };
        let hash = typed.eip712_signing_hash(&quote_domain());
        let signature = Signature::from_slice(&signed.signature[..64]).unwrap();
        let recovery = RecoveryId::from_byte(signed.signature[64] - 27).unwrap();
        let key =
            VerifyingKey::recover_from_prehash(hash.as_slice(), &signature, recovery).unwrap();

        assert_eq!(key, *golden_signer().0.verifying_key());
    }

    #[test]
    fn the_quote_type_string_is_the_one_the_escrow_hashes() {
        assert_eq!(
            Quote::eip712_encode_type(),
            "Quote(bytes32 messageId,address inbox,uint8 tier,uint256 amount,uint40 expiresAt)"
        );
    }

    #[test]
    fn an_inbox_with_a_broken_checksum_is_refused() {
        let broken = "0x1234567890ABcdEF1234567890aBcdef12345678";
        assert_eq!(
            sample(broken, 0),
            Err(QuoteError::InvalidAddress(broken.to_owned()))
        );
    }

    #[test]
    fn an_all_uppercase_inbox_is_refused_like_any_other_bad_checksum() {
        let upper = "0x1234567890ABCDEF1234567890ABCDEF12345678";
        assert_eq!(
            sample(upper, 0),
            Err(QuoteError::InvalidAddress(upper.to_owned()))
        );
    }

    #[test]
    fn an_inbox_that_is_not_an_address_is_refused() {
        for inbox in [
            "",
            "0x",
            "1234567890abcdef1234567890abcdef12345678",
            "0X1234567890abcdef1234567890abcdef12345678",
            "0x1234567890abcdef1234567890abcdef1234567",
            "0x1234567890abcdef1234567890abcdef123456789",
            "0x1234567890abcdef1234567890abcdef1234567g",
        ] {
            assert!(
                matches!(sample(inbox, 0), Err(QuoteError::InvalidAddress(_))),
                "{inbox:?}"
            );
        }
    }

    #[test]
    fn a_checksummed_inbox_is_echoed_as_given() {
        let checksummed = "0x1234567890AbcdEF1234567890aBcdef12345678";
        assert_eq!(sample(checksummed, 0).unwrap().inbox, checksummed);
    }

    #[test]
    fn an_expiry_one_past_uint40_is_refused() {
        let now = (1i64 << 40) - QUOTE_TTL_SECONDS;
        let inbox = "0x1234567890abcdef1234567890abcdef12345678";
        assert_eq!(
            sample(inbox, now),
            Err(QuoteError::ExpiryOutOfRange(1 << 40))
        );
        assert!(sample(inbox, now - 1).is_ok());
    }

    #[test]
    fn a_negative_expiry_is_refused() {
        let inbox = "0x1234567890abcdef1234567890abcdef12345678";
        assert_eq!(
            sample(inbox, -QUOTE_TTL_SECONDS - 1),
            Err(QuoteError::ExpiryOutOfRange(-1))
        );
        assert_eq!(sample(inbox, -QUOTE_TTL_SECONDS).unwrap().expires_at, 0);
    }

    #[test]
    fn a_private_key_viem_would_misread_is_refused() {
        let digits = "11".repeat(32);
        for key in [
            digits.clone(),
            format!("0x{}", &digits[2..]),
            format!("0x{digits}00"),
            format!("0x{}", "zz".repeat(32)),
            format!("0x{}", "00".repeat(32)),
            format!("0x{}", "ff".repeat(32)),
        ] {
            assert_eq!(
                QuoteSigner::from_hex(&key).map(|_| ()),
                Err(QuoteError::InvalidPrivateKey),
                "{key}"
            );
        }
    }

    #[test]
    fn debug_output_never_shows_the_key() {
        assert_eq!(format!("{:?}", golden_signer()), "QuoteSigner(<redacted>)");
    }

    #[test]
    fn a_different_received_at_gives_a_different_message_id() {
        let secret = MessageIdSecret::new("s").unwrap();
        let first = message_id_for(&secret, "a@b.c", "h", "s", 1).unwrap();
        let second = message_id_for(&secret, "a@b.c", "h", "s", 2).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn a_different_secret_gives_a_different_message_id() {
        let first = MessageIdSecret::new("one").unwrap();
        let second = MessageIdSecret::new("two").unwrap();
        assert_ne!(
            message_id_for(&first, "a@b.c", "h", "s", 1).unwrap(),
            message_id_for(&second, "a@b.c", "h", "s", 1).unwrap()
        );
    }
}

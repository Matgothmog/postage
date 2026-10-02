//! The personhood record `/api/world/verify` writes to `HumanRegistry`
//! (`web/src/app/api/world/verify/route.ts`): which address stands for a
//! person, how long the record lasts, and the attester's EIP-712 signature
//! the registry checks before accepting it.
//!
//! Signing is RFC 6979 deterministic ECDSA with a low-s signature, as viem's
//! `signTypedData` produces, so the same key and record give the same bytes
//! here as in TypeScript. No clock and no randomness: `expires_at` is passed
//! in, which keeps this buildable for `wasm32-unknown-unknown`.

use std::fmt;

use alloy_primitives::{Address, B256, aliases::U40, keccak256};
use alloy_sol_types::{Eip712Domain, SolStruct, eip712_domain, sol};
use k256::ecdsa::SigningKey;

use crate::contracts::{ARC_TESTNET, HUMAN_REGISTRY};
use crate::quote::{address_of, private_key_bytes, recoverable_signature};

/// Matches the Selfie Check credential lifetime, so the free lane lapses when
/// the credential does rather than outliving it.
pub const CREDENTIAL_LIFETIME_SECONDS: i64 = 90 * 24 * 60 * 60;

/// The registry takes the expiry as a `uint40`.
const MAX_EXPIRES_AT: u64 = (1 << 40) - 1;

sol! {
    /// The typed-data struct the registry recovers the attester from. Field
    /// names, order and widths are the type hash, so they are the
    /// contract's, not ours to tidy.
    #[derive(Debug, PartialEq, Eq)]
    struct Attestation {
        address wallet;
        bytes32 nullifierHash;
        uint40 expiresAt;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttestationError {
    #[error("the attester private key is not a valid secp256k1 key")]
    InvalidPrivateKey,
    #[error("attestation expiry {0} does not fit the registry's uint40")]
    ExpiryOutOfRange(u64),
    #[error("the attestation could not be signed")]
    Signing,
}

/// The attester's key. `Debug` never shows it.
#[derive(Clone)]
pub struct AttesterSigner(SigningKey);

impl AttesterSigner {
    /// Reads `0x` followed by 64 hex digits, as viem's `privateKeyToAccount`
    /// does; zero and anything at or above the curve order are refused.
    pub fn from_hex(key: &str) -> Result<Self, AttestationError> {
        let bytes = private_key_bytes(key).map_err(|_| AttestationError::InvalidPrivateKey)?;
        SigningKey::from_slice(&bytes)
            .map(Self)
            .map_err(|_| AttestationError::InvalidPrivateKey)
    }

    /// The address the registry must have registered as its attester for
    /// these signatures to verify.
    pub fn address(&self) -> Address {
        address_of(&self.0)
    }

    /// Signs `Attestation { wallet, nullifierHash, expiresAt }` under
    /// [`attestation_domain`]: 65 bytes of r, s and v, with v as 27 or 28.
    pub fn sign(
        &self,
        wallet: Address,
        nullifier_hash: B256,
        expires_at: u64,
    ) -> Result<[u8; 65], AttestationError> {
        let digest = attestation_digest(wallet, nullifier_hash, expires_at)?;
        recoverable_signature(&self.0, &digest).map_err(|_| AttestationError::Signing)
    }
}

impl fmt::Debug for AttesterSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AttesterSigner(<redacted>)")
    }
}

/// The domain `HumanRegistry` verifies attestations under.
pub fn attestation_domain() -> Eip712Domain {
    eip712_domain! {
        name: "Postage",
        version: "1",
        chain_id: ARC_TESTNET.id,
        verifying_contract: HUMAN_REGISTRY,
    }
}

/// The EIP-712 digest the attester signs.
pub fn attestation_digest(
    wallet: Address,
    nullifier_hash: B256,
    expires_at: u64,
) -> Result<B256, AttestationError> {
    if expires_at > MAX_EXPIRES_AT {
        return Err(AttestationError::ExpiryOutOfRange(expires_at));
    }
    let typed = Attestation {
        wallet,
        nullifierHash: nullifier_hash,
        expiresAt: U40::from(expires_at),
    };
    Ok(typed.eip712_signing_hash(&attestation_domain()))
}

/// A nullifier as the 32-byte key the ledger and the registry use.
///
/// One that already is 32 bytes of hex is taken as it stands, lowercased: two
/// spellings of one nullifier must not read as two people. Anything else is
/// hashed as its UTF-8 text.
pub fn to_bytes32(nullifier: &str) -> B256 {
    let mut bytes = [0u8; 32];
    let spelled = nullifier
        .strip_prefix("0x")
        .filter(|digits| digits.len() == 64)
        .is_some_and(|digits| hex::decode_to_slice(digits, &mut bytes).is_ok());
    if spelled {
        B256::from(bytes)
    } else {
        keccak256(nullifier.as_bytes())
    }
}

/// An address standing for a person rather than an account: the last 20
/// bytes of the nullifier hash. One person is one record by construction, and
/// nobody holds a key to it. Its `Display` is the EIP-55 spelling viem's
/// `getAddress` returns.
pub fn identity_for(nullifier_hash: &B256) -> Address {
    Address::from_slice(&nullifier_hash[12..])
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{address, b256, hex};
    use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};

    use super::*;

    /// From viem 2.x against a throwaway key (`0x11` repeated): `keccak256`,
    /// `getAddress`, `hashTypedData` and `privateKeyToAccount(..)
    /// .signTypedData` over the route's own domain and types.
    const ATTESTER_KEY: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";
    const ATTESTER: Address = address!("0x19E7E376E7C213B7E7e7e46cc70A5dD086DAff2A");
    const NULLIFIER_HASH: B256 =
        b256!("0x26100b7c8a6b62d23d1f5d738654db70bee64d31098ab01ce780e538faf5b081");
    const IDENTITY: &str = "0x8654dB70bEE64d31098Ab01cE780e538fAF5B081";
    const EXPIRES_AT: u64 = 1_765_108_800;
    const DIGEST: B256 =
        b256!("0x315379196e716b4631f880dd594831758d62b58595a7cd85dce9abd991633c43");
    const SIGNATURE: &str = "0x0969bfa3ca93edfa0925b679dd341e44215700af14d75998b66e39bbeff7b9f12b6233872d8f3ec968ebf3dde9426b68c9f8e1b023f7d4f621300b75c90451831c";

    fn attester() -> AttesterSigner {
        AttesterSigner::from_hex(ATTESTER_KEY).unwrap()
    }

    #[test]
    fn a_non_hex_nullifier_is_hashed_as_text() {
        assert_eq!(to_bytes32("world-nullifier"), NULLIFIER_HASH);
    }

    #[test]
    fn a_32_byte_hex_nullifier_is_kept_whatever_its_case() {
        let upper = "0x26100B7C8A6B62D23D1F5D738654DB70BEE64D31098AB01CE780E538FAF5B081";
        assert_eq!(to_bytes32(upper), NULLIFIER_HASH);
        assert_eq!(
            to_bytes32(&NULLIFIER_HASH.to_string()),
            NULLIFIER_HASH,
            "a lowercase spelling is the same nullifier"
        );
    }

    #[test]
    fn hex_of_the_wrong_length_or_prefix_is_hashed_as_text() {
        let short = "0x26100b7c";
        assert_eq!(to_bytes32(short), keccak256(short.as_bytes()));
        let bare = "26100b7c8a6b62d23d1f5d738654db70bee64d31098ab01ce780e538faf5b081";
        assert_eq!(to_bytes32(bare), keccak256(bare.as_bytes()));
        let upper_prefix = "0X26100b7c8a6b62d23d1f5d738654db70bee64d31098ab01ce780e538faf5b081";
        assert_eq!(to_bytes32(upper_prefix), keccak256(upper_prefix.as_bytes()));
        let not_hex = format!("0x{}", "zz".repeat(32));
        assert_eq!(to_bytes32(&not_hex), keccak256(not_hex.as_bytes()));
    }

    #[test]
    fn the_identity_is_the_checksummed_tail_of_the_nullifier_hash() {
        assert_eq!(identity_for(&NULLIFIER_HASH).to_string(), IDENTITY);
    }

    #[test]
    fn the_attester_key_derives_the_address_viem_does() {
        assert_eq!(attester().address(), ATTESTER);
    }

    #[test]
    fn the_typed_data_digest_matches_viem() {
        let digest =
            attestation_digest(identity_for(&NULLIFIER_HASH), NULLIFIER_HASH, EXPIRES_AT).unwrap();
        assert_eq!(digest, DIGEST);
    }

    #[test]
    fn the_signature_matches_viem_byte_for_byte() {
        let signature = attester()
            .sign(identity_for(&NULLIFIER_HASH), NULLIFIER_HASH, EXPIRES_AT)
            .unwrap();
        assert_eq!(format!("0x{}", hex::encode(signature)), SIGNATURE);
    }

    #[test]
    fn the_signature_recovers_to_the_attester() {
        let signature = attester()
            .sign(identity_for(&NULLIFIER_HASH), NULLIFIER_HASH, EXPIRES_AT)
            .unwrap();
        let recovery = RecoveryId::from_byte(signature[64] - 27).unwrap();
        let parsed = Signature::from_slice(&signature[..64]).unwrap();
        let key = VerifyingKey::recover_from_prehash(DIGEST.as_slice(), &parsed, recovery).unwrap();
        let point = key.to_encoded_point(false);
        let recovered = Address::from_slice(&keccak256(&point.as_bytes()[1..])[12..]);
        assert_eq!(recovered, ATTESTER);
    }

    #[test]
    fn an_expiry_past_uint40_is_refused_rather_than_truncated() {
        assert_eq!(
            attester().sign(Address::ZERO, B256::ZERO, 1 << 40),
            Err(AttestationError::ExpiryOutOfRange(1 << 40))
        );
        assert!(
            attester()
                .sign(Address::ZERO, B256::ZERO, MAX_EXPIRES_AT)
                .is_ok()
        );
    }

    #[test]
    fn a_malformed_or_zero_key_is_refused() {
        for key in [
            "",
            "0x11",
            &"11".repeat(32),
            &format!("0x{}", "00".repeat(32)),
        ] {
            assert_eq!(
                AttesterSigner::from_hex(key).unwrap_err(),
                AttestationError::InvalidPrivateKey,
                "{key:?}"
            );
        }
    }

    #[test]
    fn the_debug_output_never_shows_the_key() {
        assert_eq!(format!("{:?}", attester()), "AttesterSigner(<redacted>)");
    }
}

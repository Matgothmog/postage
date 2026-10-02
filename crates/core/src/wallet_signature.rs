//! Checking an EIP-191 `personal_sign` signature locally, the way viem's
//! standalone `verifyMessage` does.
//!
//! The signer is recovered from the signature and compared with the address
//! claimed. Nothing is asked of a chain: an RPC that answered "valid" to
//! everything would otherwise decide who holds which wallet. The price is that
//! a contract wallet (ERC-1271, ERC-6492) cannot prove itself, and nothing
//! Postage issues is one.

use alloy_primitives::{Address, eip191_hash_message, keccak256};
use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};

const SIGNATURE_BYTES: usize = 65;

/// Whether `signature` over `message` was made by the key behind `address`.
///
/// Every malformed input is a `false`, never a panic: all three arrive from a
/// caller. Like viem's `isAddressEqual`, the address is compared without
/// regard to case or checksum; a caller that wants a checksum enforced checks
/// it first, as `isAddress` does in front of this in the TypeScript.
pub fn verify_message(address: &str, message: &str, signature: &str) -> bool {
    let Some(claimed) = loose_address(address) else {
        return false;
    };
    recover_message_address(message, signature).is_some_and(|signer| signer == claimed)
}

/// `isAddress(address, { strict: false })`: `0x` and 40 hex digits of any case.
fn loose_address(text: &str) -> Option<Address> {
    let digits = text.strip_prefix("0x")?;
    let mut bytes = [0u8; 20];
    hex::decode_to_slice(digits, &mut bytes).ok()?;
    Some(Address::from(bytes))
}

/// The address whose key signed `message`, or `None` if `signature` is not one.
///
/// Accepts what viem's `recoverPublicKey` accepts and nothing more: `0x` and
/// 130 hex digits of r, s and v, with v one of 0, 1, 27 or 28. A high-s
/// signature recovers like its low-s twin, as it does in noble-curves; the
/// spent-nonce ledger, not malleability, is what stops a replay.
pub fn recover_message_address(message: &str, signature: &str) -> Option<Address> {
    let bytes = signature_bytes(signature)?;
    let recovery = recovery_bit(bytes[SIGNATURE_BYTES - 1])?;
    let signature = Signature::from_slice(&bytes[..SIGNATURE_BYTES - 1]).ok()?;

    // k256 refuses a high-s signature outright, so it is flipped to its low-s
    // twin, which names the other point of the pair and so the same key.
    let (signature, recovery) = match signature.normalize_s() {
        Some(low) => (low, !recovery),
        None => (signature, recovery),
    };

    let hash = eip191_hash_message(message);
    let key = VerifyingKey::recover_from_prehash(
        hash.as_slice(),
        &signature,
        RecoveryId::new(recovery, false),
    )
    .ok()?;
    Some(address_of(&key))
}

/// The Ethereum address of a secp256k1 public key.
pub fn address_of(key: &VerifyingKey) -> Address {
    let point = key.to_encoded_point(false);
    // Uncompressed SEC1 is a 0x04 tag followed by x and y.
    let hash = keccak256(&point.as_bytes()[1..]);
    Address::from_slice(&hash[12..])
}

/// viem's `isHex` is strict by default: a lowercase `0x`, then hex digits of
/// either case.
fn signature_bytes(signature: &str) -> Option<[u8; SIGNATURE_BYTES]> {
    let digits = signature.strip_prefix("0x")?;
    let mut bytes = [0u8; SIGNATURE_BYTES];
    hex::decode_to_slice(digits, &mut bytes).ok()?;
    Some(bytes)
}

/// viem's `toRecoveryBit`: the y parity, written either bare or Ethereum's way.
/// EIP-155 values are refused, as viem refuses them.
fn recovery_bit(v: u8) -> Option<bool> {
    match v {
        0 | 27 => Some(false),
        1 | 28 => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use k256::ecdsa::SigningKey;

    use super::*;

    /// viem's `privateKeyToAccount(0x1111...)` and its address.
    const HOLDER_ADDRESS: &str = "0x19E7E376E7C213B7E7e7e46cc70A5dD086DAff2A";
    const MESSAGE: &str = "Postage: read my inbox\nDomain: usepostage.com\nWallet: 0x19e7e376e7c213b7e7e7e46cc70a5dd086daff2a\nIssued: 1757332800\nNonce: n";

    /// What viem 2.56's `signMessage` produced for `MESSAGE` under that key.
    const VIEM_SIGNATURE: &str = "0x651014fa76f8b435b6ab2046259d682b2c64b0b5a80c9f0db8e34e87eb728aef2bf32e047f9a143f90e6cf96b1793b84bf1343985f782c289f9884f33a3294661c";
    /// The same signature with s replaced by n - s and v flipped; viem's
    /// `verifyMessage` accepted it.
    const VIEM_HIGH_S: &str = "0x651014fa76f8b435b6ab2046259d682b2c64b0b5a80c9f0db8e34e87eb728aefd40cd1fb8065ebc06f1930694e86c479fb9b994e4fd074132039d9999603acdb1b";

    fn holder() -> SigningKey {
        SigningKey::from_slice(&[0x11; 32]).unwrap()
    }

    /// `personal_sign` as a wallet does it, independent of the code under test
    /// apart from the hash.
    fn sign(key: &SigningKey, message: &str) -> String {
        let hash = eip191_hash_message(message);
        let (signature, recovery) = key.sign_prehash_recoverable(hash.as_slice()).unwrap();
        format!(
            "0x{}{:02x}",
            hex::encode(signature.to_bytes()),
            27 + u8::from(recovery.is_y_odd())
        )
    }

    #[test]
    fn a_signature_viem_produced_verifies_for_its_signer() {
        assert!(verify_message(HOLDER_ADDRESS, MESSAGE, VIEM_SIGNATURE));
    }

    /// RFC 6979 makes signing deterministic, so the same key over the same
    /// text gives viem's bytes exactly: the hash and the encoding agree.
    #[test]
    fn signing_here_reproduces_viems_bytes() {
        assert_eq!(sign(&holder(), MESSAGE), VIEM_SIGNATURE);
        assert_eq!(
            address_of(holder().verifying_key()).to_checksum(None),
            HOLDER_ADDRESS
        );
    }

    #[test]
    fn a_lowercase_address_is_the_same_wallet() {
        assert!(verify_message(
            &HOLDER_ADDRESS.to_lowercase(),
            MESSAGE,
            VIEM_SIGNATURE
        ));
    }

    #[test]
    fn a_signature_over_other_text_does_not_verify() {
        assert!(!verify_message(
            HOLDER_ADDRESS,
            "Postage: something else",
            VIEM_SIGNATURE
        ));
    }

    #[test]
    fn a_signature_from_another_key_does_not_verify() {
        let impostor = SigningKey::from_slice(&[0x22; 32]).unwrap();
        assert!(!verify_message(
            HOLDER_ADDRESS,
            MESSAGE,
            &sign(&impostor, MESSAGE)
        ));
    }

    #[test]
    fn a_high_s_twin_verifies_as_it_does_in_viem() {
        assert!(verify_message(HOLDER_ADDRESS, MESSAGE, VIEM_HIGH_S));
    }

    #[test]
    fn a_bare_y_parity_is_accepted_like_27_and_28() {
        let bare = format!("{}01", &VIEM_SIGNATURE[..130]);
        assert!(verify_message(HOLDER_ADDRESS, MESSAGE, &bare));
    }

    #[test]
    fn uppercase_hex_digits_are_accepted() {
        let upper = format!("0x{}", VIEM_SIGNATURE[2..].to_uppercase());
        assert!(verify_message(HOLDER_ADDRESS, MESSAGE, &upper));
    }

    #[test]
    fn malformed_signatures_are_refused_rather_than_panicking() {
        let body = &VIEM_SIGNATURE[2..130];
        for signature in [
            String::new(),
            "0x".to_owned(),
            VIEM_SIGNATURE[..128].to_owned(),
            format!("0x{body}25"),
            format!("0x{body}1c00"),
            format!("0X{}", &VIEM_SIGNATURE[2..]),
            VIEM_SIGNATURE[2..].to_owned(),
            format!("0x{}", "ab".repeat(65)),
            format!("0x{}1b", "00".repeat(64)),
            format!("0x{}1b", "ff".repeat(64)),
        ] {
            assert!(
                !verify_message(HOLDER_ADDRESS, MESSAGE, &signature),
                "{signature}"
            );
        }
    }

    #[test]
    fn something_that_is_not_an_address_verifies_nothing() {
        for address in [
            "0xnot-an-address",
            "19E7E376E7C213B7E7e7e46cc70A5dD086DAff2A",
            "0X19E7E376E7C213B7E7e7e46cc70A5dD086DAff2A",
            "0x19E7E376E7C213B7E7e7e46cc70A5dD086DAff",
        ] {
            assert!(
                !verify_message(address, MESSAGE, VIEM_SIGNATURE),
                "{address}"
            );
        }
    }

    /// `isAddressEqual` is not strict, so neither is this; the checksum is the
    /// caller's to enforce.
    #[test]
    fn the_comparison_ignores_case_and_checksum_as_viem_does() {
        let upper = format!("0x{}", HOLDER_ADDRESS[2..].to_uppercase());
        assert!(verify_message(&upper, MESSAGE, VIEM_SIGNATURE));
    }

    #[test]
    fn a_multibyte_message_hashes_its_utf8_bytes() {
        // viem's signMessage over "héllo ✉️" with the same key.
        let signature = "0xab649b5b9013ac90654ef6afa20955e8563d659ed37b4a9c8e2372f332f4d98c0d6a2074c2358659d69e2d0fc782c2328b27240637781c05edec0ec06bef61741c";
        assert!(verify_message(HOLDER_ADDRESS, "héllo ✉️", signature));
    }
}

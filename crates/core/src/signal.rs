//! `hashSignal` from `@worldcoin/idkit/hashing` (re-exported from
//! `@worldcoin/idkit-core`): the field element World App folds a request's
//! signal into, and returns as a proof's `signal_hash`.
//!
//! `/api/world/verify` recomputes it from the challenge token and compares, so
//! a proof made for one challenge cannot be presented against another. It has
//! to match the SDK byte for byte; `fixtures/golden/hash-signal.json` pins it.

use crate::rp_signature::hash_to_field;

/// The signal's field hash as `0x` and 64 lowercase hex digits.
///
/// A signal spelled as `0x` followed by a non-empty, even-length run of hex
/// digits is hashed as the bytes it spells; anything else, including an
/// uppercase `0X` prefix, is hashed as its UTF-8 text. That is the SDK's rule,
/// so `"0xdeadbeef"` and `"deadbeef"` are different signals.
pub fn hash_signal(signal: &str) -> String {
    let field = match hex_bytes(signal) {
        Some(bytes) => hash_to_field(&bytes),
        None => hash_to_field(signal.as_bytes()),
    };
    format!("0x{}", hex::encode(field))
}

/// The bytes a `0x`-prefixed hex signal spells, when it spells any.
fn hex_bytes(signal: &str) -> Option<Vec<u8>> {
    signal
        .strip_prefix("0x")
        .filter(|digits| !digits.is_empty())
        .and_then(|digits| hex::decode(digits).ok())
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    const GOLDEN: &str = include_str!("../../../fixtures/golden/hash-signal.json");

    #[derive(Deserialize)]
    struct Fixture {
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        name: String,
        input: Input,
        output: String,
    }

    #[derive(Deserialize)]
    struct Input {
        signal: String,
    }

    #[test]
    fn every_golden_vector_is_reproduced_byte_for_byte() {
        let fixture: Fixture = serde_json::from_str(GOLDEN).unwrap();
        assert_eq!(fixture.cases.len(), 8);
        for case in fixture.cases {
            assert_eq!(
                hash_signal(&case.input.signal),
                case.output,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn a_hex_signal_is_hashed_as_the_bytes_it_spells() {
        assert_eq!(
            hash_signal("0xdeadbeef"),
            format!(
                "0x{}",
                hex::encode(hash_to_field(&[0xde, 0xad, 0xbe, 0xef]))
            )
        );
    }

    #[test]
    fn a_prefix_without_valid_hex_after_it_is_hashed_as_text() {
        for signal in ["0x", "0xabc", "0xzz", "0Xdeadbeef"] {
            assert_eq!(
                hash_signal(signal),
                format!("0x{}", hex::encode(hash_to_field(signal.as_bytes()))),
                "{signal}"
            );
        }
    }

    #[test]
    fn mixed_case_hex_digits_still_count_as_bytes() {
        assert_eq!(hash_signal("0xDEADbeef"), hash_signal("0xdeadbeef"));
    }

    #[test]
    fn the_hash_is_a_field_element_with_a_leading_zero_byte() {
        let hash = hash_signal("anything");
        assert_eq!(hash.len(), 66);
        assert!(hash.starts_with("0x00"), "{hash}");
    }
}

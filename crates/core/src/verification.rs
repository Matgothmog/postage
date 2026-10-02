//! The six-digit code mailed to confirm a claim, and how it is stored and
//! checked.

use hmac::Mac;
use rand_core::CryptoRng;
use subtle::ConstantTimeEq;

use crate::secret::VerificationKey;

/// Long enough to walk to another device, short enough that a guessed code is
/// not worth the wait.
pub const CODE_TTL_SECONDS: i64 = 15 * 60;

/// A six digit code is only safe because guessing is capped. Past this many
/// wrong answers the claim has to be started again, which mints a new code.
pub const MAX_ATTEMPTS: u32 = 5;

const CODE_SPACE: u32 = 1_000_000;

/// A uniformly drawn code in `000000..=999999`, zero-padded.
pub fn generate_code<R: CryptoRng + ?Sized>(rng: &mut R) -> String {
    format!("{:06}", uniform_below(rng, CODE_SPACE))
}

/// Rejection sampling, so no code is likelier than another: draws at or above
/// the largest multiple of `bound` that fits a `u32` are thrown away rather
/// than folded back by `%`.
fn uniform_below<R: CryptoRng + ?Sized>(rng: &mut R, bound: u32) -> u32 {
    let limit = u32::MAX - u32::MAX % bound;
    loop {
        let draw = rng.next_u32();
        if draw < limit {
            return draw % bound;
        }
    }
}

/// Bound to the handle, so a code mailed for one claim cannot settle another.
pub fn hash_code(key: &VerificationKey, handle: &str, code: &str) -> String {
    hex::encode(code_mac(key, handle, code))
}

fn code_mac(key: &VerificationKey, handle: &str, code: &str) -> Vec<u8> {
    let mut mac = key.mac();
    mac.update(format!("{}:{code}", handle.to_lowercase()).as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// Whether `code` is the one `expected` (a stored [`hash_code`]) was minted
/// from, compared in constant time. A stored hash of the wrong length is a
/// mismatch, never a panic.
pub fn code_matches(key: &VerificationKey, handle: &str, code: &str, expected: &str) -> bool {
    let offered = code_mac(key, handle, code);
    let stored = decode_hex_leniently(expected);
    offered.len() == stored.len() && bool::from(offered.ct_eq(&stored))
}

/// Decodes the way Node's `Buffer.from(text, "hex")` does, so a stored value
/// reads back exactly as the TypeScript read it: either case is accepted, and
/// decoding stops at the first pair that is not two hex digits, dropping any
/// odd trailing character.
fn decode_hex_leniently(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map_while(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            u8::try_from(high << 4 | low).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use rand_core::{OsRng, RngCore, TryRngCore};
    use serde::Deserialize;

    use super::*;
    use crate::secret::MessageIdSecret;

    fn key_for(secret: &str) -> VerificationKey {
        VerificationKey::derive(&MessageIdSecret::new(secret).unwrap()).unwrap()
    }

    fn test_key() -> VerificationKey {
        key_for(&"x".repeat(32))
    }

    fn fresh_code() -> String {
        generate_code(&mut OsRng.unwrap_err())
    }

    #[test]
    fn a_code_matches_the_hash_it_was_minted_from() {
        let key = test_key();
        let code = fresh_code();
        let stored = hash_code(&key, "demo", &code);
        assert!(code_matches(&key, "demo", &code, &stored));
    }

    #[test]
    fn a_wrong_code_does_not_match_another_codes_hash() {
        let key = test_key();
        let stored = hash_code(&key, "demo", "123456");
        assert!(!code_matches(&key, "demo", "654321", &stored));
    }

    #[test]
    fn a_hash_of_the_wrong_length_is_rejected_without_panicking() {
        assert!(!code_matches(
            &test_key(),
            "demo",
            &fresh_code(),
            "deadbeef"
        ));
    }

    #[test]
    fn an_empty_expected_hash_is_rejected_without_panicking() {
        assert!(!code_matches(&test_key(), "demo", &fresh_code(), ""));
    }

    #[test]
    fn a_code_minted_for_one_handle_does_not_match_a_different_handle() {
        let key = test_key();
        let code = fresh_code();
        let stored = hash_code(&key, "alice", &code);
        assert!(!code_matches(&key, "bob", &code, &stored));
    }

    #[test]
    fn handle_comparison_is_case_insensitive_matching_how_the_hash_was_minted() {
        let key = test_key();
        let code = fresh_code();
        let stored = hash_code(&key, "Demo", &code);
        assert!(code_matches(&key, "demo", &code, &stored));
    }

    #[test]
    fn generate_code_always_produces_a_six_digit_string_zero_padded() {
        for _ in 0..50 {
            let code = fresh_code();
            assert_eq!(code.len(), 6);
            assert!(code.bytes().all(|byte| byte.is_ascii_digit()), "{code}");
        }
    }

    /// Yields queued `u32` draws, the way the golden capture queued
    /// `randomInt` results.
    struct QueuedDraws(Vec<u32>);

    impl RngCore for QueuedDraws {
        fn next_u32(&mut self) -> u32 {
            self.0.remove(0)
        }

        fn next_u64(&mut self) -> u64 {
            unreachable!("codes only draw u32s")
        }

        fn fill_bytes(&mut self, _dst: &mut [u8]) {
            unreachable!("codes only draw u32s")
        }
    }

    impl CryptoRng for QueuedDraws {}

    #[test]
    fn a_draw_in_the_biased_tail_is_redrawn_rather_than_folded() {
        let mut rng = QueuedDraws(vec![u32::MAX, 7]);
        assert_eq!(generate_code(&mut rng), "000007");
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Golden {
        config: GoldenConfig,
        hash_code: Vec<Case<HashInput, String>>,
        code_matches: Vec<Case<MatchInput, bool>>,
        generate_code: Vec<Case<GenerateInput, String>>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GoldenConfig {
        message_id_secret: String,
    }

    #[derive(Deserialize)]
    struct Case<I, O> {
        name: String,
        input: I,
        output: O,
    }

    #[derive(Deserialize)]
    struct HashInput {
        handle: String,
        code: String,
    }

    #[derive(Deserialize)]
    struct MatchInput {
        handle: String,
        code: String,
        expected: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GenerateInput {
        random_int: u32,
    }

    fn golden() -> Golden {
        serde_json::from_str(include_str!("../../../fixtures/golden/code-hash.json")).unwrap()
    }

    #[test]
    fn golden_hash_code_cases_match_byte_for_byte() {
        let golden = golden();
        let key = key_for(&golden.config.message_id_secret);
        assert_eq!(golden.hash_code.len(), 7);

        for case in golden.hash_code {
            let hash = hash_code(&key, &case.input.handle, &case.input.code);
            assert_eq!(hash, case.output, "{}", case.name);
        }
    }

    #[test]
    fn golden_code_matches_cases_match() {
        let golden = golden();
        let key = key_for(&golden.config.message_id_secret);
        assert_eq!(golden.code_matches.len(), 9);

        for case in golden.code_matches {
            let input = case.input;
            let verdict = code_matches(&key, &input.handle, &input.code, &input.expected);
            assert_eq!(verdict, case.output, "{}", case.name);
        }
    }

    #[test]
    fn golden_generate_code_cases_match() {
        let golden = golden();
        assert_eq!(golden.generate_code.len(), 4);

        for case in golden.generate_code {
            let mut rng = QueuedDraws(vec![case.input.random_int]);
            assert_eq!(generate_code(&mut rng), case.output, "{}", case.name);
        }
    }

    #[test]
    fn lenient_hex_stops_at_the_first_invalid_pair_like_node() {
        assert_eq!(decode_hex_leniently("abZZcd"), vec![0xab]);
        assert_eq!(decode_hex_leniently("ABc"), vec![0xab]);
        assert_eq!(decode_hex_leniently("zz"), Vec::<u8>::new());
    }
}

//! Stateless, single-window nonces a wallet signs under.
//!
//! Nothing is written down to mint one, which is the whole point of the
//! design: the route that hands them out is open, and a ledger written at issue
//! time would be a table anyone could grow without limit. The MAC is what makes
//! the value unforgeable instead, and the single-use record is only taken
//! later, once a valid signature has already been presented under it.
//!
//! Server-only in spirit: the key behind it must never reach a browser bundle.

use hmac::Mac;
use rand_core::CryptoRng;
use subtle::ConstantTimeEq;

use crate::secret::WalletNonceKey;

/// How long a minted nonce is worth anything.
///
/// Judged entirely on the server's own clock against a value the server itself
/// minted, so it can be as tight as a signature prompt takes: two minutes
/// covers reading a wallet prompt and tapping approve on a phone. Unrelated to
/// the much wider clock-skew tolerance for the signer's own timestamp.
pub const WALLET_NONCE_TTL_SECONDS: i64 = 120;

/// Names what this MAC is *for*, inside the MAC. Without it a value
/// authenticated for some other purpose under the same key could be presented
/// here as a nonce.
const PURPOSE: &str = "postage-wallet-nonce";

const RANDOM_BYTES: usize = 16;

/// The tag over the three fields a nonce carries. `|` separates them and none
/// of the three can contain one (an address and a hex string are both drawn
/// from `[0-9a-fx]`, the expiry from digits), so no two different triples share
/// a preimage.
fn tag(key: &WalletNonceKey, wallet: &str, expires_at_text: &str, random: &str) -> String {
    let preimage = [PURPOSE, &wallet.to_lowercase(), expires_at_text, random].join("|");
    let mut mac = key.mac();
    mac.update(preimage.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// A fresh nonce bound to one wallet and one short window, as
/// `<expiresAt>.<random hex>.<tag hex>`.
///
/// `now` is the server's clock in epoch seconds and `rng` its source of
/// randomness; both are passed in so this stays pure and runs under wasm.
pub fn mint_wallet_nonce<R: CryptoRng + ?Sized>(
    key: &WalletNonceKey,
    wallet: &str,
    now: i64,
    rng: &mut R,
) -> String {
    let mut random_bytes = [0u8; RANDOM_BYTES];
    rng.fill_bytes(&mut random_bytes);

    let expires_at_text = now.saturating_add(WALLET_NONCE_TTL_SECONDS).to_string();
    let random = hex::encode(random_bytes);
    let tag = tag(key, wallet, &expires_at_text, &random);
    format!("{expires_at_text}.{random}.{tag}")
}

/// Whether this nonce is one we minted, for this wallet, still inside its
/// window, and if so when it stops being one.
///
/// The expiry is returned rather than merely checked because the caller has to
/// write it down when it spends the nonce, and re-deriving it there would mean
/// parsing this string twice with two chances to disagree. Every malformed
/// input is a `None`, never a panic: this runs on caller-controlled headers.
pub fn verify_wallet_nonce(
    key: &WalletNonceKey,
    nonce: &str,
    wallet: &str,
    now: i64,
) -> Option<i64> {
    let parts: Vec<&str> = nonce.split('.').collect();
    let [expires_at_text, random, offered] = parts.as_slice() else {
        return None;
    };
    if !is_ascii_digits(expires_at_text) || !is_random_hex(random) {
        return None;
    }

    // The tag is computed over the text exactly as it arrived, so a
    // re-serialised expiry can never drift from the one that was authenticated.
    if !same_tag(offered, &tag(key, wallet, expires_at_text, random)) {
        return None;
    }

    // Digits only, so the one way to fail is overflow, which no nonce this
    // server minted can reach.
    let expires_at: i64 = expires_at_text.parse().ok()?;
    if expires_at <= now {
        return None;
    }
    Some(expires_at)
}

fn is_ascii_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Lowercase only, because that is what minting writes and the tag covers the
/// text as written.
fn is_random_hex(text: &str) -> bool {
    text.len() == RANDOM_BYTES * 2
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Compared without leaking where two tags first differ. A length mismatch is
/// refused up front and says nothing an attacker could not already learn.
fn same_tag(offered: &str, expected: &str) -> bool {
    offered.len() == expected.len() && bool::from(offered.as_bytes().ct_eq(expected.as_bytes()))
}

#[cfg(test)]
mod tests {
    use rand_core::{OsRng, RngCore, TryRngCore};
    use serde::Deserialize;

    use super::*;
    use crate::secret::MessageIdSecret;

    const WALLET: &str = "0x19E7E376E7C213B7E7e7e46cc70A5dD086DAff2A";
    const OTHER_WALLET: &str = "0x1563915e194D8CfBA1943570603F7606A3115508";
    const FROZEN_NOW: i64 = 1_788_868_800;

    fn key_for(secret: &str) -> WalletNonceKey {
        WalletNonceKey::derive(&MessageIdSecret::new(secret).unwrap()).unwrap()
    }

    fn test_key() -> WalletNonceKey {
        key_for(&"x".repeat(32))
    }

    fn mint(wallet: &str) -> String {
        mint_wallet_nonce(&test_key(), wallet, FROZEN_NOW, &mut OsRng.unwrap_err())
    }

    fn verify(nonce: &str, wallet: &str, now: i64) -> Option<i64> {
        verify_wallet_nonce(&test_key(), nonce, wallet, now)
    }

    #[test]
    fn a_freshly_minted_nonce_verifies_for_the_wallet_it_was_minted_for() {
        assert!(verify(&mint(WALLET), WALLET, FROZEN_NOW).is_some());
    }

    #[test]
    fn a_nonce_reports_the_moment_it_stops_being_one() {
        assert_eq!(
            verify(&mint(WALLET), WALLET, FROZEN_NOW),
            Some(FROZEN_NOW + WALLET_NONCE_TTL_SECONDS)
        );
    }

    #[test]
    fn a_nonce_minted_for_one_wallet_proves_nothing_for_another() {
        assert_eq!(verify(&mint(WALLET), OTHER_WALLET, FROZEN_NOW), None);
    }

    #[test]
    fn a_nonce_verifies_against_the_same_wallet_named_in_lowercase() {
        assert!(verify(&mint(WALLET), &WALLET.to_lowercase(), FROZEN_NOW).is_some());
    }

    #[test]
    fn a_nonce_is_worthless_one_second_after_its_window_closes() {
        let nonce = mint(WALLET);
        assert!(
            verify(&nonce, WALLET, FROZEN_NOW).is_some(),
            "precondition: it is good now"
        );

        assert_eq!(
            verify(&nonce, WALLET, FROZEN_NOW + WALLET_NONCE_TTL_SECONDS + 1),
            None
        );
    }

    #[test]
    fn an_expiry_edited_to_a_later_one_proves_nothing() {
        let nonce = mint(WALLET);
        let [expires_at, random, mac] = nonce.split('.').collect::<Vec<_>>()[..] else {
            panic!("minted nonce has three parts");
        };
        let stretched = format!(
            "{}.{random}.{mac}",
            expires_at.parse::<i64>().unwrap() + 86_400
        );

        assert_eq!(verify(&stretched, WALLET, FROZEN_NOW), None);
    }

    #[test]
    fn a_nonce_minted_with_no_key_at_all_proves_nothing() {
        let expires_at = FROZEN_NOW + WALLET_NONCE_TTL_SECONDS;
        let forged = format!("{expires_at}.{}.{}", "ab".repeat(16), "cd".repeat(32));

        assert_eq!(verify(&forged, WALLET, FROZEN_NOW), None);
    }

    #[test]
    fn a_nonce_whose_random_half_was_swapped_proves_nothing() {
        let nonce = mint(WALLET);
        let [expires_at, _, mac] = nonce.split('.').collect::<Vec<_>>()[..] else {
            panic!("minted nonce has three parts");
        };
        let swapped = format!("{expires_at}.{}.{mac}", "00".repeat(16));

        assert_eq!(verify(&swapped, WALLET, FROZEN_NOW), None);
    }

    #[test]
    fn anything_that_is_not_a_nonce_is_refused_rather_than_panicking() {
        let soon = FROZEN_NOW + 60;
        let rubbish = [
            String::new(),
            ".".into(),
            "..".into(),
            "not-a-nonce".into(),
            "1.2".into(),
            "1.2.3.4".into(),
            format!("{soon}.{}", "ab".repeat(16)),
            format!("-1.{}.{}", "ab".repeat(16), "cd".repeat(32)),
            format!("1e9.{}.{}", "ab".repeat(16), "cd".repeat(32)),
            format!("{soon}.NOTHEX{}.{}", "ab".repeat(13), "cd".repeat(32)),
            format!(
                "99999999999999999999999.{}.{}",
                "ab".repeat(16),
                "cd".repeat(32)
            ),
        ];

        for offered in rubbish {
            assert_eq!(
                verify(&offered, WALLET, FROZEN_NOW),
                None,
                "accepted {offered:?}"
            );
        }
    }

    #[test]
    fn two_nonces_minted_in_the_same_second_are_different_values() {
        assert_ne!(mint(WALLET), mint(WALLET));
    }

    /// Hands out exactly the bytes a golden case recorded.
    struct RecordedBytes(Vec<u8>);

    impl RngCore for RecordedBytes {
        fn next_u32(&mut self) -> u32 {
            unreachable!("minting only fills bytes")
        }

        fn next_u64(&mut self) -> u64 {
            unreachable!("minting only fills bytes")
        }

        fn fill_bytes(&mut self, dst: &mut [u8]) {
            assert_eq!(dst.len(), self.0.len(), "recorded byte count differs");
            dst.copy_from_slice(&self.0);
        }
    }

    impl CryptoRng for RecordedBytes {}

    #[derive(Deserialize)]
    struct Golden {
        config: GoldenConfig,
        mint: Vec<MintCase>,
        verify: Vec<VerifyCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GoldenConfig {
        message_id_secret: String,
        ttl_seconds: i64,
        random_byte_count: usize,
    }

    #[derive(Deserialize)]
    struct MintCase {
        name: String,
        input: MintInput,
        output: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct MintInput {
        wallet: String,
        now: i64,
        random_bytes_hex: String,
    }

    #[derive(Deserialize)]
    struct VerifyCase {
        name: String,
        input: VerifyInput,
        output: Option<i64>,
    }

    #[derive(Deserialize)]
    struct VerifyInput {
        nonce: String,
        wallet: String,
        now: i64,
    }

    fn golden() -> Golden {
        serde_json::from_str(include_str!("../../../fixtures/golden/wallet-nonce.json")).unwrap()
    }

    #[test]
    fn golden_config_matches_the_constants() {
        let config = golden().config;
        assert_eq!(config.ttl_seconds, WALLET_NONCE_TTL_SECONDS);
        assert_eq!(config.random_byte_count, RANDOM_BYTES);
    }

    #[test]
    fn golden_mint_cases_match_byte_for_byte() {
        let golden = golden();
        let key = key_for(&golden.config.message_id_secret);
        assert_eq!(golden.mint.len(), 4);

        for case in golden.mint {
            let mut rng = RecordedBytes(hex::decode(&case.input.random_bytes_hex).unwrap());
            let minted = mint_wallet_nonce(&key, &case.input.wallet, case.input.now, &mut rng);
            assert_eq!(minted, case.output, "{}", case.name);
        }
    }

    #[test]
    fn golden_verify_cases_match() {
        let golden = golden();
        let key = key_for(&golden.config.message_id_secret);
        assert_eq!(golden.verify.len(), 18);

        for case in golden.verify {
            let verdict =
                verify_wallet_nonce(&key, &case.input.nonce, &case.input.wallet, case.input.now);
            assert_eq!(verdict, case.output, "{}", case.name);
        }
    }
}

//! The contract a wallet proof travels under, in one place.
//!
//! A request proves the wallet it names one of two ways, and both ride on
//! headers. A disagreement between the browser that writes a proof and the
//! route that reads it is not a type error and not a failing build: it is a
//! sign-in that quietly stops working for everybody. So the names are declared
//! here once, the writers build from them, the readers parse from them, and the
//! statements are re-exported alongside, because changing what is signed
//! without changing what is verified breaks exactly the same thing.

pub use crate::statements::{claim_statement, confirm_statement, read_statement};

use crate::js_number::to_js_string;

/// Privy's identity token. It already names the wallets Privy minted for
/// whoever is signed in, so a session holding one has nothing left to sign.
pub const IDENTITY_TOKEN_HEADER: &str = "privy-id-token";

/// What a session without an identity token sends instead: the wallet it
/// claims, when it said so, the nonce the server minted for it, and a
/// signature over the statement naming all of them.
pub const WALLET_HEADER: &str = "x-postage-wallet";
pub const ISSUED_AT_HEADER: &str = "x-postage-issued";
pub const SIGNATURE_HEADER: &str = "x-postage-signature";
pub const NONCE_HEADER: &str = "x-postage-nonce";

/// Where a browser asks for the nonce it is about to sign under.
pub const WALLET_NONCE_PATH: &str = "/api/wallet-nonce";

/// The line that makes one signature answer one request instead of every
/// request inside the freshness window. Both ends build the signed text
/// through here, so a change to the wording is a change to both at once.
pub fn with_nonce(statement: &str, nonce: &str) -> String {
    format!("{statement}\nNonce: {nonce}")
}

/// A proof as it arrives, before any of it has been believed. Every field is
/// caller-controlled, so this says only what was offered, never that it holds
/// up.
#[derive(Debug, Clone, PartialEq)]
pub struct OfferedProof {
    pub identity_token: Option<String>,
    pub wallet: Option<String>,
    /// Read the way JavaScript's `Number()` reads a header, so a header that
    /// was never sent is 0 and one carrying junk is NaN. Both sit outside any
    /// freshness window, and the reader refuses them there rather than here.
    pub issued_at: f64,
    pub signature: Option<String>,
    /// Worth nothing on its own: it authenticates itself to the server that
    /// minted it, which is checked before anything else about the signature.
    pub nonce: Option<String>,
}

/// Header name and value pairs, in the order they are written.
pub type ProofHeaders = Vec<(&'static str, String)>;

/// The proof a session that holds an identity token offers.
pub fn identity_proof(identity_token: &str) -> ProofHeaders {
    vec![(IDENTITY_TOKEN_HEADER, identity_token.to_owned())]
}

/// The proof every other session offers, from a signature it has already
/// collected. Prompting a wallet is kept out of here so this half of the
/// contract stays callable from a test and a route. The timestamp is written
/// as JavaScript's `String(number)` would, the same text the statement signs.
pub fn signed_proof(wallet: &str, issued_at: f64, signature: &str, nonce: &str) -> ProofHeaders {
    vec![
        (WALLET_HEADER, wallet.to_owned()),
        (ISSUED_AT_HEADER, to_js_string(issued_at)),
        (SIGNATURE_HEADER, signature.to_owned()),
        (NONCE_HEADER, nonce.to_owned()),
    ]
}

/// The other end of the two above, and the only place a route should learn
/// these names from. `header` looks a header up by its lowercase name, which
/// keeps this free of any one HTTP library's types.
pub fn read_proof<F>(header: F) -> OfferedProof
where
    F: Fn(&str) -> Option<String>,
{
    OfferedProof {
        identity_token: header(IDENTITY_TOKEN_HEADER),
        wallet: header(WALLET_HEADER),
        issued_at: js_number(header(ISSUED_AT_HEADER).as_deref().unwrap_or_default()),
        signature: header(SIGNATURE_HEADER),
        nonce: header(NONCE_HEADER),
    }
}

/// JavaScript's `Number(text)` for a string: surrounding whitespace ignored,
/// empty is 0, `0x`/`0o`/`0b` literals and signed `Infinity` accepted, and
/// anything else that is not a plain decimal literal is NaN.
pub(crate) fn js_number(text: &str) -> f64 {
    let trimmed = text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if trimmed.is_empty() {
        return 0.0;
    }
    if let Some(value) = radix_literal(trimmed) {
        return value;
    }

    let unsigned = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if unsigned == "Infinity" {
        let infinity = f64::INFINITY;
        return if trimmed.starts_with('-') {
            -infinity
        } else {
            infinity
        };
    }
    // Rust's float parser also takes `inf`, `nan` and friends, which `Number`
    // does not, so only a decimal literal's characters are let through.
    let is_decimal_literal = unsigned
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'));
    if !is_decimal_literal {
        return f64::NAN;
    }
    trimmed.parse().unwrap_or(f64::NAN)
}

/// `0x1F`, `0o17`, `0b101`: unsigned, at least one digit, any case of prefix.
fn radix_literal(text: &str) -> Option<f64> {
    let prefix = text.get(..2)?.to_ascii_lowercase();
    let radix = match prefix.as_str() {
        "0x" => 16,
        "0o" => 8,
        "0b" => 2,
        _ => return None,
    };
    let digits = &text[2..];
    if digits.is_empty() {
        return Some(f64::NAN);
    }
    let value = digits.chars().try_fold(0.0_f64, |total, c| {
        c.to_digit(radix)
            .map(|digit| total * f64::from(radix) + f64::from(digit))
    });
    Some(value.unwrap_or(f64::NAN))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WALLET: &str = "0x19E7E376E7C213B7E7e7e46cc70A5dD086DAff2A";

    fn lookup(headers: ProofHeaders) -> impl Fn(&str) -> Option<String> {
        move |name| {
            headers
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
        }
    }

    /// The only place these strings are written by hand a second time, so a
    /// rename cannot silently change the wire under an open browser tab.
    #[test]
    fn the_header_names_are_the_ones_already_on_the_wire() {
        assert_eq!(IDENTITY_TOKEN_HEADER, "privy-id-token");
        assert_eq!(WALLET_HEADER, "x-postage-wallet");
        assert_eq!(ISSUED_AT_HEADER, "x-postage-issued");
        assert_eq!(SIGNATURE_HEADER, "x-postage-signature");
        assert_eq!(NONCE_HEADER, "x-postage-nonce");
        assert_eq!(WALLET_NONCE_PATH, "/api/wallet-nonce");
    }

    #[test]
    fn what_the_writer_puts_on_the_wire_is_what_the_reader_takes_off_it() {
        let written = signed_proof(WALLET, 1_757_332_800.0, "0xsignature", "0xnonce");

        assert_eq!(
            read_proof(lookup(written)),
            OfferedProof {
                identity_token: None,
                wallet: Some(WALLET.to_owned()),
                issued_at: 1_757_332_800.0,
                signature: Some("0xsignature".to_owned()),
                nonce: Some("0xnonce".to_owned()),
            }
        );
    }

    /// 0 rather than None, because `Number` says so. Safe only because 0 is
    /// half a century stale and no window admits it.
    #[test]
    fn a_request_carrying_no_proof_reads_back_empty_with_a_timestamp_no_window_admits() {
        assert_eq!(
            read_proof(|_| None),
            OfferedProof {
                identity_token: None,
                wallet: None,
                issued_at: 0.0,
                signature: None,
                nonce: None,
            }
        );
    }

    #[test]
    fn the_identity_token_a_writer_attaches_is_the_one_the_reader_finds() {
        let proof = read_proof(lookup(identity_proof("token-abc")));
        assert_eq!(proof.identity_token.as_deref(), Some("token-abc"));
    }

    #[test]
    fn the_nonce_line_is_appended_to_the_statement() {
        assert_eq!(with_nonce("Postage: x", "n1"), "Postage: x\nNonce: n1");
    }

    #[test]
    fn issued_at_reads_like_javascript_number() {
        let cases: [(&str, f64); 9] = [
            (" 12 ", 12.0),
            ("1.e5", 100_000.0),
            ("0x1F", 31.0),
            ("0b101", 5.0),
            (".5", 0.5),
            ("+Infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
            ("\u{a0}12", 12.0),
            ("1757332800.5", 1_757_332_800.5),
        ];
        for (text, expected) in cases {
            assert_eq!(js_number(text), expected, "{text:?}");
        }
    }

    #[test]
    fn issued_at_junk_reads_as_nan() {
        for text in [
            "-0x10", "1_0", "inf", "NaN", "1e", "0x", "abc", "12abc", "+-5",
        ] {
            assert!(js_number(text).is_nan(), "{text:?}");
        }
    }
}

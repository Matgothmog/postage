//! Privy identity tokens: the pure half of reading one.
//!
//! An identity token is an ES256 JWT whose claims carry the accounts Privy has
//! already verified for whoever holds it. Everything here works on bytes that
//! are already in hand - parsing, the algorithm check, the signature over a
//! JWK, the claim checks against a `now` the caller reads, and picking the key
//! a header names out of a key set. Fetching and caching that key set is I/O
//! and lives in the server crate.
//!
//! Every failure is `None` rather than an error: an absent, stale or forged
//! token is an ordinary state that falls back to the longer signup path, and
//! nothing about which check refused it is the caller's business.
//!
//! The checks deliberately reproduce what the TypeScript original did with
//! loosely typed JSON, including which odd shapes it rejected outright (a
//! `null` linked account, an address that is not a string) rather than skipped.

use base64::Engine;
use base64::alphabet::URL_SAFE;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use p256::EncodedPoint;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde_json::Value;

pub const ISSUER: &str = "privy.io";
pub const ALGORITHM: &str = "ES256";

/// Base64url as JWTs write it, read without caring whether the padding was
/// kept or how the unused trailing bits are set - the decoder a token was
/// minted against did not care either.
const BASE64URL: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// The accounts Privy has already confirmed for whoever is holding this token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivyIdentity {
    pub user_id: String,
    /// `None` for someone who signed in with a passkey and never linked an address.
    pub email: Option<String>,
    pub wallets: Vec<String>,
}

/// The `kid` a token header names, kept as the raw JSON value (or its
/// absence) because that is what is compared against each JWKS entry's `kid`:
/// an absent header kid matches an entry that has none.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyId(Option<Value>);

impl KeyId {
    /// Whether the header actually names a key. An empty string, `null`,
    /// `false` or `0` count as naming none, so they go straight to the
    /// single-key fallback without the refetch a named-but-unknown kid earns.
    pub fn is_named(&self) -> bool {
        match &self.0 {
            None | Some(Value::Null) => false,
            Some(Value::Bool(flag)) => *flag,
            Some(Value::Number(number)) => number.as_f64().is_some_and(|value| value != 0.0),
            Some(Value::String(text)) => !text.is_empty(),
            Some(Value::Array(_) | Value::Object(_)) => true,
        }
    }

    fn names(&self, key: &Value) -> bool {
        key.get("kid") == self.0.as_ref()
    }
}

/// A token that has passed the checks that need no key: three non-empty
/// segments and a header naming ES256. Nothing past the header has been read.
#[derive(Debug, Clone)]
pub struct UnverifiedToken<'a> {
    signing_input: &'a str,
    payload: &'a str,
    signature: &'a str,
    kid: KeyId,
}

/// Splits a token and checks its header. The algorithm is checked here,
/// before any key is looked up, because the token names its own algorithm and
/// "none" is a signature nobody has to forge.
pub fn parse(token: &str) -> Option<UnverifiedToken<'_>> {
    // Exactly three segments: a genuine token with junk appended must not
    // verify on the three segments that were actually signed.
    let segments: Vec<&str> = token.split('.').collect();
    let [head, payload, signature] = segments.as_slice() else {
        return None;
    };
    if head.is_empty() || payload.is_empty() || signature.is_empty() {
        return None;
    }

    let header = decode_json(head)?;
    if header.get("alg").and_then(Value::as_str) != Some(ALGORITHM) {
        return None;
    }

    let signing_input = token.get(..head.len() + 1 + payload.len())?;
    Some(UnverifiedToken {
        signing_input,
        payload,
        signature,
        kid: KeyId(header.get("kid").cloned()),
    })
}

impl UnverifiedToken<'_> {
    pub fn kid(&self) -> &KeyId {
        &self.kid
    }

    /// The identity this token carries, if `jwk` signed it and its claims
    /// are for `app_id`, from Privy, and unexpired at `now` (epoch seconds).
    /// The payload is decoded only once the signature has verified.
    pub fn verified_identity(&self, jwk: &Value, app_id: &str, now: i64) -> Option<PrivyIdentity> {
        if !self.signed_by(jwk) {
            return None;
        }
        claims(&decode_json(self.payload)?, app_id, now)
    }

    fn signed_by(&self, jwk: &Value) -> bool {
        let Some(key) = verifying_key(jwk) else {
            return false;
        };
        let Some(signature) = BASE64URL
            .decode(self.signature)
            .ok()
            .and_then(|bytes| Signature::from_slice(&bytes).ok())
        else {
            return false;
        };
        key.verify(self.signing_input.as_bytes(), &signature)
            .is_ok()
    }
}

/// A P-256 public key from its JWK. Anything else - another curve, another
/// key type, a point off the curve - verifies nothing.
fn verifying_key(jwk: &Value) -> Option<VerifyingKey> {
    if jwk.get("kty").and_then(Value::as_str) != Some("EC")
        || jwk.get("crv").and_then(Value::as_str) != Some("P-256")
    {
        return None;
    }
    let coordinate = |name: &str| -> Option<[u8; 32]> {
        let text = jwk.get(name)?.as_str()?;
        BASE64URL.decode(text).ok()?.try_into().ok()
    };
    let (x, y) = (coordinate("x")?, coordinate("y")?);
    let point = EncodedPoint::from_affine_coordinates(&x.into(), &y.into(), false);
    VerifyingKey::from_encoded_point(&point).ok()
}

/// The entry whose `kid` is exactly the one the header names.
pub fn named_key<'k>(keys: &'k [Value], kid: &KeyId) -> Option<&'k Value> {
    keys.iter().find(|key| kid.names(key))
}

/// The key naming `kid`, or the only key published when the header names one
/// the set doesn't have - Privy publishes exactly one key outside a rotation
/// window, so that's the only case with one honest answer. With more than one,
/// taking the first would accept a token that names neither.
pub fn match_key<'k>(keys: &'k [Value], kid: &KeyId) -> Option<&'k Value> {
    named_key(keys, kid).or(match keys {
        [only] => Some(only),
        _ => None,
    })
}

/// Why a JWKS response body could not be read as a key set.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JwksBodyError {
    #[error("Privy JWKS body is not JSON")]
    NotJson,
    #[error("Privy JWKS body is null")]
    Null,
    #[error("Privy JWKS `keys` is not an array")]
    KeysNotAnArray,
}

/// The entries of a JWKS body. A body with no `keys` is an empty set, which
/// verifies no token - true, and the answer.
pub fn jwks_keys(body: &[u8]) -> Result<Vec<Value>, JwksBodyError> {
    let parsed: Value = serde_json::from_slice(body).map_err(|_| JwksBodyError::NotJson)?;
    if parsed.is_null() {
        return Err(JwksBodyError::Null);
    }
    match parsed.get("keys") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(keys)) => Ok(keys.clone()),
        Some(_) => Err(JwksBodyError::KeysNotAnArray),
    }
}

fn decode_json(segment: &str) -> Option<Value> {
    let bytes = BASE64URL.decode(segment).ok()?;
    serde_json::from_str(&String::from_utf8_lossy(&bytes)).ok()
}

fn claims(payload: &Value, app_id: &str, now: i64) -> Option<PrivyIdentity> {
    let for_this_app = match payload.get("aud") {
        Some(Value::Array(audiences)) => audiences.iter().any(|aud| aud.as_str() == Some(app_id)),
        Some(Value::String(audience)) => audience == app_id,
        _ => false,
    };
    if payload.get("iss").and_then(Value::as_str) != Some(ISSUER) || !for_this_app {
        return None;
    }
    // Typed, not compared: a string expiry read leniently either rejects
    // live tokens or, on the other side of a refactor, accepts dead ones.
    // Expiry is exclusive, so a token whose `exp` is this second is dead.
    let expires = payload.get("exp")?.as_f64()?;
    if expires <= now as f64 {
        return None;
    }
    let user_id = payload.get("sub")?.as_str()?.to_owned();

    let accounts = linked_accounts(payload.get("linked_accounts"))?;
    // Reading a field off a `null` entry is an error, not a skip.
    if accounts.iter().any(Value::is_null) {
        return None;
    }
    let email = email(&accounts)?;
    let wallets = wallets(&accounts)?;

    Some(PrivyIdentity {
        user_id,
        email,
        wallets,
    })
}

/// Privy stringifies this claim rather than nesting it, so it arrives as JSON
/// inside JSON. A nested array reads the same way, so a change at their end
/// does not silently empty every identity. `None` when the string is not JSON.
fn linked_accounts(claim: Option<&Value>) -> Option<Vec<Value>> {
    let parsed = match claim {
        Some(Value::String(text)) => serde_json::from_str(text).ok()?,
        Some(other) => other.clone(),
        None => Value::Null,
    };
    match parsed {
        Value::Array(accounts) => Some(accounts),
        _ => Some(Vec::new()),
    }
}

fn has_type(account: &Value, kind: &str) -> bool {
    account.get("type").and_then(Value::as_str) == Some(kind)
}

/// The first email account's address, lower-cased. The outer `None` rejects
/// the token: an address that is present but neither a string nor a falsy
/// value cannot be lower-cased.
fn email(accounts: &[Value]) -> Option<Option<String>> {
    let Some(account) = accounts.iter().find(|account| has_type(account, "email")) else {
        return Some(None);
    };
    match account.get("address") {
        None | Some(Value::Null | Value::Bool(false)) => Some(None),
        Some(Value::Number(number)) if number.as_f64() == Some(0.0) => Some(None),
        Some(Value::String(address)) if address.is_empty() => Some(None),
        Some(Value::String(address)) => Some(Some(address.to_lowercase())),
        Some(_) => None,
    }
}

/// Wallet addresses that start with `0x`, lower-cased; a missing address is
/// skipped, and one that is not a string rejects the token.
fn wallets(accounts: &[Value]) -> Option<Vec<String>> {
    let mut wallets = Vec::new();
    for account in accounts
        .iter()
        .filter(|account| has_type(account, "wallet"))
    {
        match account.get("address") {
            None | Some(Value::Null) => {}
            Some(Value::String(address)) if address.starts_with("0x") => {
                wallets.push(address.to_lowercase());
            }
            Some(Value::String(_)) => {}
            Some(_) => return None,
        }
    }
    Some(wallets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const APP_ID: &str = "test-privy-app-id";
    const NOW: i64 = 1_788_868_800;

    fn honest_claims() -> Value {
        json!({
            "iss": "privy.io",
            "aud": APP_ID,
            "sub": "did:privy:cm2testsubject",
            "exp": NOW + 600,
            "linked_accounts": "[{\"type\":\"email\",\"address\":\"a@b.c\"},{\"type\":\"wallet\",\"address\":\"0x11\"}]",
        })
    }

    fn with_accounts(accounts: Value) -> Value {
        let mut payload = honest_claims();
        payload["linked_accounts"] = accounts;
        payload
    }

    fn encode(value: &Value) -> String {
        BASE64URL.encode(value.to_string())
    }

    #[test]
    fn honest_claims_read_as_an_identity() {
        assert_eq!(
            claims(&honest_claims(), APP_ID, NOW),
            Some(PrivyIdentity {
                user_id: "did:privy:cm2testsubject".to_owned(),
                email: Some("a@b.c".to_owned()),
                wallets: vec!["0x11".to_owned()],
            })
        );
    }

    #[test]
    fn a_fractional_exp_after_now_is_still_live() {
        let mut payload = honest_claims();
        payload["exp"] = json!(NOW as f64 + 0.5);
        assert!(claims(&payload, APP_ID, NOW).is_some());
    }

    #[test]
    fn a_null_linked_account_rejects_the_token() {
        let payload = with_accounts(json!([null, { "type": "wallet", "address": "0x11" }]));
        assert_eq!(claims(&payload, APP_ID, NOW), None);
    }

    #[test]
    fn a_wallet_address_that_is_not_a_string_rejects_the_token() {
        let payload = with_accounts(json!([{ "type": "wallet", "address": 17 }]));
        assert_eq!(claims(&payload, APP_ID, NOW), None);
    }

    #[test]
    fn an_empty_email_address_reads_as_no_email() {
        let payload = with_accounts(json!([{ "type": "email", "address": "" }]));
        assert_eq!(claims(&payload, APP_ID, NOW).and_then(|id| id.email), None);
    }

    #[test]
    fn only_the_first_email_account_is_read() {
        let payload = with_accounts(json!([
            { "type": "email" },
            { "type": "email", "address": "later@b.c" },
        ]));
        let identity = claims(&payload, APP_ID, NOW);
        assert_eq!(identity.map(|id| id.email), Some(None));
    }

    #[test]
    fn an_email_address_that_cannot_be_lower_cased_rejects_the_token() {
        let payload = with_accounts(json!([{ "type": "email", "address": true }]));
        assert_eq!(claims(&payload, APP_ID, NOW), None);
    }

    #[test]
    fn a_non_array_linked_accounts_claim_reads_as_no_accounts() {
        let identity = claims(&with_accounts(json!({ "type": "email" })), APP_ID, NOW);
        assert_eq!(identity.map(|id| id.wallets), Some(Vec::new()));
    }

    #[test]
    fn a_payload_that_is_not_an_object_is_rejected() {
        assert_eq!(claims(&json!(null), APP_ID, NOW), None);
        assert_eq!(claims(&json!([1, 2]), APP_ID, NOW), None);
    }

    #[test]
    fn falsy_kids_name_no_key_and_others_do() {
        for unnamed in [
            None,
            Some(json!(null)),
            Some(json!("")),
            Some(json!(0)),
            Some(json!(false)),
        ] {
            assert!(!KeyId(unnamed.clone()).is_named(), "{unnamed:?}");
        }
        for named in [json!("k"), json!(1), json!(true), json!([]), json!({})] {
            assert!(KeyId(Some(named.clone())).is_named(), "{named}");
        }
    }

    #[test]
    fn an_absent_kid_matches_an_entry_without_one_before_falling_back() {
        let keys = [json!({ "kid": "a" }), json!({ "x": "no kid" })];
        assert_eq!(match_key(&keys, &KeyId(None)), Some(&keys[1]));
    }

    #[test]
    fn a_kid_is_compared_by_json_value_not_by_its_text() {
        let keys = [json!({ "kid": "1" }), json!({ "kid": "2" })];
        assert_eq!(match_key(&keys, &KeyId(Some(json!(1)))), None);
    }

    #[test]
    fn parse_rejects_a_header_that_is_json_null() {
        let token = format!("{}.{}.c2ln", encode(&json!(null)), encode(&honest_claims()));
        assert!(parse(&token).is_none());
    }

    #[test]
    fn parse_keeps_the_signing_input_as_the_first_two_segments() {
        let head = encode(&json!({ "alg": "ES256", "kid": "k" }));
        let token = format!("{head}.cGF5bG9hZA.c2ln");
        let parsed = parse(&token);
        assert_eq!(
            parsed.as_ref().map(|t| t.signing_input),
            Some(&*format!("{head}.cGF5bG9hZA"))
        );
        assert_eq!(parsed.map(|t| t.kid), Some(KeyId(Some(json!("k")))));
    }

    #[test]
    fn padded_base64url_segments_decode_like_unpadded_ones() {
        assert_eq!(decode_json("eyJhIjoxfQ=="), Some(json!({ "a": 1 })));
        assert_eq!(decode_json("eyJhIjoxfQ"), Some(json!({ "a": 1 })));
    }

    #[test]
    fn a_jwk_on_another_curve_verifies_nothing() {
        let jwk = json!({ "kty": "EC", "crv": "P-384", "x": "", "y": "" });
        assert!(verifying_key(&jwk).is_none());
    }

    #[test]
    fn a_point_off_the_curve_is_not_a_key() {
        let one = BASE64URL.encode([1_u8; 32]);
        let jwk = json!({ "kty": "EC", "crv": "P-256", "x": one, "y": one });
        assert!(verifying_key(&jwk).is_none());
    }

    #[test]
    fn jwks_keys_reads_absent_or_null_keys_as_an_empty_set() {
        assert_eq!(jwks_keys(b"{}"), Ok(Vec::new()));
        assert_eq!(jwks_keys(b"{\"keys\":null}"), Ok(Vec::new()));
        assert_eq!(jwks_keys(b"{\"keys\":[1]}"), Ok(vec![json!(1)]));
    }

    #[test]
    fn jwks_keys_refuses_a_body_it_cannot_read_a_key_set_from() {
        assert_eq!(jwks_keys(b"<html>"), Err(JwksBodyError::NotJson));
        assert_eq!(jwks_keys(b"null"), Err(JwksBodyError::Null));
        assert_eq!(
            jwks_keys(b"{\"keys\":{}}"),
            Err(JwksBodyError::KeysNotAnArray)
        );
    }
}

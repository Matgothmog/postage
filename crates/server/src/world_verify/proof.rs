//! Reading the request and checking the proof it carries, before anything is
//! spent or anyone is asked.
//!
//! This is an identity-verification boundary, so the body is untrusted input
//! all the way down to its shape. Only the fields this route inspects are
//! read; the proof itself goes on to World untouched, since checking its
//! cryptography is World's job.

use axum::body::Bytes;
use axum::http::StatusCode;
use postage_core::signal::hash_signal;
use serde_json::Value;

use super::Stopped;
use crate::routes::js::{field, is_truthy, lookup_text, request_json};

const SELFIE_IDENTIFIER: &str = "selfie";

/// `{ token, proof }` as the route reads them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VerifyRequest {
    pub token: String,
    /// Absent (or `null`) in mock mode, where nothing reads it.
    pub proof: Option<Value>,
}

/// The body, refused before any lookup if no token can be read off it.
///
/// A non-object body has no token, and neither does `null`, where
/// destructuring threw in the TypeScript (a bare 500). A token that is a
/// number is looked up by its decimal text as it always was; one that is an
/// array or object could not be bound to a query at all and is refused.
pub(crate) fn read_request(body: &Bytes) -> Result<VerifyRequest, Stopped> {
    let Ok(body) = request_json(body) else {
        return Err(Stopped::refused(
            StatusCode::BAD_REQUEST,
            "Body must be JSON",
        ));
    };
    let Some(token) = field(&body, "token").filter(|token| is_truthy(Some(token))) else {
        return Err(Stopped::refused(
            StatusCode::BAD_REQUEST,
            "A challenge token is required",
        ));
    };
    let token = lookup_text(token)
        .map_err(|_| Stopped::refused(StatusCode::BAD_REQUEST, "Invalid request"))?;
    Ok(VerifyRequest {
        token,
        proof: field(&body, "proof").cloned(),
    })
}

/// The IDKit result as far as this route reads it. `raw` is what goes on to
/// World, whole.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WorldIdProof<'a> {
    pub raw: &'a Value,
    /// The request this proof answers: what ties it to a context we signed.
    pub nonce: &'a str,
    pub responses: Vec<ProofResponse<'a>>,
}

/// One credential response inside the proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProofResponse<'a> {
    pub identifier: &'a str,
    /// Present only when the request that produced the proof carried a
    /// signal, which is why its absence is refused below.
    pub signal_hash: Option<&'a str>,
}

/// Fails loudly and specifically rather than letting a missing or malformed
/// proof reach World.
pub(crate) fn require_world_id_proof(proof: Option<&Value>) -> Result<WorldIdProof<'_>, Stopped> {
    let Some(proof) = proof.filter(|proof| !proof.is_null()) else {
        return Err(Stopped::refused(
            StatusCode::BAD_REQUEST,
            "A World ID proof is required",
        ));
    };
    parse_proof(proof)
        .ok_or_else(|| Stopped::refused(StatusCode::BAD_REQUEST, "Malformed World ID proof"))
}

/// `isWorldIdProof`: an object with a string `nonce` and a non-empty
/// `responses` array whose every entry is an object with a string
/// `identifier` and, if it has one, a string `signal_hash`.
fn parse_proof(proof: &Value) -> Option<WorldIdProof<'_>> {
    let Value::Object(fields) = proof else {
        return None;
    };
    let Some(Value::String(nonce)) = fields.get("nonce") else {
        return None;
    };
    let Some(Value::Array(items)) = fields.get("responses") else {
        return None;
    };
    if items.is_empty() {
        return None;
    }
    let responses = items.iter().map(parse_response).collect::<Option<_>>()?;
    Some(WorldIdProof {
        raw: proof,
        nonce,
        responses,
    })
}

fn parse_response(item: &Value) -> Option<ProofResponse<'_>> {
    let Value::Object(fields) = item else {
        return None;
    };
    let Some(Value::String(identifier)) = fields.get("identifier") else {
        return None;
    };
    let signal_hash = match fields.get("signal_hash") {
        None => None,
        Some(Value::String(hash)) => Some(hash.as_str()),
        Some(_) => return None,
    };
    Some(ProofResponse {
        identifier,
        signal_hash,
    })
}

/// The one Selfie Check credential a proof may carry; zero or more than one
/// is the same refusal.
///
/// More than one matters because the signal is checked on the request's
/// entry while the nullifier is read off World's response, and nothing in the
/// response says which request entry it answers. With two, a stub carrying
/// this challenge's `signal_hash` could vouch for a harvested proof riding
/// beside it. With exactly one, the entry checked and the entry consumed
/// cannot differ.
fn require_single_selfie_credential<'a>(
    responses: &[ProofResponse<'a>],
) -> Result<ProofResponse<'a>, Stopped> {
    let mut selfies = responses
        .iter()
        .filter(|response| response.identifier == SELFIE_IDENTIFIER);
    match (selfies.next(), selfies.next()) {
        (Some(selfie), None) => Ok(*selfie),
        _ => Err(Stopped::refused(
            StatusCode::BAD_REQUEST,
            "Proof must include exactly one Selfie Check credential",
        )),
    }
}

/// Refuses a proof made for some other challenge, or bound to none.
///
/// The signal hash is a public input to the proof, so it cannot be edited
/// without the proof failing at World: a proof that verifies there and
/// carries this token's hash was made for this token and no other. World does
/// not echo the signal back, so the presented proof is the only place to
/// read it.
pub(crate) fn require_bound_to_challenge(
    proof: &WorldIdProof<'_>,
    token: &str,
) -> Result<(), Stopped> {
    let selfie = require_single_selfie_credential(&proof.responses)?;
    let expected = hash_signal(token).to_lowercase();
    if selfie.signal_hash.map(str::to_lowercase) != Some(expected) {
        return Err(Stopped::refused(
            StatusCode::BAD_REQUEST,
            "This proof was not made for this challenge",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn refusal_of<T: std::fmt::Debug>(result: Result<T, Stopped>) -> (StatusCode, String) {
        match result.unwrap_err() {
            Stopped::Refused { status, error } => (status, error),
            Stopped::Failed(error) => panic!("expected a refusal, got {error}"),
        }
    }

    fn read(body: &str) -> Result<VerifyRequest, Stopped> {
        read_request(&Bytes::from(body.to_owned()))
    }

    #[test]
    fn reads_the_token_and_the_proof() {
        let request = read(r#"{"token":"tok","proof":{"nonce":"n"}}"#).unwrap();
        assert_eq!(request.token, "tok");
        assert_eq!(request.proof, Some(json!({ "nonce": "n" })));
    }

    #[test]
    fn a_body_that_is_not_json_is_refused_in_the_routes_own_words() {
        for body in ["not json", ""] {
            assert_eq!(
                refusal_of(read(body)),
                (StatusCode::BAD_REQUEST, "Body must be JSON".to_owned()),
                "{body}"
            );
        }
    }

    #[test]
    fn a_body_without_a_usable_token_is_refused_before_any_lookup() {
        for body in [
            "null",
            "[]",
            "42",
            r#""tok""#,
            "{}",
            r#"{"token":""}"#,
            r#"{"token":null}"#,
            r#"{"token":0}"#,
            r#"{"token":false}"#,
        ] {
            assert_eq!(
                refusal_of(read(body)),
                (
                    StatusCode::BAD_REQUEST,
                    "A challenge token is required".to_owned()
                ),
                "{body}"
            );
        }
    }

    #[test]
    fn a_token_no_query_could_bind_is_an_invalid_request() {
        for body in [r#"{"token":[]}"#, r#"{"token":{"a":1}}"#] {
            assert_eq!(
                refusal_of(read(body)),
                (StatusCode::BAD_REQUEST, "Invalid request".to_owned()),
                "{body}"
            );
        }
    }

    #[test]
    fn a_number_token_is_read_as_its_decimal_text() {
        assert_eq!(read(r#"{"token":42}"#).unwrap().token, "42");
    }

    #[test]
    fn a_missing_or_null_proof_is_required() {
        for proof in [None, Some(Value::Null)] {
            assert_eq!(
                refusal_of(require_world_id_proof(proof.as_ref())),
                (
                    StatusCode::BAD_REQUEST,
                    "A World ID proof is required".to_owned()
                )
            );
        }
    }

    #[test]
    fn every_shape_is_world_id_proof_refuses_is_malformed() {
        for proof in [
            json!({ "not": "a proof" }),
            json!("proof"),
            json!([]),
            json!(5),
            json!({ "nonce": "n" }),
            json!({ "responses": [{ "identifier": "selfie" }] }),
            json!({ "nonce": 5, "responses": [{ "identifier": "selfie" }] }),
            json!({ "nonce": "n", "responses": [] }),
            json!({ "nonce": "n", "responses": {} }),
            json!({ "nonce": "n", "responses": [null] }),
            json!({ "nonce": "n", "responses": [["selfie"]] }),
            json!({ "nonce": "n", "responses": [{ "identifier": 1 }] }),
            json!({ "nonce": "n", "responses": [{ "identifier": "selfie", "signal_hash": null }] }),
            json!({ "nonce": "n", "responses": [{ "identifier": "selfie", "signal_hash": 7 }] }),
            json!({ "nonce": "n", "responses": [{ "identifier": "selfie" }, 3] }),
        ] {
            assert_eq!(
                refusal_of(require_world_id_proof(Some(&proof))),
                (
                    StatusCode::BAD_REQUEST,
                    "Malformed World ID proof".to_owned()
                ),
                "{proof}"
            );
        }
    }

    #[test]
    fn a_well_formed_proof_keeps_the_whole_object_for_world() {
        let proof = json!({
            "protocol_version": "3.0",
            "nonce": "n",
            "responses": [{ "identifier": "selfie", "signal_hash": "0x01", "proof": "0xp" }],
        });

        let parsed = require_world_id_proof(Some(&proof)).unwrap();

        assert_eq!(parsed.raw, &proof);
        assert_eq!(parsed.nonce, "n");
        assert_eq!(
            parsed.responses,
            [ProofResponse {
                identifier: "selfie",
                signal_hash: Some("0x01")
            }]
        );
    }

    fn bound(responses: Value) -> Result<(), Stopped> {
        let proof = json!({ "nonce": "n", "responses": responses });
        let parsed = require_world_id_proof(Some(&proof)).unwrap();
        require_bound_to_challenge(&parsed, "tok")
    }

    #[test]
    fn a_proof_carrying_this_tokens_signal_hash_is_bound_to_it() {
        assert!(
            bound(json!([{ "identifier": "selfie", "signal_hash": hash_signal("tok") }])).is_ok()
        );
    }

    #[test]
    fn the_signal_hash_comparison_ignores_case() {
        let upper = hash_signal("tok").to_uppercase().replace("0X", "0x");
        assert!(bound(json!([{ "identifier": "selfie", "signal_hash": upper }])).is_ok());
    }

    #[test]
    fn a_proof_for_another_token_or_none_is_not_bound() {
        for responses in [
            json!([{ "identifier": "selfie", "signal_hash": hash_signal("other") }]),
            json!([{ "identifier": "selfie" }]),
        ] {
            assert_eq!(
                refusal_of(bound(responses)),
                (
                    StatusCode::BAD_REQUEST,
                    "This proof was not made for this challenge".to_owned()
                )
            );
        }
    }

    #[test]
    fn zero_or_two_selfie_credentials_are_refused_alike() {
        let hash = hash_signal("tok");
        for responses in [
            json!([{ "identifier": "orb", "signal_hash": hash }]),
            json!([
                { "identifier": "selfie", "signal_hash": hash },
                { "identifier": "selfie", "signal_hash": hash },
            ]),
        ] {
            assert_eq!(
                refusal_of(bound(responses)),
                (
                    StatusCode::BAD_REQUEST,
                    "Proof must include exactly one Selfie Check credential".to_owned()
                )
            );
        }
    }

    #[test]
    fn other_credentials_beside_the_one_selfie_are_tolerated() {
        let responses = json!([
            { "identifier": "orb" },
            { "identifier": "selfie", "signal_hash": hash_signal("tok") },
        ]);
        assert!(bound(responses).is_ok());
    }
}

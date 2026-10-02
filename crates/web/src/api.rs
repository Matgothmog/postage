//! The inbox, claim and wallet-nonce endpoints, typed. Replaces the `fetch`
//! calls scattered through `Account.tsx`, `ClaimStrip.tsx`,
//! `pending-claim.ts` and `lib/wallet-proof.ts`.
//!
//! Each endpoint is two pieces. A pure function turns the answer (a status and
//! a body, or a transport failure) into a typed outcome, and runs natively
//! under test; a thin `async` function builds the request, sends it through
//! `http`, and hands the answer to the pure one. The server answers a
//! malformed body with `400 {"error":"Invalid request"}`; none of these ever
//! sends one.

use alloy_primitives::Address;
use postage_core::wallet_proof::{IDENTITY_TOKEN_HEADER, ProofHeaders, WALLET_NONCE_PATH};
use serde::{Deserialize, Serialize};
use web_sys::AbortSignal;

use crate::http::{self, HttpError, HttpRequest, HttpResponse};
use crate::pending_claim::verify_claim_url;

pub const INBOX_PATH: &str = "/api/inbox";
pub const INBOX_VERIFY_PATH: &str = "/api/inbox/verify";

/// The inbox row, as far as the UI reads it (the server also sends the
/// wallet and creation time).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Inbox {
    pub handle: String,
    pub destination: String,
}

/// Why the inbox could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxErrorKind {
    /// The proof of the wallet was turned down; only signing in again helps.
    Unauthorized,
    /// Anything else: the server could not be asked, or did not answer well.
    Network,
}

/// Whether the route turned down the proof rather than failing to read it.
/// 401 is the signature that did not verify; 400 is the proof that named no
/// wallet at all, which is what a session offering an expired identity token
/// and nothing else sends. Both are permanent until the user signs in again,
/// so neither may be dressed up as a transient fault with a retry that cannot
/// work.
pub fn refused_the_proof(status: u16) -> bool {
    matches!(status, 400 | 401)
}

/// What one answer from `GET /api/inbox` says. A non-ok answer is never "no
/// inbox": that would show someone who already claimed one the claim flow
/// again, inviting them to fight themselves for their own handle.
pub fn inbox_outcome(
    answer: Result<HttpResponse, HttpError>,
) -> Result<Option<Inbox>, InboxErrorKind> {
    #[derive(Deserialize)]
    struct Reply {
        inbox: Option<Inbox>,
    }
    let response = answer.map_err(|_| InboxErrorKind::Network)?;
    if !response.is_ok() {
        return Err(if refused_the_proof(response.status) {
            InboxErrorKind::Unauthorized
        } else {
            InboxErrorKind::Network
        });
    }
    serde_json::from_str::<Reply>(&response.body)
        .map(|reply| reply.inbox)
        .map_err(|_| InboxErrorKind::Network)
}

/// `GET /api/inbox` under `proof`.
pub async fn get_inbox(proof: ProofHeaders) -> Result<Option<Inbox>, InboxErrorKind> {
    let request = HttpRequest::get(INBOX_PATH).with_headers(proof);
    inbox_outcome(http::send(&request, None).await)
}

/// A refusal or failure in the words the user reads.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ApiError(pub String);

const UNREACHABLE: &str = "Could not reach the server. Check your connection and try again";

/// The server's own words for a refusal (`{"error": "..."}`), when it gave
/// some.
fn error_text(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Refusal {
        error: Option<String>,
    }
    serde_json::from_str::<Refusal>(body).ok()?.error
}

/// Splits an answer into its body (2xx) or the refusal's words (anything
/// else, or no answer), using `fallback` when the server said nothing usable.
fn body_or_refusal(
    answer: Result<HttpResponse, HttpError>,
    fallback: &str,
) -> Result<String, ApiError> {
    let response = answer.map_err(|_| ApiError(UNREACHABLE.to_owned()))?;
    if response.is_ok() {
        return Ok(response.body);
    }
    Err(ApiError(
        error_text(&response.body).unwrap_or_else(|| fallback.to_owned()),
    ))
}

/// Exactly what `/api/inbox/verify` answers with, from either verb. `error`
/// is only ever present on a refusal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClaimState {
    pub code_verified: bool,
    pub cloudflare_verified: bool,
    pub live: bool,
    /// The server has stopped asking Cloudflare about this claim, so polling
    /// it can only ever return the same answer.
    pub stalled: bool,
}

/// What `POST /api/inbox` answers with once a claim has started.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClaimReply {
    pub handle: String,
    pub destination: String,
    pub code_verified: bool,
    pub cloudflare_verified: bool,
    pub live: bool,
}

/// The signature half of a claim, sent in the body (the claim route reads its
/// proof from there, not from headers).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedClaim {
    /// Whole seconds, so it travels as the integer the server's `Number()`
    /// reads back unchanged.
    pub issued_at: i64,
    pub signature: String,
    pub nonce: String,
}

/// The claim a user asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRequest {
    pub handle: String,
    pub destination: String,
    pub wallet: Address,
}

#[derive(Serialize)]
struct ClaimBody<'a> {
    handle: &'a str,
    destination: &'a str,
    wallet: String,
    #[serde(flatten)]
    proof: Option<&'a SignedClaim>,
}

const CLAIM_FAILED: &str = "Could not claim that address";
const CODE_CHECK_FAILED: &str = "Could not check that code";

/// The claim route's answer, or the refusal's words.
pub fn claim_reply_of(answer: Result<HttpResponse, HttpError>) -> Result<ClaimReply, ApiError> {
    let body = body_or_refusal(answer, CLAIM_FAILED)?;
    serde_json::from_str(&body).map_err(|_| ApiError(CLAIM_FAILED.to_owned()))
}

/// `POST /api/inbox`: starts the claim server-side. The identity token
/// travels as a header when there is one; the signature, when there is one,
/// in the body.
pub async fn post_claim(
    identity_token: Option<&str>,
    claim: &ClaimRequest,
    proof: Option<&SignedClaim>,
) -> Result<ClaimReply, ApiError> {
    let body = ClaimBody {
        handle: &claim.handle,
        destination: &claim.destination,
        wallet: claim.wallet.to_string(),
        proof,
    };
    let body = serde_json::to_string(&body).map_err(|_| ApiError(CLAIM_FAILED.to_owned()))?;
    let mut request = HttpRequest::post_json(INBOX_PATH, body);
    if let Some(token) = identity_token {
        request = request.with_headers([(IDENTITY_TOKEN_HEADER, token)]);
    }
    claim_reply_of(http::send(&request, None).await)
}

/// The confirm route's answer, or the refusal's words. The server's own words
/// are better than any picked from a status.
pub fn claim_state_of(answer: Result<HttpResponse, HttpError>) -> Result<ClaimState, ApiError> {
    let body = body_or_refusal(answer, CODE_CHECK_FAILED)?;
    serde_json::from_str(&body).map_err(|_| ApiError(CODE_CHECK_FAILED.to_owned()))
}

/// `POST /api/inbox/verify`: the code from the email, with the wallet proof.
pub async fn post_confirmation(
    handle: &str,
    code: &str,
    proof: ProofHeaders,
) -> Result<ClaimState, ApiError> {
    let body = serde_json::json!({ "handle": handle, "code": code }).to_string();
    let request = HttpRequest::post_json(INBOX_VERIFY_PATH, body).with_headers(proof);
    claim_state_of(http::send(&request, None).await)
}

/// `GET /api/inbox/verify?handle=`: where the claim has got to. Unauthenticated
/// by design (the poll runs before the visitor has agreed to anything), and
/// the raw answer is returned because `poll_verdict` decides what a status
/// means.
pub async fn get_claim_state(
    handle: &str,
    signal: Option<&AbortSignal>,
) -> Result<HttpResponse, HttpError> {
    http::send(&HttpRequest::get(verify_claim_url(handle)), signal).await
}

/// Every way of not getting a nonce is one thing to whoever clicked the
/// button: there is nothing they can do differently about a 503, an HTML
/// error page, or a body with no nonce in it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Could not start a signature. Try again in a moment")]
pub struct NonceUnavailable;

/// The nonce in the answer, or `NonceUnavailable`.
pub fn nonce_of(answer: Result<HttpResponse, HttpError>) -> Result<String, NonceUnavailable> {
    #[derive(Deserialize)]
    struct Reply {
        nonce: Option<String>,
    }
    let response = answer.map_err(|_| NonceUnavailable)?;
    if !response.is_ok() {
        return Err(NonceUnavailable);
    }
    // A gateway that answered with something other than this route's JSON (an
    // HTML error page, an empty body) reads as the same unavailability.
    serde_json::from_str::<Reply>(&response.body)
        .ok()
        .and_then(|reply| reply.nonce)
        .filter(|nonce| !nonce.is_empty())
        .ok_or(NonceUnavailable)
}

/// `POST /api/wallet-nonce`: the nonce a wallet is about to sign under. There
/// is no degraded proof to fall back to: a signature collected without one is
/// refused by every reader, so carrying on would only move the failure to a
/// place where it reads as "your wallet is not yours".
pub async fn request_wallet_nonce(wallet: Address) -> Result<String, NonceUnavailable> {
    let body = serde_json::json!({ "wallet": wallet.to_string() }).to_string();
    let request = HttpRequest::post_json(WALLET_NONCE_PATH, body);
    nonce_of(http::send(&request, None).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(status: u16, body: &str) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse {
            status,
            body: body.to_owned(),
        })
    }

    fn offline() -> Result<HttpResponse, HttpError> {
        Err(HttpError::Network("offline".to_owned()))
    }

    #[test]
    fn only_400_and_401_count_as_a_refused_proof() {
        assert!(refused_the_proof(400) && refused_the_proof(401));
        for status in [200, 403, 500, 502] {
            assert!(!refused_the_proof(status), "{status}");
        }
    }

    #[test]
    fn a_400_is_unauthorized_not_network_and_never_an_inbox() {
        let outcome = inbox_outcome(answer(400, r#"{"inbox":null}"#));
        assert_eq!(outcome, Err(InboxErrorKind::Unauthorized));
    }

    #[test]
    fn a_502_is_still_network_not_unauthorized() {
        let outcome = inbox_outcome(answer(502, r#"{"inbox":null}"#));
        assert_eq!(outcome, Err(InboxErrorKind::Network));
    }

    #[test]
    fn an_answer_with_an_inbox_reads_the_handle_and_destination_and_ignores_the_rest() {
        let outcome = inbox_outcome(answer(
            200,
            r#"{"inbox":{"handle":"demo","destination":"d@example.com","wallet":"0xab","created_at":1}}"#,
        ));
        assert_eq!(
            outcome,
            Ok(Some(Inbox {
                handle: "demo".to_owned(),
                destination: "d@example.com".to_owned()
            }))
        );
    }

    #[test]
    fn an_ok_answer_with_no_inbox_is_no_inbox() {
        assert_eq!(inbox_outcome(answer(200, r#"{"inbox":null}"#)), Ok(None));
    }

    #[test]
    fn an_unreadable_or_missing_answer_is_network() {
        assert_eq!(
            inbox_outcome(answer(200, "<html>gateway</html>")),
            Err(InboxErrorKind::Network)
        );
        assert_eq!(inbox_outcome(offline()), Err(InboxErrorKind::Network));
    }

    #[test]
    fn a_claim_refusal_carries_the_servers_own_words() {
        let error = claim_reply_of(answer(409, r#"{"error":"That handle is taken"}"#));
        assert_eq!(error, Err(ApiError("That handle is taken".to_owned())));
    }

    #[test]
    fn a_claim_refusal_without_words_falls_back_to_a_plain_sentence() {
        let error = claim_reply_of(answer(502, "<html>bad gateway</html>"));
        assert_eq!(error, Err(ApiError(CLAIM_FAILED.to_owned())));
        assert_eq!(
            claim_reply_of(offline()),
            Err(ApiError(UNREACHABLE.to_owned()))
        );
    }

    #[test]
    fn a_started_claim_reads_its_progress() {
        let reply = claim_reply_of(answer(
            200,
            r#"{"status":"code_sent","handle":"demo","destination":"d@example.com","codeVerified":false,"cloudflareVerified":false,"live":false,"expiresIn":600}"#,
        ))
        .unwrap();
        assert_eq!(reply.handle, "demo");
        assert!(!reply.live);
    }

    #[test]
    fn a_wrong_code_shows_the_servers_words_and_a_silent_failure_a_fallback() {
        let words = claim_state_of(answer(400, r#"{"error":"That code is wrong"}"#));
        assert_eq!(words, Err(ApiError("That code is wrong".to_owned())));
        let silent = claim_state_of(answer(500, ""));
        assert_eq!(silent, Err(ApiError(CODE_CHECK_FAILED.to_owned())));
    }

    #[test]
    fn a_confirmation_reads_every_flag_and_defaults_the_missing_ones() {
        let state = claim_state_of(answer(
            200,
            r#"{"codeVerified":true,"cloudflareVerified":false,"live":false}"#,
        ))
        .unwrap();
        assert!(state.code_verified && !state.stalled && !state.live);
    }

    #[test]
    fn a_claim_body_carries_the_proof_inline_only_when_there_is_one() {
        let signed = SignedClaim {
            issued_at: 1_757_332_800,
            signature: "0xsig".to_owned(),
            nonce: "n".to_owned(),
        };
        let with = serde_json::to_value(ClaimBody {
            handle: "demo",
            destination: "d@example.com",
            wallet: "0xW".to_owned(),
            proof: Some(&signed),
        })
        .unwrap();
        assert_eq!(
            with,
            serde_json::json!({
                "handle": "demo", "destination": "d@example.com", "wallet": "0xW",
                "issuedAt": 1_757_332_800, "signature": "0xsig", "nonce": "n"
            })
        );
        let without = serde_json::to_value(ClaimBody {
            handle: "demo",
            destination: "d@example.com",
            wallet: "0xW".to_owned(),
            proof: None,
        })
        .unwrap();
        assert_eq!(
            without,
            serde_json::json!({"handle": "demo", "destination": "d@example.com", "wallet": "0xW"})
        );
    }

    #[test]
    fn a_nonce_is_read_from_a_good_answer() {
        assert_eq!(
            nonce_of(answer(200, r#"{"nonce":"abc","expiresIn":300}"#)),
            Ok("abc".to_owned())
        );
    }

    #[test]
    fn every_way_of_not_getting_a_nonce_is_the_same_unavailability() {
        for bad in [
            answer(503, r#"{"error":"no"}"#),
            answer(200, "<html>gateway</html>"),
            answer(200, ""),
            answer(200, r#"{"nonce":""}"#),
            answer(200, r#"{"nonce":7}"#),
            answer(200, r#"{}"#),
            offline(),
        ] {
            assert_eq!(nonce_of(bad), Err(NonceUnavailable));
        }
    }
}

//! The challenge endpoints, typed: what the page shows
//! (`GET /api/challenge/{token}`, new in Rust; the Next page read the database
//! directly), the payment check (`POST /api/challenge/resolve`) and the pasted
//! message (`POST /api/challenge/deliver`).
//!
//! The same shape as `api`: a pure function turns an answer into a typed
//! outcome and runs natively under test; a thin `async` function sends the
//! request and hands the answer over.

use postage_core::quote_types::StoredQuote;
use serde::Deserialize;

use crate::api::{ApiError, UNREACHABLE, error_text};
use crate::http::{self, HttpError, HttpRequest, HttpResponse};

pub const RESOLVE_PATH: &str = "/api/challenge/resolve";
pub const DELIVER_PATH: &str = "/api/challenge/deliver";

/// Which identity check the server enforces. Decided there and handed down
/// with the challenge, not read from a public build variable that could
/// disagree with what `/api/world/verify` actually requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdentityMode {
    Live,
    Mock,
}

/// A challenge that can still be answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenChallenge {
    /// The bare handle; shown as an address.
    pub handle: String,
    pub dangerous: bool,
    /// Whether the message itself is still on hold ("HELD") or was bounced
    /// ("BOUNCED").
    pub held: bool,
    pub quote: StoredQuote,
    /// `quote.quote.amount` read as base units; checked once, on arrival.
    pub amount: u128,
    pub identity_mode: IdentityMode,
}

/// The three states the page had on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChallengeView {
    /// Settled already: "That one's dealt with."
    Resolved,
    /// The stored quote is not one the page can offer: "Write again for a
    /// fresh one."
    Dead,
    /// Boxed: by far the largest state, and most others carry nothing.
    Open(Box<OpenChallenge>),
}

/// Why the challenge could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewError {
    /// No such token.
    NotFound,
    /// Anything else: offline, a server fault, an answer that does not read.
    Unavailable,
}

#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
enum Wire {
    Resolved,
    Dead,
    #[serde(rename_all = "camelCase")]
    Open {
        handle: String,
        dangerous: bool,
        held: bool,
        quote: StoredQuote,
        identity_mode: IdentityMode,
    },
}

/// What one answer from `GET /api/challenge/{token}` says. A 404 is the one
/// answer that means "no such challenge"; a fault must never be dressed up as
/// one, or a sender is told their link is dead when the server merely blinked.
pub fn view_outcome(answer: Result<HttpResponse, HttpError>) -> Result<ChallengeView, ViewError> {
    let response = answer.map_err(|_| ViewError::Unavailable)?;
    if response.status == 404 {
        return Err(ViewError::NotFound);
    }
    if !response.is_ok() {
        return Err(ViewError::Unavailable);
    }
    match serde_json::from_str::<Wire>(&response.body).map_err(|_| ViewError::Unavailable)? {
        Wire::Resolved => Ok(ChallengeView::Resolved),
        Wire::Dead => Ok(ChallengeView::Dead),
        Wire::Open {
            handle,
            dangerous,
            held,
            quote,
            identity_mode,
        } => {
            // An amount the page cannot print or pay is an offer it cannot
            // make: the same dead end as a quote that does not parse.
            let Ok(amount) = quote.quote.amount.parse::<u128>() else {
                return Ok(ChallengeView::Dead);
            };
            Ok(ChallengeView::Open(Box::new(OpenChallenge {
                handle,
                dangerous,
                held,
                quote,
                amount,
                identity_mode,
            })))
        }
    }
}

/// `GET /api/challenge/{token}`.
pub async fn get_challenge(token: &str) -> Result<ChallengeView, ViewError> {
    let path = format!(
        "/api/challenge/{}",
        js_sys::encode_uri_component(token)
            .as_string()
            .unwrap_or_default()
    );
    view_outcome(http::send(&HttpRequest::get(path), None).await)
}

/// What `POST /api/challenge/resolve` says about a payment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Cleared {
        delivered: bool,
    },
    Charged,
    /// Not (yet) cleared: the chain has not shown the payment, the server was
    /// unreachable, or the answer did not read. All of these are one more "not
    /// yet" for the caller to retry, never a verdict on a payment that has
    /// already succeeded.
    NotYet,
}

/// Reads a resolve answer. Never fails: the status field decides, whatever the
/// HTTP status, exactly as `askOnce` read it.
pub fn resolution_of(answer: Result<HttpResponse, HttpError>) -> Resolution {
    #[derive(Deserialize)]
    struct Reply {
        status: Option<String>,
        delivered: Option<bool>,
    }
    let Ok(response) = answer else {
        return Resolution::NotYet;
    };
    let Ok(reply) = serde_json::from_str::<Reply>(&response.body) else {
        return Resolution::NotYet;
    };
    match reply.status.as_deref() {
        Some("cleared") => Resolution::Cleared {
            delivered: reply.delivered == Some(true),
        },
        Some("charged") => Resolution::Charged,
        _ => Resolution::NotYet,
    }
}

/// `POST /api/challenge/resolve`: has the payment landed, and what did the
/// gate make of it.
pub async fn post_resolve(token: &str) -> Resolution {
    let body = serde_json::json!({ "token": token }).to_string();
    resolution_of(http::send(&HttpRequest::post_json(RESOLVE_PATH, body), None).await)
}

const DELIVER_FAILED: &str = "Could not deliver it";

/// Whether the paste went out, or the server's words for why not. An ok status
/// with a body that is not JSON is not delivery: a proxy's interstitial must
/// not read as "Sent."
pub fn delivery_outcome(answer: Result<HttpResponse, HttpError>) -> Result<(), ApiError> {
    let response = answer.map_err(|_| ApiError(UNREACHABLE.to_owned()))?;
    let readable = serde_json::from_str::<serde_json::Value>(&response.body).is_ok();
    if response.is_ok() && readable {
        return Ok(());
    }
    Err(ApiError(
        error_text(&response.body).unwrap_or_else(|| DELIVER_FAILED.to_owned()),
    ))
}

/// `POST /api/challenge/deliver`.
pub async fn post_deliver(token: &str, subject: &str, body: &str) -> Result<(), ApiError> {
    let payload = serde_json::json!({ "token": token, "subject": subject, "body": body });
    let request = HttpRequest::post_json(DELIVER_PATH, payload.to_string());
    delivery_outcome(http::send(&request, None).await)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn answer(status: u16, body: serde_json::Value) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse {
            status,
            body: body.to_string(),
        })
    }

    fn offline() -> Result<HttpResponse, HttpError> {
        Err(HttpError::Network("offline".to_owned()))
    }

    fn quote(amount: &str) -> serde_json::Value {
        json!({
            "messageId": format!("0x{}", "ab".repeat(32)),
            "inbox": format!("0x{}", "cd".repeat(20)),
            "tier": "commercial",
            "amount": amount,
            "expiresAt": 1_800_000_000u64,
            "signature": format!("0x{}", "ef".repeat(65)),
            "reasons": ["looked automated"],
        })
    }

    fn open(amount: &str) -> serde_json::Value {
        json!({
            "state": "open", "handle": "demo", "dangerous": false, "held": true,
            "quote": quote(amount), "identityMode": "mock",
        })
    }

    #[test]
    fn an_open_challenge_reads_everything_the_page_shows() {
        let Ok(ChallengeView::Open(open)) = view_outcome(answer(200, open("1000000"))) else {
            panic!("expected an open challenge");
        };
        assert_eq!(open.handle, "demo");
        assert!(!open.dangerous && open.held);
        assert_eq!(open.amount, 1_000_000);
        assert_eq!(open.identity_mode, IdentityMode::Mock);
        assert_eq!(open.quote.reasons, ["looked automated"]);
        assert_eq!(open.quote.quote.expires_at, 1_800_000_000);
    }

    #[test]
    fn a_settled_challenge_and_a_dead_one_carry_nothing_more() {
        assert_eq!(
            view_outcome(answer(200, json!({"state": "resolved"}))),
            Ok(ChallengeView::Resolved)
        );
        assert_eq!(
            view_outcome(answer(200, json!({"state": "dead"}))),
            Ok(ChallengeView::Dead)
        );
    }

    #[test]
    fn only_a_404_means_the_token_is_unknown() {
        let unknown = answer(404, json!({"error": "Unknown challenge"}));
        assert_eq!(view_outcome(unknown), Err(ViewError::NotFound));
        for status in [400, 500, 502, 503] {
            assert_eq!(
                view_outcome(answer(status, json!({"error": "x"}))),
                Err(ViewError::Unavailable),
                "{status}"
            );
        }
    }

    #[test]
    fn a_missing_or_unreadable_answer_is_unavailable_not_unknown() {
        assert_eq!(view_outcome(offline()), Err(ViewError::Unavailable));
        let html = Ok(HttpResponse {
            status: 200,
            body: "<html>portal</html>".to_owned(),
        });
        assert_eq!(view_outcome(html), Err(ViewError::Unavailable));
        assert_eq!(
            view_outcome(answer(200, json!({"state": "mystery"}))),
            Err(ViewError::Unavailable)
        );
        assert_eq!(
            view_outcome(answer(200, json!({"state": "open", "handle": "demo"}))),
            Err(ViewError::Unavailable),
            "an open challenge without its quote cannot be offered"
        );
    }

    #[test]
    fn an_amount_too_large_to_pay_is_a_dead_link() {
        let huge = "9".repeat(40);
        assert_eq!(
            view_outcome(answer(200, open(&huge))),
            Ok(ChallengeView::Dead)
        );
    }

    #[test]
    fn an_unrecognised_identity_mode_is_unavailable() {
        let mut body = open("1");
        body["identityMode"] = json!("Mock ");
        assert_eq!(view_outcome(answer(200, body)), Err(ViewError::Unavailable));
    }

    #[test]
    fn resolve_answers_are_read_by_their_status_field() {
        let read = |status: u16, body: serde_json::Value| resolution_of(answer(status, body));
        assert_eq!(
            read(
                200,
                json!({"status": "cleared", "reason": "paid", "delivered": true})
            ),
            Resolution::Cleared { delivered: true }
        );
        assert_eq!(
            read(200, json!({"status": "cleared"})),
            Resolution::Cleared { delivered: false },
            "delivered counts only when it is exactly true"
        );
        assert_eq!(
            read(200, json!({"status": "charged", "reason": "dangerous"})),
            Resolution::Charged
        );
        assert_eq!(read(200, json!({"status": "pending"})), Resolution::NotYet);
        assert_eq!(
            read(404, json!({"error": "Unknown challenge"})),
            Resolution::NotYet
        );
    }

    #[test]
    fn an_unreachable_or_unreadable_resolve_is_one_more_not_yet() {
        assert_eq!(resolution_of(offline()), Resolution::NotYet);
        let html = Ok(HttpResponse {
            status: 502,
            body: "<html>bad gateway</html>".to_owned(),
        });
        assert_eq!(resolution_of(html), Resolution::NotYet);
    }

    #[test]
    fn a_delivered_paste_is_ok() {
        assert_eq!(
            delivery_outcome(answer(200, json!({"status": "delivered", "to": "a@b.c"}))),
            Ok(())
        );
    }

    #[test]
    fn a_refused_paste_carries_the_servers_words_or_a_fallback() {
        assert_eq!(
            delivery_outcome(answer(403, json!({"error": "That pass has run out"}))),
            Err(ApiError("That pass has run out".to_owned()))
        );
        assert_eq!(
            delivery_outcome(answer(500, json!({}))),
            Err(ApiError(DELIVER_FAILED.to_owned()))
        );
    }

    #[test]
    fn a_200_that_is_not_json_is_not_a_delivery() {
        let html = Ok(HttpResponse {
            status: 200,
            body: "<html>portal</html>".to_owned(),
        });
        assert_eq!(
            delivery_outcome(html),
            Err(ApiError(DELIVER_FAILED.to_owned()))
        );
    }

    #[test]
    fn an_unreachable_server_is_said_plainly() {
        assert_eq!(
            delivery_outcome(offline()),
            Err(ApiError(UNREACHABLE.to_owned()))
        );
    }
}

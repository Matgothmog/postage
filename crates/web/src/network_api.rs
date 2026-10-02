//! `GET /api/network`, typed: everything the ledger page shows, already worked
//! out by the server (`crates/server/src/routes/network_view.rs`). The Next
//! page asked the subgraph itself while rendering on the server; this one
//! reads the answer, so it holds no totals logic and no clock of its own.
//!
//! The same shape as `api`: a pure function turns an answer into a typed
//! outcome and runs natively under test; a thin `async` function sends the
//! request.

use serde::{Deserialize, Deserializer};

use crate::api::{ApiError, UNREACHABLE, error_text};
use crate::http::{self, HttpError, HttpRequest, HttpResponse};

pub const NETWORK_PATH: &str = "/api/network";

/// What the callout says when the server refused without saying why.
const UNREADABLE: &str = "The ledger could not be read.";

/// An amount in base units. The server sends these as decimal strings because
/// they are wider than a JSON number holds. One that does not read fails the
/// whole answer, as `BigInt(..)` throwing took the Next page down.
fn base_units<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
    let text = String::deserialize(deserializer)?;
    text.parse()
        .map_err(|_| serde::de::Error::custom(format!("{text:?} is not an amount in base units")))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkView {
    /// "Paid out".
    #[serde(deserialize_with = "base_units")]
    pub earned: u128,
    /// "Verified free".
    pub verified_count: usize,
    /// "Held, then paid".
    pub delivered: u64,
    /// How many more attestations the sponsor pool funds.
    #[serde(deserialize_with = "base_units")]
    pub sponsored: u128,
    pub has_active_signer: bool,
    pub vault: Option<Vault>,
    pub feed: Vec<FeedItem>,
    pub senders: Vec<Sender>,
    pub inboxes: Vec<Inbox>,
    pub enclaves: Vec<Enclave>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vault {
    #[serde(deserialize_with = "base_units")]
    pub total_funded: u128,
    #[serde(deserialize_with = "base_units")]
    pub to_sponsorship: u128,
    #[serde(deserialize_with = "base_units")]
    pub refilled_to_relayer: u128,
    pub funding_events: u64,
}

/// One line of the merged, newest-first timeline. `since` is the server's
/// "14m ago" on the server's clock.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FeedItem {
    Payment {
        since: String,
        tier: String,
        sender: String,
        inbox: String,
        tx: String,
        /// What left the sender's wallet.
        #[serde(deserialize_with = "base_units")]
        total: u128,
    },
    Verification {
        since: String,
        wallet: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sender {
    pub id: String,
    pub paid_count: u64,
    /// A decimal fraction such as `"0.25"`.
    pub spam_rate: String,
    pub still_human: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inbox {
    pub id: String,
    #[serde(deserialize_with = "base_units")]
    pub floor_price: u128,
    #[serde(deserialize_with = "base_units")]
    pub earned: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Enclave {
    pub id: String,
    pub revoked: bool,
    /// The first characters of the measurement, cut on the server.
    pub measurement_prefix: String,
}

/// What one answer from `GET /api/network` says. A refusal carries the
/// server's own words (`502 {"error": ..}` when the chain cannot be read),
/// which the page shows as the Next page showed the failure it caught.
pub fn view_outcome(answer: Result<HttpResponse, HttpError>) -> Result<NetworkView, ApiError> {
    let response = answer.map_err(|_| ApiError(UNREACHABLE.to_owned()))?;
    if !response.is_ok() {
        return Err(ApiError(
            error_text(&response.body).unwrap_or_else(|| UNREADABLE.to_owned()),
        ));
    }
    serde_json::from_str(&response.body).map_err(|_| ApiError(UNREADABLE.to_owned()))
}

/// `GET /api/network`.
pub async fn get_network() -> Result<NetworkView, ApiError> {
    view_outcome(http::send(&HttpRequest::get(NETWORK_PATH), None).await)
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

    const FULL: &str = r#"{
        "earned":"200","verifiedCount":1,"delivered":2,"sponsored":"3","hasActiveSigner":true,
        "vault":{"totalFunded":"10","toSponsorship":"5","refilledToRelayer":"2","fundingEvents":4},
        "feed":[
            {"kind":"payment","at":5,"since":"1h ago","id":"0x05","tier":"COMMERCIAL","sender":"0x04","inbox":"0x03","tx":"0x06","total":"100"},
            {"kind":"verification","at":4,"since":"just now","wallet":"0x02"}
        ],
        "senders":[{"id":"0x04","paidCount":1,"spamRate":"0.25","stillHuman":false}],
        "inboxes":[{"id":"0x03","floorPrice":"100","earned":"200"}],
        "enclaves":[{"id":"0x01","revoked":false,"measurementPrefix":"0xaaaa"}]
    }"#;

    #[test]
    fn a_full_answer_reads_every_section() {
        let view = view_outcome(answer(200, FULL)).unwrap();
        assert_eq!(view.earned, 200);
        assert_eq!(view.sponsored, 3);
        assert_eq!(view.vault.unwrap().to_sponsorship, 5);
        assert_eq!(view.feed.len(), 2);
        assert!(matches!(
            &view.feed[0],
            FeedItem::Payment { total: 100, tier, .. } if tier == "COMMERCIAL"
        ));
        assert!(matches!(&view.feed[1], FeedItem::Verification { wallet, .. } if wallet == "0x02"));
        assert_eq!(view.inboxes[0].floor_price, 100);
        assert_eq!(view.enclaves[0].measurement_prefix, "0xaaaa");
    }

    #[test]
    fn amounts_wider_than_a_json_number_keep_every_digit() {
        let body = FULL.replace(
            r#""earned":"200""#,
            r#""earned":"340282366920938463463374607431768211455""#,
        );
        assert_eq!(view_outcome(answer(200, &body)).unwrap().earned, u128::MAX);
    }

    #[test]
    fn a_vault_may_be_absent() {
        let body = FULL.replace(
            r#""vault":{"totalFunded":"10","toSponsorship":"5","refilledToRelayer":"2","fundingEvents":4},"#,
            r#""vault":null,"#,
        );
        assert_eq!(view_outcome(answer(200, &body)).unwrap().vault, None);
    }

    #[test]
    fn an_amount_that_is_not_digits_fails_the_whole_answer() {
        let body = FULL.replace(r#""earned":"200""#, r#""earned":"2e2""#);
        assert_eq!(
            view_outcome(answer(200, &body)),
            Err(ApiError(UNREADABLE.to_owned()))
        );
    }

    #[test]
    fn a_refusal_passes_the_servers_reason_on() {
        assert_eq!(
            view_outcome(answer(502, r#"{"error":"subgraph timed out"}"#)),
            Err(ApiError("subgraph timed out".to_owned()))
        );
    }

    #[test]
    fn a_refusal_with_no_reason_gets_a_plain_one() {
        assert_eq!(
            view_outcome(answer(502, "<html>bad gateway</html>")),
            Err(ApiError(UNREADABLE.to_owned()))
        );
    }

    #[test]
    fn no_answer_at_all_is_unreachable() {
        assert_eq!(
            view_outcome(Err(HttpError::Network("offline".to_owned()))),
            Err(ApiError(UNREACHABLE.to_owned()))
        );
    }

    #[test]
    fn an_answer_that_is_not_the_ledger_is_unreadable() {
        assert_eq!(
            view_outcome(answer(200, r#"{"unexpected":true}"#)),
            Err(ApiError(UNREADABLE.to_owned()))
        );
    }
}

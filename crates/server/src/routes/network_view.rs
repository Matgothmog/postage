//! `GET /api/network`: what the ledger page (`web/src/app/network/page.tsx`)
//! read from the subgraph and worked out on the server, for the
//! client-rendered page to fetch instead.
//!
//! Only the fields that page rendered, and the values it computed from the
//! server's clock (`since`, `stillHuman`) or across rows (the totals and the
//! merged feed). Amounts stay in base units as decimal strings: they are wider
//! than a JSON number holds, and formatting them is the page's job. The
//! contract addresses the page listed are constants the client already has.

use axum::extract::State;
use axum::http::StatusCode;
use postage_core::network::{Overview, derive_aggregates, payment_total, since, still_human};
use serde::Serialize;

use super::{RouteResult, json, refuse};
use crate::app::AppState;
use crate::faults::redact;
use crate::network::fetch_overview;

/// How many entries the merged feed keeps.
const FEED_LENGTH: usize = 25;

/// How much of an enclave's measurement the page showed, in characters.
const MEASUREMENT_PREFIX: usize = 26;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NetworkView {
    /// "Paid out": what every inbox listed has earned.
    earned: String,
    /// "Verified free": how many attestations the subgraph returned.
    verified_count: usize,
    /// "Held, then paid": messages the listed inboxes received.
    delivered: u64,
    /// How many more attestations the sponsor pool funds.
    sponsored: String,
    /// Whether any registered signer is unrevoked.
    has_active_signer: bool,
    vault: Option<VaultView>,
    feed: Vec<FeedItem>,
    senders: Vec<SenderView>,
    inboxes: Vec<InboxView>,
    enclaves: Vec<EnclaveView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VaultView {
    total_funded: String,
    to_sponsorship: String,
    refilled_to_relayer: String,
    funding_events: u64,
}

/// A payment and a verification are both onchain proof, so the feed reads as
/// one timeline, merged and re-sorted by when each happened.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum FeedItem {
    Payment {
        at: i64,
        since: String,
        id: String,
        tier: String,
        sender: String,
        inbox: String,
        tx: String,
        /// What left the sender's wallet: the inbox's share plus the vault's.
        total: String,
    },
    Verification {
        at: i64,
        since: String,
        wallet: String,
    },
}

impl FeedItem {
    fn at(&self) -> i64 {
        match self {
            Self::Payment { at, .. } | Self::Verification { at, .. } => *at,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SenderView {
    id: String,
    paid_count: u64,
    spam_rate: String,
    still_human: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InboxView {
    id: String,
    floor_price: String,
    earned: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnclaveView {
    id: String,
    revoked: bool,
    measurement_prefix: String,
}

pub(crate) async fn get(State(state): State<AppState>) -> RouteResult {
    let overview = match fetch_overview(state.graph()).await {
        Ok(overview) => overview,
        // The page put the failure's own words in its "Can't reach the chain"
        // callout. They are passed on the same way, less any secret a URL in
        // them carried, which a page rendered on the server never risked.
        Err(error) => {
            let message = redact(state.env(), &error.to_string());
            return Err(refuse(StatusCode::BAD_GATEWAY, &message));
        }
    };
    Ok(json(StatusCode::OK, &view_of(&overview, state.now())))
}

fn view_of(data: &Overview, now: i64) -> NetworkView {
    let aggregates = derive_aggregates(data);
    NetworkView {
        earned: aggregates.earned.to_string(),
        verified_count: data.human_attestations.len(),
        delivered: aggregates.delivered,
        sponsored: aggregates.sponsored.to_string(),
        has_active_signer: aggregates.signer.is_some(),
        vault: data.vaults.first().map(|vault| VaultView {
            total_funded: vault.total_funded.clone(),
            to_sponsorship: vault.to_sponsorship.clone(),
            refilled_to_relayer: vault.refilled_to_relayer.clone(),
            funding_events: vault.funding_events,
        }),
        feed: feed_of(data, now),
        senders: data
            .senders
            .iter()
            .map(|sender| SenderView {
                id: sender.id.clone(),
                paid_count: sender.paid_count,
                spam_rate: sender.spam_rate.clone(),
                still_human: still_human(sender.human_until.as_deref(), now),
            })
            .collect(),
        inboxes: data
            .inboxes
            .iter()
            .map(|inbox| InboxView {
                id: inbox.id.clone(),
                floor_price: inbox.floor_price.clone(),
                earned: inbox.earned.clone(),
            })
            .collect(),
        enclaves: data
            .enclaves
            .iter()
            .map(|enclave| EnclaveView {
                id: enclave.id.clone(),
                revoked: enclave.revoked,
                measurement_prefix: enclave
                    .measurement
                    .chars()
                    .take(MEASUREMENT_PREFIX)
                    .collect(),
            })
            .collect(),
    }
}

/// Payments, then verifications, sorted newest first (a stable sort, as
/// JavaScript's is), and cut to the page's length.
fn feed_of(data: &Overview, now: i64) -> Vec<FeedItem> {
    let payments = data.payments.iter().map(|payment| {
        let at = seconds(&payment.paid_at);
        FeedItem::Payment {
            at,
            since: since(at, now),
            id: payment.id.clone(),
            tier: payment.tier.clone(),
            sender: payment.sender.id.clone(),
            inbox: payment.inbox.id.clone(),
            tx: payment.tx.clone(),
            total: payment_total(&payment.amount, &payment.to_vault).to_string(),
        }
    });
    let verifications = data.human_attestations.iter().map(|attestation| {
        let at = seconds(&attestation.attested_at);
        FeedItem::Verification {
            at,
            since: since(at, now),
            wallet: attestation.id.clone(),
        }
    });
    let mut feed: Vec<FeedItem> = payments.chain(verifications).collect();
    feed.sort_by_key(|item| std::cmp::Reverse(item.at()));
    feed.truncate(FEED_LENGTH);
    feed
}

/// A subgraph timestamp, which is only ever digits; anything else sorts as
/// the epoch rather than taking the page down.
fn seconds(column: &str) -> i64 {
    column.parse().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::config::Env;
    use crate::graph::Graph;
    use crate::http_stub::serve;
    use crate::routes::router;
    use crate::routes::testing::{Answer, NOW, clock_at, get as get_request, send};

    const ANSWER: &str = r#"{"data":{
        "vaults":[{"totalFunded":"10","toTreasury":"3","toSponsorship":"5","refilledToRelayer":"2","fundingEvents":4}],
        "enclaves":[{"id":"0x01","measurement":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","revoked":false,"registeredAt":"1700000000"}],
        "humanAttestations":[{"id":"0x02","attestedAt":"1788868700"}],
        "inboxes":[{"id":"0x03","floorPrice":"100","receivedCount":2,"earned":"200","claimed":"50"}],
        "senders":[{"id":"0x04","paidCount":1,"totalPaid":"100","spamReports":3,"spamRate":"0.25","humanUntil":"1788868900"}],
        "payments":[{"id":"0x05","tier":"2","amount":"90","toVault":"10","reportedAsSpam":false,"paidAt":"1788865200","tx":"0x06","sender":{"id":"0x04"},"inbox":{"id":"0x03"}}]
    }}"#;

    async fn ledger(status: u16, body: &str, env: Env) -> Answer {
        let stub = serve(&[("/postage", status, body)]).await;
        let app = router(
            AppState::builder(env)
                .clock(clock_at(NOW))
                .graph(Graph::new(
                    reqwest::Client::default(),
                    Some(format!("{}/postage", stub.base)),
                    None,
                ))
                .build(),
        );
        send(app, get_request("/api/network")).await
    }

    #[tokio::test]
    async fn the_ledger_carries_what_the_page_rendered() {
        let answer = ledger(200, ANSWER, Env::empty()).await;

        assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
        assert_eq!(
            answer.body,
            json!({
                "earned": "200",
                "verifiedCount": 1,
                "delivered": 2,
                "sponsored": "0",
                "hasActiveSigner": true,
                "vault": {
                    "totalFunded": "10",
                    "toSponsorship": "5",
                    "refilledToRelayer": "2",
                    "fundingEvents": 4,
                },
                "feed": [
                    { "kind": "verification", "at": 1_788_868_700, "since": "1m ago", "wallet": "0x02" },
                    {
                        "kind": "payment", "at": 1_788_865_200, "since": "1h ago", "id": "0x05",
                        "tier": "2", "sender": "0x04", "inbox": "0x03", "tx": "0x06", "total": "100",
                    },
                ],
                "senders": [{ "id": "0x04", "paidCount": 1, "spamRate": "0.25", "stillHuman": true }],
                "inboxes": [{ "id": "0x03", "floorPrice": "100", "earned": "200" }],
                "enclaves": [{ "id": "0x01", "revoked": false, "measurementPrefix": "0xaaaaaaaaaaaaaaaaaaaaaaaa" }],
            })
        );
    }

    /// Fields the subgraph returned that the page never showed stay out.
    #[tokio::test]
    async fn fields_the_page_never_rendered_are_not_passed_on() {
        let answer = ledger(200, ANSWER, Env::empty()).await;

        for field in [
            "toTreasury",
            "claimed",
            "totalPaid",
            "spamReports",
            "registeredAt",
            "reportedAsSpam",
        ] {
            assert!(
                !answer.text.contains(field),
                "{field} leaked: {}",
                answer.text
            );
        }
    }

    #[tokio::test]
    async fn an_empty_network_has_no_vault_and_an_empty_feed() {
        let empty = r#"{"data":{"vaults":[],"enclaves":[],"humanAttestations":[],"inboxes":[],"senders":[],"payments":[]}}"#;

        let answer = ledger(200, empty, Env::empty()).await;

        assert_eq!(answer.body["vault"], Value::Null);
        assert_eq!(answer.body["feed"], json!([]));
        assert_eq!(answer.body["earned"], "0");
        assert_eq!(answer.body["hasActiveSigner"], false);
    }

    #[tokio::test]
    async fn the_feed_keeps_the_newest_twenty_five() {
        let attestations: Vec<Value> = (0..30)
            .map(|n| json!({ "id": format!("0x{n:02}"), "attestedAt": (NOW - n).to_string() }))
            .collect();
        let body = json!({ "data": {
            "vaults": [], "enclaves": [], "humanAttestations": attestations,
            "inboxes": [], "senders": [], "payments": [],
        }})
        .to_string();

        let answer = ledger(200, &body, Env::empty()).await;

        let feed = answer.body["feed"].as_array().unwrap();
        assert_eq!(feed.len(), 25);
        assert_eq!(feed[0]["wallet"], "0x00");
        assert_eq!(feed[24]["wallet"], "0x24");
    }

    /// The page showed the failure in its callout; the subgraph URL carries
    /// its API key in the path, so it is taken out first.
    #[tokio::test]
    async fn a_subgraph_that_fails_is_a_bad_gateway_with_the_reason_and_no_secret() {
        let failing = r#"{"errors":[{"message":"bad indexers at https://gateway.example/api/0123456789abcdef/x"}]}"#;
        let env = Env::fixed([("GRAPH_API_KEY", "0123456789abcdef")]);

        let answer = ledger(200, failing, env).await;

        assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            answer.body,
            json!({ "error": "bad indexers at https://gateway.example/api/[redacted GRAPH_API_KEY]/x" })
        );
    }

    #[tokio::test]
    async fn an_unconfigured_subgraph_says_which_setting_is_missing() {
        let app = router(AppState::builder(Env::empty()).clock(clock_at(NOW)).build());

        let answer = send(app, get_request("/api/network")).await;

        assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            answer.body,
            json!({ "error": "GRAPH_QUERY_URL is not set" })
        );
    }
}

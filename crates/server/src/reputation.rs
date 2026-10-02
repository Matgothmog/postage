//! A sender's standing, gathered from the subgraphs (`web/src/lib/reputation.ts`).

use postage_core::pricing::SenderSignals;
use postage_core::reputation::{
    ENS_OWNED, ENS_SUBGRAPH, EnsNames, POSTAGE_HISTORY, SenderHistory, signals_from,
};
use serde_json::json;

use crate::graph::Graph;

/// Both lookups are independent, and a failure in either should soften the
/// price rather than block the message, so they settle rather than fail.
pub async fn gather_signals(graph: &Graph, wallet: &str) -> SenderSignals {
    let id = wallet.to_lowercase();
    let variables = json!({ "wallet": id });

    let (history, ens) = tokio::join!(
        graph.query_postage::<SenderHistory>(POSTAGE_HISTORY, variables.clone()),
        graph.query_network::<EnsNames>(ENS_SUBGRAPH, ENS_OWNED, variables),
    );

    let sender = history.ok().and_then(|history| history.sender);
    let domains = ens.map(|ens| ens.domains).unwrap_or_default();
    signals_from(sender.as_ref(), &domains)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::http_stub::{Reply, serve, serve_with};

    const ENS_PATH: &str = "/api/key/subgraphs/id/5XqPmWe6gjyrJtFn9cLy237i4cWw2j9HcUJEXsP5qGtH";

    fn graph(base: &str) -> Graph {
        Graph::new(
            reqwest::Client::default(),
            Some(format!("{base}/postage")),
            Some("key".to_owned()),
        )
        .with_gateway_base(base)
    }

    #[tokio::test]
    async fn both_lookups_are_asked_about_the_lowercased_wallet_and_folded_together() {
        let stub = serve(&[
            (
                "/postage",
                200,
                r#"{"data":{"sender":{"paidCount":4,"spamReports":1,"spamRate":"0.25"}}}"#,
            ),
            (
                ENS_PATH,
                200,
                r#"{"data":{"domains":[{"createdAt":"1700000000"},{"createdAt":"1500000000"}]}}"#,
            ),
        ])
        .await;

        let signals = gather_signals(
            &graph(&stub.base),
            "0xAbCdEf0123456789aBcDeF0123456789AbCdEf01",
        )
        .await;

        assert_eq!(
            signals,
            SenderSignals {
                paid_count: 4,
                spam_reports: 1,
                spam_rate: 0.25,
                ens_names: 2,
                oldest_ens_at: Some(1_500_000_000),
            }
        );
        let sent = stub.sent();
        assert_eq!(sent.len(), 2);
        for request in &sent {
            assert_eq!(
                request.body["variables"],
                json!({ "wallet": "0xabcdef0123456789abcdef0123456789abcdef01" })
            );
        }
        let postage = sent
            .iter()
            .find(|request| request.path == "/postage")
            .unwrap();
        assert_eq!(postage.body["query"], POSTAGE_HISTORY);
        let ens = sent
            .iter()
            .find(|request| request.path == ENS_PATH)
            .unwrap();
        assert_eq!(ens.body["query"], ENS_OWNED);
    }

    #[tokio::test]
    async fn a_failed_history_lookup_still_counts_ens_names() {
        let stub = serve(&[
            ("/postage", 500, ""),
            (ENS_PATH, 200, r#"{"data":{"domains":[{"createdAt":"1"}]}}"#),
        ])
        .await;

        let signals = gather_signals(&graph(&stub.base), "0xab").await;

        assert_eq!(signals.paid_count, 0);
        assert_eq!(signals.spam_rate, 0.0);
        assert_eq!(signals.ens_names, 1);
        assert_eq!(signals.oldest_ens_at, Some(1));
    }

    #[tokio::test]
    async fn a_failed_ens_lookup_still_carries_the_history() {
        let stub = serve(&[
            (
                "/postage",
                200,
                r#"{"data":{"sender":{"paidCount":3,"spamReports":0,"spamRate":"0"}}}"#,
            ),
            (ENS_PATH, 200, r#"{"errors":[{"message":"indexer down"}]}"#),
        ])
        .await;

        let signals = gather_signals(&graph(&stub.base), "0xab").await;

        assert_eq!(signals.paid_count, 3);
        assert_eq!(signals.ens_names, 0);
        assert_eq!(signals.oldest_ens_at, None);
    }

    #[tokio::test]
    async fn an_unknown_sender_and_unconfigured_gateway_is_a_blank_sender() {
        let stub = serve(&[("/postage", 200, r#"{"data":{"sender":null}}"#)]).await;
        let unconfigured = Graph::new(
            reqwest::Client::default(),
            Some(format!("{}/postage", stub.base)),
            None,
        );

        let signals = gather_signals(&unconfigured, "0xab").await;

        assert_eq!(signals, signals_from(None, &[]));
    }

    #[tokio::test]
    async fn lookups_that_time_out_leave_a_blank_sender() {
        let stub = serve_with(|_| Reply::new(200, "{}").after(Duration::from_secs(30))).await;
        let slow = graph(&stub.base).with_timeout(Duration::from_millis(200));

        let signals = gather_signals(&slow, "0xab").await;

        assert_eq!(signals, signals_from(None, &[]));
    }
}

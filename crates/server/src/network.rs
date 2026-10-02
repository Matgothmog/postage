//! The network page's data (`fetchOverview` in `web/src/lib/network.ts`).

use postage_core::network::{OVERVIEW, Overview};
use serde_json::json;

use crate::graph::{Graph, GraphError};

/// Everything the network page renders, in one query to our own subgraph.
pub async fn fetch_overview(graph: &Graph) -> Result<Overview, GraphError> {
    graph.query_postage(OVERVIEW, json!({})).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_stub::serve;

    const ANSWER: &str = r#"{"data":{
        "vaults":[{"totalFunded":"10","toTreasury":"3","toSponsorship":"5","refilledToRelayer":"2","fundingEvents":4}],
        "enclaves":[{"id":"0x01","measurement":"0xaa","revoked":false,"registeredAt":"1700000000"}],
        "humanAttestations":[{"id":"0x02","attestedAt":"1700000001"}],
        "inboxes":[{"id":"0x03","floorPrice":"100","receivedCount":2,"earned":"200","claimed":"50"}],
        "senders":[{"id":"0x04","paidCount":1,"totalPaid":"100","spamReports":0,"spamRate":"0","humanUntil":null}],
        "payments":[{"id":"0x05","tier":"2","amount":"90","toVault":"10","reportedAsSpam":false,"paidAt":"1700000002","tx":"0x06","sender":{"id":"0x04"},"inbox":{"id":"0x03"}}]
    }}"#;

    fn graph(base: &str) -> Graph {
        Graph::new(
            reqwest::Client::default(),
            Some(format!("{base}/postage")),
            None,
        )
    }

    #[tokio::test]
    async fn the_overview_query_is_sent_verbatim_with_empty_variables_and_decoded() {
        let stub = serve(&[("/postage", 200, ANSWER)]).await;

        let overview = fetch_overview(&graph(&stub.base)).await.unwrap();

        assert_eq!(overview.vaults[0].to_sponsorship, "5");
        assert_eq!(overview.inboxes[0].received_count, 2);
        assert_eq!(overview.senders[0].human_until, None);
        assert_eq!(overview.payments[0].sender.id, "0x04");
        let sent = stub.sent();
        assert_eq!(sent[0].body["query"], OVERVIEW);
        assert_eq!(sent[0].body["variables"], json!({}));
    }

    #[tokio::test]
    async fn a_failing_subgraph_is_an_error_for_the_page_to_handle() {
        let stub = serve(&[("/postage", 200, r#"{"errors":[{"message":"bad query"}]}"#)]).await;

        let error = fetch_overview(&graph(&stub.base)).await.unwrap_err();

        assert_eq!(error.to_string(), "bad query");
    }
}

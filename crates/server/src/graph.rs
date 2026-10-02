//! GraphQL queries against The Graph (`web/src/lib/graph.ts`): our own
//! subgraph at `GRAPH_QUERY_URL`, and public subgraphs on the decentralized
//! network through the gateway, keyed with `GRAPH_API_KEY`.
//!
//! Each URL is required only when a query needs it, as in the TypeScript: a
//! deployment without a gateway key still reads its own subgraph, and the
//! reputation lookup that wants both softens instead of failing.

use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::config::{ConfigError, required};

pub const DEFAULT_GATEWAY_BASE: &str = "https://gateway.thegraph.com";

/// How long one subgraph query may take. The TypeScript set none; an indexer
/// that never answers would otherwise hold the sender's request open until the
/// host kills the function. A timeout is a `GraphError::Transport`, which the
/// reputation lookup already softens into a blank sender.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("{0}")]
    Transport(String),
    #[error("Graph query failed: {0}")]
    Status(u16),
    #[error("Graph response was not readable: {0}")]
    Decode(String),
    /// The first error the endpoint reported, as it worded it.
    #[error("{0}")]
    Query(String),
    #[error("Graph returned no data")]
    NoData,
}

#[derive(Deserialize)]
struct GraphResponse {
    data: Option<Value>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Deserialize)]
struct GraphQlError {
    message: Option<String>,
}

#[derive(Clone)]
pub struct Graph {
    client: reqwest::Client,
    postage_url: Option<String>,
    api_key: Option<String>,
    gateway_base: String,
    timeout: Duration,
}

impl std::fmt::Debug for Graph {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Graph")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Graph {
    pub fn new(
        client: reqwest::Client,
        postage_url: Option<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            client,
            postage_url,
            api_key,
            gateway_base: DEFAULT_GATEWAY_BASE.to_owned(),
            timeout: QUERY_TIMEOUT,
        }
    }

    /// Reads `GRAPH_QUERY_URL` and `GRAPH_API_KEY` now; their absence is
    /// reported by the query that needs them.
    pub fn from_env<F>(env: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let optional = |name: &'static str| required(&env, name).ok();
        Self::new(
            reqwest::Client::default(),
            optional("GRAPH_QUERY_URL"),
            optional("GRAPH_API_KEY"),
        )
    }

    /// Points gateway queries somewhere other than The Graph's gateway.
    pub fn with_gateway_base(mut self, base: impl Into<String>) -> Self {
        self.gateway_base = base.into();
        self
    }

    /// Shortens the per-query timeout so tests need not wait out ten seconds.
    #[cfg(test)]
    pub(crate) fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Our own subgraph: what a sender has done inside Postage.
    pub async fn query_postage<T: DeserializeOwned>(
        &self,
        document: &str,
        variables: Value,
    ) -> Result<T, GraphError> {
        let url = self
            .postage_url
            .as_deref()
            .ok_or(ConfigError::Missing("GRAPH_QUERY_URL"))?;
        self.query(url, document, variables).await
    }

    /// A public subgraph on the network, reached through the gateway.
    pub async fn query_network<T: DeserializeOwned>(
        &self,
        subgraph_id: &str,
        document: &str,
        variables: Value,
    ) -> Result<T, GraphError> {
        let key = self
            .api_key
            .as_deref()
            .ok_or(ConfigError::Missing("GRAPH_API_KEY"))?;
        let url = format!("{}/api/{key}/subgraphs/id/{subgraph_id}", self.gateway_base);
        self.query(&url, document, variables).await
    }

    /// POSTs `{ query, variables }` and returns `data`. A non-2xx status, any
    /// reported GraphQL error, a missing `data`, or no answer within the
    /// timeout is a failure.
    pub async fn query<T: DeserializeOwned>(
        &self,
        url: &str,
        document: &str,
        variables: Value,
    ) -> Result<T, GraphError> {
        let body = json!({ "query": document, "variables": variables });
        let response = self
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|error| GraphError::Transport(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(GraphError::Status(status.as_u16()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| GraphError::Transport(error.to_string()))?;

        let payload: GraphResponse = serde_json::from_slice(&bytes)
            .map_err(|error| GraphError::Decode(error.to_string()))?;
        if let Some(first) = payload.errors.and_then(|errors| errors.into_iter().next()) {
            return Err(GraphError::Query(first.message.unwrap_or_default()));
        }
        let data = payload
            .data
            .filter(|data| !data.is_null())
            .ok_or(GraphError::NoData)?;
        serde_json::from_value(data).map_err(|error| GraphError::Decode(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn debug_output_leaves_out_the_api_key_and_urls() {
        let graph = Graph::new(
            reqwest::Client::default(),
            Some("https://postage.test/secret-path".to_owned()),
            Some("graph-secret-key".to_owned()),
        );
        let shown = format!("{graph:?}");
        assert!(!shown.contains("graph-secret-key"));
        assert!(!shown.contains("postage.test"));
    }

    use super::*;
    use crate::http_stub::{Reply, closed_port, serve, serve_with};

    #[derive(Debug, PartialEq, Eq, Deserialize)]
    struct Answer {
        value: u64,
    }

    fn postage_graph(base: &str) -> Graph {
        Graph::new(
            reqwest::Client::default(),
            Some(format!("{base}/postage")),
            Some("test-key".to_owned()),
        )
        .with_gateway_base(base)
    }

    #[tokio::test]
    async fn a_query_posts_its_document_and_variables_as_json_and_returns_data() {
        let stub = serve(&[("/postage", 200, r#"{"data":{"value":7}}"#)]).await;

        let answer: Answer = postage_graph(&stub.base)
            .query_postage("query Q { value }", json!({ "wallet": "0xab" }))
            .await
            .unwrap();

        assert_eq!(answer, Answer { value: 7 });
        let sent = stub.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].content_type.as_deref(), Some("application/json"));
        assert_eq!(
            sent[0].body,
            json!({ "query": "query Q { value }", "variables": { "wallet": "0xab" } })
        );
    }

    #[tokio::test]
    async fn a_network_query_goes_through_the_gateway_with_the_key_and_subgraph_id() {
        let stub = serve(&[(
            "/api/test-key/subgraphs/id/SUBGRAPH",
            200,
            r#"{"data":{"value":1}}"#,
        )])
        .await;

        let answer: Answer = postage_graph(&stub.base)
            .query_network("SUBGRAPH", "query Q { value }", json!({}))
            .await
            .unwrap();

        assert_eq!(answer, Answer { value: 1 });
    }

    #[test]
    fn the_default_gateway_is_the_graphs_own() {
        let graph = Graph::new(reqwest::Client::default(), None, None);
        assert_eq!(graph.gateway_base, "https://gateway.thegraph.com");
    }

    #[tokio::test]
    async fn a_non_success_status_fails_with_the_status() {
        let stub = serve(&[("/postage", 502, "bad gateway")]).await;

        let error = postage_graph(&stub.base)
            .query_postage::<Answer>("q", json!({}))
            .await
            .unwrap_err();

        assert!(matches!(error, GraphError::Status(502)));
        assert_eq!(error.to_string(), "Graph query failed: 502");
    }

    #[tokio::test]
    async fn the_first_reported_error_is_the_failure_even_beside_data() {
        let stub = serve(&[(
            "/postage",
            200,
            r#"{"data":{"value":1},"errors":[{"message":"first"},{"message":"second"}]}"#,
        )])
        .await;

        let error = postage_graph(&stub.base)
            .query_postage::<Answer>("q", json!({}))
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "first");
    }

    #[tokio::test]
    async fn an_empty_error_list_is_not_a_failure() {
        let stub = serve(&[("/postage", 200, r#"{"data":{"value":2},"errors":[]}"#)]).await;

        let answer: Answer = postage_graph(&stub.base)
            .query_postage("q", json!({}))
            .await
            .unwrap();

        assert_eq!(answer.value, 2);
    }

    #[tokio::test]
    async fn missing_or_null_data_is_no_data() {
        for body in [r#"{}"#, r#"{"data":null}"#] {
            let stub = serve(&[("/postage", 200, body)]).await;

            let error = postage_graph(&stub.base)
                .query_postage::<Answer>("q", json!({}))
                .await
                .unwrap_err();

            assert!(matches!(error, GraphError::NoData), "{body}");
            assert_eq!(error.to_string(), "Graph returned no data");
        }
    }

    #[tokio::test]
    async fn a_body_that_is_not_json_is_a_decode_failure() {
        let stub = serve(&[("/postage", 200, "<html>")]).await;

        let error = postage_graph(&stub.base)
            .query_postage::<Answer>("q", json!({}))
            .await
            .unwrap_err();

        assert!(matches!(error, GraphError::Decode(_)), "{error}");
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_is_a_transport_failure() {
        let closed = closed_port();

        let error = postage_graph(&closed)
            .query_postage::<Answer>("q", json!({}))
            .await
            .unwrap_err();

        assert!(matches!(error, GraphError::Transport(_)), "{error}");
    }

    #[tokio::test]
    async fn an_unset_url_is_reported_by_the_query_that_needs_it() {
        let graph = Graph::from_env(|_| None);

        let postage = graph
            .query_postage::<Answer>("q", json!({}))
            .await
            .unwrap_err();
        let network = graph
            .query_network::<Answer>("id", "q", json!({}))
            .await
            .unwrap_err();

        assert!(matches!(
            postage,
            GraphError::Config(ConfigError::Missing("GRAPH_QUERY_URL"))
        ));
        assert!(matches!(
            network,
            GraphError::Config(ConfigError::Missing("GRAPH_API_KEY"))
        ));
    }

    #[test]
    fn from_env_reads_both_names_and_treats_empty_as_unset() {
        let graph = Graph::from_env(|name| match name {
            "GRAPH_QUERY_URL" => Some("https://example.test/q".to_owned()),
            "GRAPH_API_KEY" => Some(String::new()),
            _ => None,
        });
        assert_eq!(graph.postage_url.as_deref(), Some("https://example.test/q"));
        assert_eq!(graph.api_key, None);
    }

    #[test]
    fn a_query_may_take_ten_seconds_by_default() {
        assert_eq!(QUERY_TIMEOUT, Duration::from_secs(10));
        let graph = Graph::from_env(|_| None);
        assert_eq!(graph.timeout, QUERY_TIMEOUT);
    }

    #[tokio::test]
    async fn an_endpoint_that_never_answers_is_a_timeout_within_the_limit() {
        let stub = serve_with(|_| Reply::new(200, "{}").after(Duration::from_secs(30))).await;
        let graph = postage_graph(&stub.base).with_timeout(Duration::from_millis(200));
        let started = std::time::Instant::now();

        let error = graph
            .query_postage::<Answer>("q", json!({}))
            .await
            .unwrap_err();

        assert!(matches!(error, GraphError::Transport(_)), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}

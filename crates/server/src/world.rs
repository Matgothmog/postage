//! World's Developer Portal verify endpoint
//! (`verifyWithWorld` in `web/src/app/api/world/verify/route.ts`).
//!
//! Only the transport lives here: post the IDKit result, hand back the status
//! and the raw body. What the answer means for a sender is decided by
//! `world_verify`, which has to tell World's verdict on a proof apart from
//! World being unreachable or making no sense.

use std::fmt;
use std::time::Duration;

use axum::body::Bytes;
use serde_json::Value;

use crate::faults::reason_chain;

/// World ID 4.0's verify API (`route.ts:65`).
pub const DEFAULT_VERIFY_BASE: &str = "https://developer.world.org/api/v4";

/// How long one verify call may take, body included. The TypeScript set none,
/// so a hung connection held the sender's request open until the host killed
/// the function; a timeout reads as World being unreachable.
pub const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);

/// What came back from World, before anyone has judged it.
#[derive(Debug, Clone)]
pub struct WorldReply {
    pub status: u16,
    /// The body as sent, or why it could not be read off the wire.
    pub body: Result<Bytes, String>,
}

/// World could not be reached, or did not answer within the timeout.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Unreachable(pub String);

#[derive(Clone)]
pub struct WorldVerify {
    client: reqwest::Client,
    base: String,
    timeout: Duration,
}

impl fmt::Debug for WorldVerify {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorldVerify")
            .field("base", &self.base)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Default for WorldVerify {
    fn default() -> Self {
        Self::new(reqwest::Client::default())
    }
}

impl WorldVerify {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            base: DEFAULT_VERIFY_BASE.to_owned(),
            timeout: VERIFY_TIMEOUT,
        }
    }

    /// Points the client somewhere other than World's API.
    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }

    /// Shortens the timeout so tests need not wait out ten seconds.
    #[cfg(test)]
    pub(crate) fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// POSTs `proof` as JSON to `{base}/verify/{rp_id}`, the relying party's
    /// id spliced into the path as the TypeScript did, unencoded. A failure
    /// carries reqwest's message with its causes: the top line alone is often
    /// just "error sending request".
    pub async fn verify(&self, rp_id: &str, proof: &Value) -> Result<WorldReply, Unreachable> {
        let response = self
            .client
            .post(format!("{}/verify/{rp_id}", self.base))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(proof.to_string())
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|error| Unreachable(reason_chain(&error)))?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(|error| reason_chain(&error));
        Ok(WorldReply { status, body })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::http_stub::{Reply, closed_port, serve, serve_with};

    #[test]
    fn talks_to_world_v4_with_a_ten_second_bound_by_default() {
        let world = WorldVerify::default();
        assert_eq!(world.base, "https://developer.world.org/api/v4");
        assert_eq!(world.timeout, Duration::from_secs(10));
    }

    #[tokio::test]
    async fn posts_the_proof_as_json_to_the_relying_partys_verify_path() {
        let stub = serve(&[("/verify/app_rp", 200, r#"{"success":true}"#)]).await;
        let proof = json!({ "nonce": "n", "responses": [{ "identifier": "selfie" }] });

        let reply = WorldVerify::default()
            .with_base(&stub.base)
            .verify("app_rp", &proof)
            .await
            .unwrap();

        assert_eq!(reply.status, 200);
        assert_eq!(reply.body.unwrap(), Bytes::from(r#"{"success":true}"#));
        let sent = stub.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].method, "POST");
        assert_eq!(sent[0].content_type.as_deref(), Some("application/json"));
        assert_eq!(sent[0].body, proof);
    }

    #[tokio::test]
    async fn hands_back_a_refusal_as_it_came_rather_than_as_an_error() {
        let stub = serve(&[("/verify/app_rp", 503, "down")]).await;

        let reply = WorldVerify::default()
            .with_base(&stub.base)
            .verify("app_rp", &json!({}))
            .await
            .unwrap();

        assert_eq!(reply.status, 503);
        assert_eq!(reply.body.unwrap(), Bytes::from("down"));
    }

    #[tokio::test]
    async fn a_refused_connection_is_unreachable() {
        let result = WorldVerify::default()
            .with_base(closed_port())
            .verify("app_rp", &json!({}))
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn an_answer_slower_than_the_timeout_is_unreachable() {
        let stub = serve_with(|_| Reply::new(200, "{}").after(Duration::from_millis(500))).await;

        let result = WorldVerify::default()
            .with_base(&stub.base)
            .with_timeout(Duration::from_millis(50))
            .verify("app_rp", &json!({}))
            .await;

        assert!(result.is_err());
    }
}

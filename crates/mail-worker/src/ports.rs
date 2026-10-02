//! The two boundaries the handlers stand on.
//!
//! `Edge` is everything the worker reaches for outside the message itself: the
//! KV namespace of held mail, outbound HTTP, the clock and the log.
//! `InboundMessage` is the message Cloudflare hands the email handler. Both
//! are traits so that the behaviour in `inbound` and `release` runs natively
//! against in-memory fakes; `cloudflare` implements them over workers-rs and
//! exists only on wasm32.

use postage_shared::GatewayNotice;

/// What went wrong at a boundary, as text. Callers log it and choose their own
/// wording for whoever is told, so nothing in here is ever returned to a sender
/// or to the gateway.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct EdgeError(pub String);

/// A response that arrived, whatever its status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpAnswer {
    pub status: u16,
    pub body: String,
}

impl HttpAnswer {
    /// `Response.ok` in the fetch API: any 2xx.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The inbound-API call: JSON, authenticated by the shared secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonPost {
    pub url: String,
    pub secret: String,
    pub body: String,
}

/// The Mailgun call: a multipart form carrying the held bytes as one file part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MimeUpload {
    pub url: String,
    pub authorization: String,
    /// Plain fields, in the order they are appended.
    pub fields: Vec<(&'static str, String)>,
    /// The `message` file part, sent as `message/rfc822` named `held.eml`.
    pub message: Vec<u8>,
}

#[allow(
    async_fn_in_trait,
    reason = "implemented only inside this crate, on one thread; no caller needs a Send bound"
)]
pub trait Edge {
    /// Stores `value` under `key`, expiring at `expiration` (epoch seconds).
    async fn put_held(&self, key: &str, value: &[u8], expiration: u64) -> Result<(), EdgeError>;
    async fn get_held(&self, key: &str) -> Result<Option<Vec<u8>>, EdgeError>;
    async fn delete_held(&self, key: &str) -> Result<(), EdgeError>;
    async fn post_json(&self, post: &JsonPost) -> Result<HttpAnswer, EdgeError>;
    async fn post_mime(&self, upload: &MimeUpload) -> Result<HttpAnswer, EdgeError>;
    fn now_seconds(&self) -> u64;
    /// One log line: a label and named details. Details never carry a token.
    fn log_error(&self, label: &str, details: &[(&str, String)]);
}

#[allow(
    async_fn_in_trait,
    reason = "implemented only inside this crate, on one thread; no caller needs a Send bound"
)]
pub trait InboundMessage {
    /// The envelope sender.
    fn envelope_from(&self) -> String;
    /// The envelope recipient.
    fn envelope_to(&self) -> String;
    /// `Authentication-Results`, every copy joined with ", " as the runtime
    /// does, or `None` when the message has none.
    fn authentication_results(&self) -> Option<String>;
    /// The exact bytes that arrived. The stream reads once, so this is called
    /// once.
    async fn read_raw(&self) -> Result<Vec<u8>, EdgeError>;
    fn set_reject(&self, reason: &str);
    async fn forward(&self, to: &str) -> Result<(), EdgeError>;
    async fn reply(
        &self,
        from_name: &str,
        from_email: &str,
        notice: &GatewayNotice,
    ) -> Result<(), EdgeError>;
}

/// How long the inbound-API call may take before the message is refused for a
/// retry. The gateway classifies synchronously, at up to 20s a try with one
/// retry, so this sits above that pair rather than cutting a slow but healthy
/// classification short.
pub const GATEWAY_TIMEOUT_MS: u32 = 45_000;

/// How long the Mailgun call may take. Below the 25s the gateway allows its own
/// `/release` call, so the worker answers 502 itself rather than leaving the
/// gateway to give up on a request that is still running.
pub const MAILGUN_TIMEOUT_MS: u32 = 20_000;

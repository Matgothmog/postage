//! The `fetch` entry point: `POST /release`, which sends a held message on to
//! the address the gateway verified (replaces `fetch()`, `deliverUntouched` and
//! `retireHold` in `worker/src/index.ts`).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use postage_core::secret::offered_secret_matches;
use postage_shared::{RELEASE_PATH, ReleaseRequest};

use crate::inbound::RETRY_LATER;
use crate::ports::{Edge, EdgeError, MimeUpload};
use crate::settings::Settings;

/// How many times a released hold is asked to go away before we stop asking.
/// The message is already sent by then, so the extra attempt costs nothing but
/// a little latency on a path that is failing anyway, and buys back the common
/// case: a KV write that stumbles once and lands on the next try.
pub const RETIRE_ATTEMPTS: usize = 2;

/// How much of Mailgun's error body is kept for the log.
const MAILGUN_ERROR_EXCERPT: usize = 200;

/// The parts of an HTTP request the release route looks at.
#[derive(Debug, Clone, Copy)]
pub struct ReleaseCall<'a> {
    pub method: &'a str,
    pub path: &'a str,
    /// The `x-postage-secret` header, if the request had one.
    pub secret: Option<&'a str>,
    pub body: &'a [u8],
}

/// Every answer this route gives: a fixed plain-text line with a status, or the
/// one JSON success body, `{"sent":true}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseReply {
    Text { status: u16, text: &'static str },
    Sent,
}

impl ReleaseReply {
    const fn text(status: u16, text: &'static str) -> Self {
        Self::Text { status, text }
    }
}

/// The one route. Everything else here answers 404.
pub async fn handle_release<E: Edge>(
    edge: &E,
    settings: &Settings,
    call: ReleaseCall<'_>,
) -> ReleaseReply {
    if call.method != "POST" || call.path != RELEASE_PATH {
        return ReleaseReply::text(404, "Not found");
    }
    let offered = call.secret.unwrap_or_default();
    if call.secret.is_none() || !offered_secret_matches(offered, &settings.secret) {
        return ReleaseReply::text(401, "Bad secret");
    }

    let request = match read_request(call.body) {
        Ok(request) => request,
        Err(reply) => return reply,
    };

    let held = match edge.get_held(&request.token).await {
        Ok(held) => held,
        Err(cause) => {
            // Nothing has been sent and the hold is untouched, so asking again
            // costs the caller nothing - which makes this the same temporary
            // fault of ours that the inbound side refuses a session over, said
            // the same way. The token is the capability that releases the
            // message, so it is not logged.
            edge.log_error("release lookup failed", &[("cause", cause.to_string())]);
            return ReleaseReply::text(503, RETRY_LATER);
        }
    };
    let Some(held) = held else {
        return ReleaseReply::text(404, "Nothing is held under that token");
    };

    if let Err(cause) = deliver_untouched(edge, settings, &held, &request.to).await {
        // Kept, so the sender can be told it did not go and try again rather than
        // losing a message they were promised was safe. Logged, not returned:
        // Mailgun's own wording, or a raw network error, is not something the
        // caller needs or should read about our infrastructure.
        edge.log_error("release delivery failed", &[("cause", cause.to_string())]);
        return ReleaseReply::text(502, "Could not send it");
    }

    retire_hold(edge, &request.token).await;
    ReleaseReply::Sent
}

/// Distinguishes a body that is not JSON (400, one wording) from JSON that does
/// not name both a token and a destination (400, another).
fn read_request(body: &[u8]) -> Result<ReleaseRequest, ReleaseReply> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| ReleaseReply::text(400, "Body must be JSON"))?;
    let required = ReleaseReply::text(400, "token and to are required");
    let request: ReleaseRequest = serde_json::from_value(value).map_err(|_| required)?;
    if request.token.is_empty() || request.to.is_empty() {
        return Err(required);
    }
    Ok(request)
}

/// Puts the message back on the wire as the bytes that arrived.
///
/// Every option here turns something off. Mailgun would otherwise sign the
/// message with our key and rewrite every link in the body for click tracking,
/// and rewriting the body changes what the sender's own signature covers - the
/// message would arrive looking forged by exactly the measure this gateway
/// exists to apply. The envelope sender is Mailgun's, as it is in any forward;
/// the `From:` header, which is what the recipient sees and what DMARC aligns
/// against, is untouched.
async fn deliver_untouched<E: Edge>(
    edge: &E,
    settings: &Settings,
    raw: &[u8],
    to: &str,
) -> Result<(), EdgeError> {
    let credentials = STANDARD.encode(format!("api:{}", settings.mailgun_api_key));
    let upload = MimeUpload {
        url: format!(
            "{}/v3/{}/messages.mime",
            settings.mailgun_api_base, settings.mailgun_domain
        ),
        authorization: format!("Basic {credentials}"),
        fields: vec![
            ("to", to.to_owned()),
            ("o:dkim", "no".to_owned()),
            ("o:tracking", "no".to_owned()),
            ("o:tracking-clicks", "no".to_owned()),
            ("o:tracking-opens", "no".to_owned()),
        ],
        message: raw.to_vec(),
    };

    let answer = edge.post_mime(&upload).await?;
    if answer.is_success() {
        return Ok(());
    }
    let excerpt: String = answer.body.chars().take(MAILGUN_ERROR_EXCERPT).collect();
    Err(EdgeError(format!(
        "Mailgun returned {}: {excerpt}",
        answer.status
    )))
}

/// Spends the token, once the message it releases has already gone out.
///
/// The order is forced, and it is the whole design. KV holds the only copy, so
/// the bytes cannot be dropped before Mailgun has taken them - which makes a
/// release at-least-once, because the send is irreversible by the time the key
/// is removed. The alternative, retiring the token first, is at-most-once: a
/// Mailgun outage, much the commoner failure, would then destroy a message the
/// recipient explicitly asked for, with no copy left anywhere to retry from. A
/// message arriving twice is a nuisance; a message that no longer exists cannot
/// be recovered by anyone.
///
/// So a delete that fails must not become an error. The mail is out, an error
/// is an invitation to retry a request that worked, and the retry would send it
/// a second time. Answering `sent: true` is both the truth and the guard.
///
/// Retried because what is left behind is a live capability and a KV write that
/// fails once usually does not fail twice. If every attempt fails the key
/// survives its own release, and nothing in a stateless handler can prevent
/// that. It expires on the deadline set when the message was held, so it does
/// not accumulate.
async fn retire_hold<E: Edge>(edge: &E, token: &str) {
    let mut last_cause = EdgeError(String::new());
    for _ in 0..RETIRE_ATTEMPTS {
        match edge.delete_held(token).await {
            Ok(()) => return,
            Err(cause) => last_cause = cause,
        }
    }
    edge.log_error(
        "hold not retired after release",
        &[
            ("attempts", RETIRE_ATTEMPTS.to_string()),
            ("cause", last_cause.to_string()),
        ],
    );
}

#[cfg(test)]
mod tests;

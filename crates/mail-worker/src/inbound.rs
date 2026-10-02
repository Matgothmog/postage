//! The `email` entry point: asks the gateway what to do with a message and
//! carries the answer out (replaces `email()` in `worker/src/index.ts`).

use postage_shared::{GatewayAction, GatewayNotice, GatewayVerdict};
use serde::Serialize;

use crate::auth_results::auth_results;
use crate::mime::read_message;
use crate::ports::{Edge, EdgeError, InboundMessage, JsonPost};
use crate::settings::Settings;

/// Only reached if the gateway sends a hold with no deadline on it. Not the
/// source of truth for how long a message is kept - that is the gateway's - just
/// a floor under it, so nothing can be stored indefinitely by omission.
pub const FALLBACK_HOLD_SECONDS: u64 = 24 * 60 * 60;

/// Said whenever the fault is ours rather than the sender's, and deliberately
/// said the same way every time: it is a temporary refusal, and the sending
/// MTA's own retry is what recovers the message afterwards.
pub const RETRY_LATER: &str = "Postage is temporarily unavailable, please retry";

/// How much of an error body is kept for the log.
const GATEWAY_ERROR_EXCERPT: usize = 300;

/// What the gateway is told about a message. `null` rather than absent for an
/// unknown result, because the gateway reads those keys.
#[derive(Debug, Serialize)]
struct GatewayRequest<'a> {
    from: &'a str,
    to: &'a str,
    subject: &'a str,
    body: &'a str,
    spf: Option<&'a str>,
    dkim: Option<&'a str>,
    dmarc: Option<&'a str>,
    /// The `From:` header address DMARC evaluated, so the gateway can tell
    /// whether a DMARC pass says anything about the envelope sender.
    header_from: Option<&'a str>,
}

pub async fn handle_email<E: Edge, M: InboundMessage>(edge: &E, settings: &Settings, message: &M) {
    // Buffered before parsing, because the raw stream reads once and holding a
    // message means keeping exactly these bytes rather than a rendering of them.
    let raw = match message.read_raw().await {
        Ok(raw) => raw,
        Err(cause) => {
            edge.log_error("message read failed", &[("cause", cause.to_string())]);
            message.set_reject(RETRY_LATER);
            return;
        }
    };

    let verdict = match ask_gateway(edge, settings, message, &raw).await {
        Ok(verdict) => verdict,
        Err(cause) => {
            // Without this the sender retries into a wall nobody can explain.
            edge.log_error(
                "classify failed",
                &[
                    ("gateway", safe_host(&settings.api_url)),
                    ("cause", cause.to_string()),
                ],
            );
            // Refused rather than forwarded unfiltered, so the sending MTA holds
            // the message and retries rather than the recipient losing the gate.
            message.set_reject(RETRY_LATER);
            return;
        }
    };

    match verdict.action {
        GatewayAction::Forward => forward_untouched(edge, message, &verdict).await,
        GatewayAction::Hold => hold(edge, message, &raw, verdict).await,
        GatewayAction::Reject => refuse(message, &verdict),
    }
}

async fn ask_gateway<E: Edge, M: InboundMessage>(
    edge: &E,
    settings: &Settings,
    message: &M,
    raw: &[u8],
) -> Result<GatewayVerdict, EdgeError> {
    let readable = read_message(raw);
    let header = message.authentication_results();
    let auth = auth_results(header.as_deref());
    let from = message.envelope_from();
    let to = message.envelope_to();
    let payload = GatewayRequest {
        from: &from,
        to: &to,
        subject: &readable.subject,
        body: &readable.body,
        spf: auth.spf.as_deref(),
        dkim: auth.dkim.as_deref(),
        dmarc: auth.dmarc.as_deref(),
        header_from: readable.header_from.as_deref(),
    };
    let body = serde_json::to_string(&payload).map_err(|cause| EdgeError(cause.to_string()))?;

    let answer = edge
        .post_json(&JsonPost {
            url: format!("{}/api/mail/inbound", settings.api_url),
            secret: settings.secret.clone(),
            body,
        })
        .await?;

    // 404 is a real answer (no such inbox), not a failure to reach the gateway.
    if answer.status == 404 {
        return Ok(GatewayVerdict::reject("unknown_inbox", None));
    }
    if !answer.is_success() {
        let excerpt: String = answer.body.chars().take(GATEWAY_ERROR_EXCERPT).collect();
        return Err(EdgeError(format!(
            "Gateway returned {}: {excerpt}",
            answer.status
        )));
    }
    serde_json::from_str(&answer.body).map_err(|cause| EdgeError(cause.to_string()))
}

/// Untouched, so the sender's DKIM signature still covers what arrives and
/// their address still displays as the one that wrote it.
async fn forward_untouched<E: Edge, M: InboundMessage>(
    edge: &E,
    message: &M,
    verdict: &GatewayVerdict,
) {
    let Some(destination) = verdict.to.as_deref().filter(|to| !to.is_empty()) else {
        reject_incoherent_verdict(edge, message, "forward without a destination");
        return;
    };
    if message.forward(destination).await.is_err() {
        // A destination Cloudflare will not accept must refuse the session, so
        // the sending MTA retries. Failing the handler loses the message instead.
        message.set_reject("Postage could not deliver to that inbox, please retry");
    }
}

async fn hold<E: Edge, M: InboundMessage>(
    edge: &E,
    message: &M,
    raw: &[u8],
    verdict: GatewayVerdict,
) {
    let Some(token) = verdict.token.as_deref().filter(|token| !token.is_empty()) else {
        reject_incoherent_verdict(edge, message, "hold without a token");
        return;
    };

    let held_until = verdict
        .held_until
        .unwrap_or_else(|| edge.now_seconds() + FALLBACK_HOLD_SECONDS);
    if let Err(cause) = edge.put_held(token, raw, held_until).await {
        // KV holds the only copy of the message. A put that did not land leaves
        // the release link pointing at nothing, so telling the sender it is
        // being kept would be a lie about mail we no longer have. Refusing
        // instead leaves the bytes at the sending MTA, which is then the only
        // place they still exist. The token is the capability that releases the
        // message, so it is not logged.
        edge.log_error("hold failed", &[("cause", cause.to_string())]);
        message.set_reject(RETRY_LATER);
        return;
    }

    if let Some(notice) = &verdict.notice
        && replied(message, notice).await
    {
        return;
    }

    message.set_reject(
        verdict
            .bounce
            .as_deref()
            .unwrap_or("Held. See the link in this message to release it"),
    );
}

fn refuse<M: InboundMessage>(message: &M, verdict: &GatewayVerdict) {
    if verdict.reason.as_deref() == Some("unknown_inbox") {
        message.set_reject("No such address at this domain");
        return;
    }
    message.set_reject(verdict.bounce.as_deref().unwrap_or("Not delivered."));
}

/// A verdict that asks for something and omits the one field that carries it
/// out: a forward naming nowhere, a hold with nothing to key it under. Nothing
/// here can complete it, and the generic refusal would file our own bug as an
/// ordinary "not delivered" and lose a message the gateway meant to keep.
/// Refused as temporary instead, so the message waits at the sending MTA while
/// the log names what was wrong with the answer.
fn reject_incoherent_verdict<E: Edge, M: InboundMessage>(edge: &E, message: &M, what: &str) {
    edge.log_error("incoherent verdict", &[("verdict", what.to_owned())]);
    message.set_reject(RETRY_LATER);
}

/// True if the sender was told. A refusal here is not a failure worth losing the
/// message over - Cloudflare will not let us reply to an unauthenticated sender,
/// which is exactly the case where replying would mail the wrong person - so the
/// caller falls back to refusing inside the session.
async fn replied<M: InboundMessage>(message: &M, notice: &GatewayNotice) -> bool {
    message
        .reply("Postage", &message.envelope_to(), notice)
        .await
        .is_ok()
}

/// The host on its own. Enough to tell a misconfigured gateway from an
/// unreachable one, without writing a configured URL into the logs whole.
fn safe_host(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return "unparseable POSTAGE_API_URL".to_owned();
    };
    match (parsed.host_str(), parsed.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_owned(),
        (None, _) => String::new(),
    }
}

#[cfg(test)]
mod tests;

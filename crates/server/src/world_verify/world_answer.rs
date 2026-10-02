//! Asking World, and reading its answer.
//!
//! Three ways this fails, kept apart: World cannot be reached, or answers
//! with something that makes no sense (ours to answer for, 502); World
//! rejects the proof (a verdict on what the sender sent, 400); World accepts
//! a proof that never mentions Selfie Check (the sender proved something,
//! just not what this route needs, 400).

use axum::http::StatusCode;
use postage_core::world_id_messages::{GENERIC_WORLD_ID_FAILURE_MESSAGE, world_id_failure_message};
use serde_json::Value;

use super::proof::WorldIdProof;
use super::{Stopped, describe_failure, token_fingerprint};
use crate::app::AppState;
use crate::config::required;
use crate::log;
use crate::routes::js::{field, is_truthy, request_json};
use crate::world::WorldReply;

const SELFIE_IDENTIFIER: &str = "selfie";
const NO_SELFIE_CREDENTIAL: &str = "Proof did not include a Selfie Check credential";
const UNREADABLE: &str = "World ID returned an unreadable response";

/// Forwards the IDKit result to World and returns the Selfie Check
/// credential's nullifier.
///
/// The relying party id is read only here, after the context has been spent,
/// as the TypeScript read it; its absence is our misconfiguration, a bare 500.
pub(crate) async fn verify_with_world(
    state: &AppState,
    proof: &WorldIdProof<'_>,
    token: &str,
) -> Result<String, Stopped> {
    let rp_id = required(state.env().lookup(), "WORLD_RP_ID").map_err(Stopped::failed)?;
    let reply = match state.world().verify(&rp_id, proof.raw).await {
        Ok(reply) => reply,
        Err(cause) => {
            log::error(
                "could not reach world id verify endpoint",
                &[
                    ("tokenRef", &token_fingerprint(token)),
                    ("reason", &describe_failure(state.env(), &cause)),
                ],
            );
            return Err(Stopped::refused(
                StatusCode::BAD_GATEWAY,
                "Could not reach World ID",
            ));
        }
    };
    judge_world_reply(reply, token)
}

/// What World's answer means for this sender.
pub(crate) fn judge_world_reply(reply: WorldReply, token: &str) -> Result<String, Stopped> {
    let status = reply.status;
    let payload = read_payload(reply).map_err(|reason| unreadable(token, status, &reason))?;

    // World's own outage is not a verdict on the proof, so it must not fall
    // through to the rejection below and read as a bad request. Its detail
    // describes its own failure and is logged, never handed back.
    if status >= 500 {
        log::error(
            "world id verify endpoint returned a server error",
            &[
                ("tokenRef", &token_fingerprint(token)),
                ("status", &status),
                ("detail", &shown(field(&payload, "detail"))),
                ("code", &shown(field(&payload, "code"))),
            ],
        );
        return Err(Stopped::refused(
            StatusCode::BAD_GATEWAY,
            "World ID is currently unavailable",
        ));
    }

    if !(200..300).contains(&status) || !is_truthy(field(&payload, "success")) {
        let message = log_and_describe_world_verify_failure(
            field(&payload, "code"),
            field(&payload, "detail"),
            token,
        );
        return Err(Stopped::refused(StatusCode::BAD_REQUEST, message));
    }

    selfie_nullifier(&payload, token, status)
}

/// The body as JSON. `null` is refused with the rest: every field read off it
/// threw in the TypeScript, which answered World's nonsense with a bare 500.
fn read_payload(reply: WorldReply) -> Result<Value, String> {
    let bytes = reply.body?;
    match request_json(&bytes) {
        Ok(Value::Null) => Err("the body was null".to_owned()),
        Ok(payload) => Ok(payload),
        Err(error) => Err(error.to_string()),
    }
}

/// The one Selfie Check result's nullifier, walked the same defensive way the
/// request was: World's response carries nothing tying a result to the one
/// selfie entry the request held, so exactly one is enforced here too.
///
/// A structure no honest World answer has (`results` that is not a list, a
/// `null` result, a nullifier that is not a string) is unreadable: the
/// TypeScript threw on each and answered with a bare 500.
fn selfie_nullifier(payload: &Value, token: &str, status: u16) -> Result<String, Stopped> {
    let results = match field(payload, "results") {
        None | Some(Value::Null) => &[],
        Some(Value::Array(results)) => results.as_slice(),
        Some(_) => return Err(unreadable(token, status, "results was not an array")),
    };
    if results.iter().any(Value::is_null) {
        return Err(unreadable(token, status, "a result was null"));
    }
    let selfies: Vec<&Value> = results
        .iter()
        .filter(|result| {
            field(result, "identifier").and_then(Value::as_str) == Some(SELFIE_IDENTIFIER)
        })
        .collect();
    let [selfie] = selfies.as_slice() else {
        return Err(no_selfie_credential());
    };
    let nullifier = field(selfie, "nullifier");
    if !is_truthy(field(selfie, "success")) || !is_truthy(nullifier) {
        return Err(no_selfie_credential());
    }
    match nullifier {
        Some(Value::String(nullifier)) => Ok(nullifier.clone()),
        _ => Err(unreadable(
            token,
            status,
            "the selfie nullifier was not a string",
        )),
    }
}

fn no_selfie_credential() -> Stopped {
    Stopped::refused(StatusCode::BAD_REQUEST, NO_SELFIE_CREDENTIAL)
}

fn unreadable(token: &str, status: u16, reason: &str) -> Stopped {
    log::error(
        "world id verify endpoint returned an unreadable response",
        &[
            ("tokenRef", &token_fingerprint(token)),
            ("status", &status),
            ("reason", &reason),
        ],
    );
    Stopped::refused(StatusCode::BAD_GATEWAY, UNREADABLE)
}

/// Sender-safe copy for a proof World rejected, looked up from `code` in the
/// same table IDKit's client-side codes are described from, and logged: this
/// is the only record of which code World actually sent. World's `detail` is
/// prose for a developer, so it is logged and never returned. An unknown
/// code, or one that is not a string, gets the generic copy and a louder line
/// so the table can learn it.
fn log_and_describe_world_verify_failure(
    code: Option<&Value>,
    detail: Option<&Value>,
    token: &str,
) -> &'static str {
    let message = code
        .and_then(Value::as_str)
        .and_then(world_id_failure_message);
    let event = match (message, code) {
        (Some(_), Some(code)) => format!(
            "world id verify endpoint rejected a proof: {}",
            shown(Some(code))
        ),
        _ => "world id verify endpoint rejected a proof with a code this route does not recognise"
            .to_owned(),
    };
    log::error(
        &event,
        &[
            ("tokenRef", &token_fingerprint(token)),
            ("code", &shown(code)),
            ("detail", &shown(detail)),
        ],
    );
    message.unwrap_or(GENERIC_WORLD_ID_FAILURE_MESSAGE)
}

/// A field World sent, as a log line shows it: a string as itself, anything
/// else as JSON, and a missing field as JavaScript printed it.
fn shown(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

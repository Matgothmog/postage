//! Stand-ins the inbound gateway's tests share: the chain, the classifier's
//! model, and the settings the route reads (`web/test/chain.ts`,
//! `web/test/model.ts`). Each is a loopback server this process owns, so
//! nothing a test does can leave the machine.

use std::time::Duration;

use serde_json::json;

use crate::classify::Classifier;
use crate::config::Env;
use crate::http_stub::{Reply, Stub, serve_with};

/// What `effectiveFloor` answers with, so a test can say where a price came
/// from rather than restating the arithmetic that produced it.
pub(crate) const FLOOR: u128 = 10_000_000_000_000_000;

/// The inbox's wallet, stored lower-cased as every wallet is.
pub(crate) const WALLET: &str = "0x1111111111111111111111111111111111111111";

/// What a refusing node says, which the chain fault's log line must carry.
pub(crate) const CHAIN_REFUSAL: &str = "the node is not accepting calls";

/// The settings the route reads, all set. Throwaway values only.
pub(crate) fn test_env() -> Env {
    test_env_without(&[])
}

/// [`test_env`] with `missing` taken away.
pub(crate) fn test_env_without(missing: &[&str]) -> Env {
    let key = format!("0x{}", "11".repeat(32));
    let secret = "x".repeat(32);
    let vars = [
        ("MAIL_WEBHOOK_SECRET", "secret"),
        ("APP_URL", "http://localhost"),
        ("MESSAGE_ID_SECRET", secret.as_str()),
        ("CLASSIFIER_PRIVATE_KEY", key.as_str()),
    ];
    Env::fixed(vars.into_iter().filter(|(name, _)| !missing.contains(name)))
}

/// A JSON-RPC node answering every `eth_call` with [`FLOOR`], or refusing
/// every one with a JSON-RPC error: the other thing a chain does to a
/// gateway carrying real mail.
pub(crate) async fn chain_node(failing: bool) -> Stub {
    serve_with(move |sent| {
        let id = sent.body["id"].clone();
        let body = if failing {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": CHAIN_REFUSAL } })
        } else {
            json!({ "jsonrpc": "2.0", "id": id, "result": floor_word() })
        };
        Reply::new(200, body.to_string())
    })
    .await
}

/// [`FLOOR`] as the 32-byte word `effectiveFloor` returns.
pub(crate) fn floor_word() -> String {
    format!("0x{FLOOR:064x}")
}

/// A stand-in for the classifier's model. `None` refuses every request with a
/// 400, which is what the classifier degrades on; `Some(tier)` answers with
/// that verdict.
pub(crate) async fn model(answer: Option<&'static str>) -> Stub {
    serve_with(move |_| match answer {
        None => Reply::new(
            400,
            json!({
                "type": "error",
                "error": { "type": "invalid_request_error", "message": "no model is reachable from a test" },
            })
            .to_string(),
        ),
        Some(tier) => Reply::new(200, model_saying(tier)),
    })
    .await
}

/// The model's answer, in the shape the Messages API wraps it in.
fn model_saying(tier: &str) -> String {
    let verdict = json!({
        "tier": tier,
        "confidence": 0.9,
        "reasons": ["a stub answers for the model here"],
    });
    json!({
        "id": "msg_stub",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-5",
        "content": [{ "type": "text", "text": verdict.to_string() }],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": { "input_tokens": 0, "output_tokens": 0 },
    })
    .to_string()
}

/// A classifier pointed at `model`, asking once.
pub(crate) fn classifier(model: &Stub) -> Classifier {
    Classifier::new(Ok("test".to_owned()), &model.base).with_retries(0, Duration::ZERO)
}

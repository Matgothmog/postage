//! Every outside service the server and the browser talk to, served from one
//! loopback origin under `/__stub/<service>/...` and recording each request.
//! The journey script reads the recordings back through `/__stub/admin/...`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use alloy_primitives::{Address, B256};
use alloy_sol_types::SolCall;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use postage_core::contracts::PostageEscrow;
use serde::Serialize;
use serde_json::{Value, json};

use crate::rpc;

/// Epoch seconds a Cloudflare address is reported verified at once confirmed.
const CONFIRMED_AT: &str = "2026-10-02T12:00:00Z";
/// Far enough ahead that an attestation reads as current after the clock jumps.
const HUMAN_FOR_SECONDS: u64 = 400 * 24 * 60 * 60;

#[derive(Debug, Clone, Serialize)]
pub struct Recorded {
    pub seq: u64,
    pub service: String,
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub body: Value,
}

#[derive(Default)]
pub struct Hub {
    log: Mutex<Vec<Recorded>>,
    next_seq: AtomicU64,
    clock_offset: AtomicI64,
    nonce: AtomicU64,
    cloudflare_confirmed: AtomicBool,
    cloudflare_addresses: Mutex<Vec<(String, String)>>,
    payments: Mutex<HashMap<B256, Address>>,
    transactions: Mutex<HashSet<B256>>,
    attested_at: AtomicU64,
    worker_refuses: AtomicBool,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Hub {
    pub fn clock_offset(&self) -> i64 {
        self.clock_offset.load(Ordering::SeqCst)
    }

    pub fn next_nonce(&self) -> u64 {
        self.nonce.fetch_add(1, Ordering::SeqCst)
    }

    pub fn payer_of(&self, message_id: &B256) -> Option<Address> {
        locked(&self.payments).get(message_id).copied()
    }

    pub fn record_transaction(&self, hash: B256) {
        locked(&self.transactions).insert(hash);
        // Any attestation makes its wallet a person from now on.
        self.attested_at.store(1, Ordering::SeqCst);
    }

    pub fn knows_transaction(&self, hash: &B256) -> bool {
        locked(&self.transactions).contains(hash)
    }

    pub fn human_until(&self) -> u64 {
        match self.attested_at.load(Ordering::SeqCst) {
            0 => 0,
            _ => {
                HUMAN_FOR_SECONDS + u64::try_from(self.clock_offset()).unwrap_or(0) + now_seconds()
            }
        }
    }

    fn record(
        &self,
        service: &str,
        method: &Method,
        path: &str,
        query: Option<String>,
        body: Value,
    ) {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        locked(&self.log).push(Recorded {
            seq,
            service: service.to_owned(),
            method: method.to_string(),
            path: path.to_owned(),
            query,
            body,
        });
    }

    fn recorded(&self, service: Option<&str>) -> Vec<Recorded> {
        locked(&self.log)
            .iter()
            .filter(|entry| service.is_none_or(|wanted| entry.service == wanted))
            .cloned()
            .collect()
    }
}

pub fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

pub fn router(hub: Arc<Hub>) -> Router {
    Router::new()
        .route("/__stub/{service}/{*rest}", any(stub))
        .with_state(hub)
}

async fn stub(
    State(hub): State<Arc<Hub>>,
    Path((service, rest)): Path<(String, String)>,
    method: Method,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let path = format!("/{rest}");
    if service != "admin" {
        hub.record(&service, &method, &path, query.clone(), parsed.clone());
    }
    match service.as_str() {
        "anthropic" => json_ok(anthropic(&parsed)),
        "resend" => json_ok(json!({ "id": "re_e2e" })),
        "cloudflare" => cloudflare(&hub, &method, &path, &parsed),
        "worker" => worker(&hub, &path, &headers),
        "world" => json_ok(json!({ "success": true, "nullifier": "0x01" })),
        "graph" | "graph-gw" => json_ok(graph(&parsed)),
        "rpc" => rpc_or_payment(&hub, &path, &parsed),
        "admin" => admin(&hub, &method, &path, query.as_deref()),
        _ => (StatusCode::NOT_FOUND, "unknown stub").into_response(),
    }
}

fn json_ok(value: Value) -> Response {
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        value.to_string(),
    )
        .into_response()
}

/// The model's verdict, tier chosen by a `e2e-tier:<tier>` marker in the
/// message under review (default `human`).
fn anthropic(request: &Value) -> Value {
    let text = request.to_string();
    let tier = ["commercial", "dangerous", "important", "human"]
        .into_iter()
        .find(|tier| text.contains(&format!("e2e-tier:{tier}")))
        .unwrap_or("human");
    let verdict = json!({
        "tier": tier,
        "confidence": 0.9,
        "reasons": [format!("the e2e stub answers {tier}")],
    });
    json!({
        "id": "msg_e2e",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-5",
        "content": [{ "type": "text", "text": verdict.to_string() }],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": { "input_tokens": 0, "output_tokens": 0 },
    })
}

fn cloudflare(hub: &Hub, method: &Method, path: &str, body: &Value) -> Response {
    let destination = |id: &str, email: &str| {
        let verified = hub
            .cloudflare_confirmed
            .load(Ordering::SeqCst)
            .then_some(CONFIRMED_AT);
        json!({ "id": id, "email": email, "verified": verified })
    };
    let envelope =
        |result: Value| json!({ "success": true, "errors": [], "messages": [], "result": result });

    if let Some(id) = path.rsplit_once("/addresses/").map(|(_, id)| id) {
        let known = locked(&hub.cloudflare_addresses)
            .iter()
            .find(|(known, _)| known == id)
            .cloned();
        return match known {
            Some((id, email)) => json_ok(envelope(destination(&id, &email))),
            None => (
                StatusCode::NOT_FOUND,
                "{\"success\":false,\"errors\":[{\"message\":\"not found\"}],\"result\":null}",
            )
                .into_response(),
        };
    }
    if method == Method::POST {
        let email = body
            .get("email")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut addresses = locked(&hub.cloudflare_addresses);
        let id = format!("addr-{}", addresses.len() + 1);
        addresses.push((id.clone(), email.to_owned()));
        return json_ok(envelope(destination(&id, email)));
    }
    let listed: Vec<Value> = locked(&hub.cloudflare_addresses)
        .iter()
        .map(|(id, email)| destination(id, email))
        .collect();
    let total = listed.len();
    json_ok(json!({
        "success": true, "errors": [], "messages": [],
        "result": listed, "result_info": { "total_count": total },
    }))
}

fn worker(hub: &Hub, path: &str, headers: &HeaderMap) -> Response {
    let has_secret = headers.contains_key("x-postage-secret");
    if path != "/release" || !has_secret {
        return (StatusCode::UNAUTHORIZED, "no").into_response();
    }
    if hub.worker_refuses.load(Ordering::SeqCst) {
        return (StatusCode::BAD_GATEWAY, "refused").into_response();
    }
    json_ok(json!({ "sent": true }))
}

/// The subgraph: the network page's overview for its query, nothing found for
/// the per-sender history and ENS lookups.
fn graph(request: &Value) -> Value {
    let query = request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if query.contains("query Overview") {
        return json!({ "data": {
            "vaults": [{ "totalFunded": "30000000000000000", "toTreasury": "9000000000000000",
                "toSponsorship": "15000000000000000", "refilledToRelayer": "6000000000000000", "fundingEvents": 3 }],
            "enclaves": [{ "id": "0xe2e0000000000000000000000000000000000001",
                "measurement": "0x7f3a9c1d5b8e2a4f6c0d9e1b3a5c7e9f1b2d4f6a8c0e2a4c6e8b0d2f4a6c8e0b",
                "revoked": false, "registeredAt": "1790000000" }],
            "humanAttestations": [
                { "id": "0xaaaa00000000000000000000000000000000aaaa", "attestedAt": "1790000100" },
                { "id": "0xbbbb00000000000000000000000000000000bbbb", "attestedAt": "1790000200" }],
            "inboxes": [{ "id": "0x70997970c51812dc3a010c7d01b50e0d17dc79c8", "floorPrice": "50000000000000000",
                "receivedCount": 3, "earned": "135000000000000000", "claimed": "0" }],
            "senders": [{ "id": "0x3c44cdddb6a900fa2b585dd299e03d12fa4293bc", "paidCount": 3,
                "totalPaid": "150000000000000000", "spamReports": 0, "spamRate": "0", "humanUntil": null }],
            "payments": [{ "id": "0xc0de000000000000000000000000000000000000000000000000000000000001",
                "tier": "2", "amount": "45000000000000000", "toVault": "5000000000000000",
                "reportedAsSpam": false, "paidAt": "1790000300",
                "tx": "0xabababababababababababababababababababababababababababababababab",
                "sender": { "id": "0x3c44cdddb6a900fa2b585dd299e03d12fa4293bc" },
                "inbox": { "id": "0x70997970c51812dc3a010c7d01b50e0d17dc79c8" } }],
        } });
    }
    json!({ "data": { "sender": null, "domains": [] } })
}

/// `POST /__stub/rpc` is the node; `POST /__stub/rpc/payment` is the browser
/// telling the node that its (mock) wallet sent a transaction, which is the
/// one thing a real chain would have learned by itself.
fn rpc_or_payment(hub: &Hub, path: &str, body: &Value) -> Response {
    if path == "/payment" {
        return match register_payment(hub, body) {
            Ok(()) => json_ok(json!({ "ok": true })),
            Err(reason) => (StatusCode::BAD_REQUEST, reason).into_response(),
        };
    }
    json_ok(rpc::handle(hub, body))
}

fn register_payment(hub: &Hub, body: &Value) -> Result<(), String> {
    let text = |name: &str| {
        body.get(name)
            .and_then(Value::as_str)
            .ok_or(format!("{name} is missing"))
    };
    let payer: Address = text("from")?
        .parse()
        .map_err(|_| "from is not an address")?;
    let data = alloy_primitives::hex::decode(text("data")?).map_err(|_| "data is not hex")?;
    let call =
        PostageEscrow::payToSendCall::abi_decode(&data).map_err(|error| error.to_string())?;
    locked(&hub.payments).insert(call.messageId, payer);
    Ok(())
}

fn admin(hub: &Hub, method: &Method, path: &str, query: Option<&str>) -> Response {
    let param = |name: &str| {
        query?
            .split('&')
            .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
            .map(str::to_owned)
    };
    match (method.as_str(), path) {
        ("GET", "/requests") => json_ok(json!(hub.recorded(param("service").as_deref()))),
        ("POST", "/cloudflare/confirm") => {
            hub.cloudflare_confirmed.store(true, Ordering::SeqCst);
            json_ok(json!({ "confirmed": true }))
        }
        ("POST", "/worker/refuse") => {
            hub.worker_refuses.store(true, Ordering::SeqCst);
            json_ok(json!({ "refusing": true }))
        }
        ("POST", "/clock/advance") => {
            let seconds: i64 = param("seconds")
                .and_then(|text| text.parse().ok())
                .unwrap_or(0);
            let offset = hub.clock_offset.fetch_add(seconds, Ordering::SeqCst) + seconds;
            json_ok(json!({ "offsetSeconds": offset }))
        }
        ("GET", "/state") => json_ok(json!({
            "clockOffsetSeconds": hub.clock_offset(),
            "cloudflareConfirmed": hub.cloudflare_confirmed.load(Ordering::SeqCst),
            "payments": locked(&hub.payments).iter()
                .map(|(message, payer)| json!({ "messageId": message.to_string(), "payer": payer.to_string() }))
                .collect::<Vec<_>>(),
            "transactions": locked(&hub.transactions).iter().map(ToString::to_string).collect::<Vec<_>>(),
        })),
        _ => (StatusCode::NOT_FOUND, "unknown admin route").into_response(),
    }
}

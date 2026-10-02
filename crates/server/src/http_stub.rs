//! A loopback HTTP endpoint for tests: it records every request it is sent and
//! answers from a canned status and body per path, or from a function of the
//! request when the answer depends on it.
//!
//! Stands in for every outbound service the server calls (The Graph,
//! Cloudflare, Resend, the mail worker), so no test reaches the network.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde_json::Value;

/// One request as the stub received it.
#[derive(Debug, Clone)]
pub(crate) struct Sent {
    pub method: String,
    pub path: String,
    /// The query string without its `?`, when there was one.
    pub query: Option<String>,
    pub headers: HeaderMap,
    pub content_type: Option<String>,
    /// The body parsed as JSON, or `Null` when it was not JSON.
    pub body: Value,
}

impl Sent {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    /// The value of one query parameter.
    pub(crate) fn query_param(&self, name: &str) -> Option<&str> {
        self.query.as_deref()?.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key == name).then_some(value)
        })
    }
}

/// What the stub answers with, and optionally how long it sits on the answer
/// first.
#[derive(Debug, Clone)]
pub(crate) struct Reply {
    pub status: u16,
    pub body: String,
    pub delay: Duration,
}

impl Reply {
    pub(crate) fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    pub(crate) fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

type Respond = Arc<dyn Fn(&Sent) -> Reply + Send + Sync>;

#[derive(Clone)]
struct Shared {
    respond: Respond,
    sent: Arc<Mutex<Vec<Sent>>>,
}

#[derive(Debug)]
pub(crate) struct Stub {
    pub base: String,
    sent: Arc<Mutex<Vec<Sent>>>,
}

impl Stub {
    pub(crate) fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }
}

async fn answer(State(shared): State<Shared>, request: Request) -> impl IntoResponse {
    let method = request.method().to_string();
    let path = request.uri().path().to_owned();
    let query = request.uri().query().map(str::to_owned);
    let headers = request.headers().clone();
    let content_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .unwrap();
    let sent = Sent {
        method,
        path,
        query,
        headers,
        content_type,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    };
    let reply = (shared.respond)(&sent);
    shared.sent.lock().unwrap().push(sent);
    if !reply.delay.is_zero() {
        tokio::time::sleep(reply.delay).await;
    }
    (StatusCode::from_u16(reply.status).unwrap(), reply.body)
}

/// Serves `answers` (path → status and body) on a loopback port; any other
/// path is a 404 with an empty body.
pub(crate) async fn serve(answers: &[(&str, u16, &str)]) -> Stub {
    let answers: HashMap<String, Reply> = answers
        .iter()
        .map(|(path, status, body)| ((*path).to_owned(), Reply::new(*status, *body)))
        .collect();
    serve_with(move |sent| {
        answers
            .get(&sent.path)
            .cloned()
            .unwrap_or_else(|| Reply::new(404, ""))
    })
    .await
}

/// Serves whatever `respond` makes of each request, on a loopback port.
pub(crate) async fn serve_with<F>(respond: F) -> Stub
where
    F: Fn(&Sent) -> Reply + Send + Sync + 'static,
{
    let shared = Shared {
        respond: Arc::new(respond),
        sent: Arc::default(),
    };
    let sent = shared.sent.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().fallback(answer).with_state(shared);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Stub { base, sent }
}

/// A loopback URL nothing is listening on, so a request to it is refused.
pub(crate) fn closed_port() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    closed
}

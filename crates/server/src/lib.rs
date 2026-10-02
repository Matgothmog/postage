//! Axum router served by the Vercel function in `api/index.rs`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod auth;
pub mod chain;
pub mod claims;
pub mod classify;
pub mod cloudflare;
pub mod config;
pub mod db;
pub mod gate;
pub mod graph;
pub mod hold;
#[cfg(test)]
mod http_stub;
mod log;
pub mod mail;
pub mod network;
pub mod privy;
pub mod reputation;

use axum::{Json, Router, routing::get};
use serde_json::{Value, json};

/// Builds the API router. Vercel rewrites every request to the single function,
/// so routes carry their full `/api/...` path.
pub fn router() -> Router {
    Router::new().route("/api/health", get(health))
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_reports_ok() {
        let Json(body) = health().await;
        assert_eq!(body, json!({ "status": "ok" }));
    }
}

//! Axum router served by the Vercel function in `api/index.rs`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod classify;
pub mod config;
pub mod db;
pub mod privy;

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

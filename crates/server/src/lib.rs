//! Axum router served by the Vercel function in `api/index.rs`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod app;
pub mod auth;
pub mod chain;
pub mod claims;
pub mod classify;
pub mod cloudflare;
pub mod config;
pub mod db;
pub mod faults;
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
mod routes;
pub mod world;
pub mod world_verify;

pub use app::AppState;
pub use routes::router;

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use super::*;
    use crate::config::Env;
    use crate::routes::testing::{get, send};

    #[tokio::test]
    async fn health_reports_ok() {
        let app = router(AppState::builder(Env::empty()).build());

        let answer = send(app, get("/api/health")).await;

        assert_eq!(answer.status, StatusCode::OK);
        assert_eq!(answer.body, json!({ "status": "ok" }));
    }
}

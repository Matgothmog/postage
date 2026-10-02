//! Cloudflare Worker (workers-rs) for inbound mail: asks the gateway what to do
//! with each message and holds, forwards or refuses it, and releases held mail
//! on `POST /release`. Ports `worker/src/index.ts`.
//!
//! All behaviour lives in `inbound` and `release`, written against the traits in
//! `ports` so it runs natively under test. `cloudflare` is the thin workers-rs
//! layer that implements those traits and exists only on wasm32.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod auth_results;
pub mod inbound;
pub mod mime;
pub mod ports;
pub mod release;
pub mod settings;

#[cfg(test)]
mod testing;

#[cfg(target_arch = "wasm32")]
mod cloudflare;

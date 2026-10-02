//! The Postage web front end (Leptos CSR, built with trunk). Replaces the
//! browser half of `web/`.
//!
//! - `config`: build-time client configuration (the `NEXT_PUBLIC_*` values).
//! - `bridge`: typed access to the Privy and World IDKit JS SDKs.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod bridge;
pub mod config;

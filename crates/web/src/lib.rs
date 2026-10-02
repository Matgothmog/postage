//! The Postage web front end (Leptos CSR, built with trunk). Replaces the
//! browser half of `web/`.
//!
//! - `app`: the root component and routes; `pages`, `landing`, `screens` and
//!   `chrome` are the screens and shared shell.
//! - `config`: build-time client configuration (the `NEXT_PUBLIC_*` values).
//! - `bridge`: typed access to the Privy and World IDKit JS SDKs.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod app;
pub mod bridge;
pub mod chrome;
pub mod config;
pub mod landing;
pub mod pages;
pub mod privy_context;
pub mod screens;

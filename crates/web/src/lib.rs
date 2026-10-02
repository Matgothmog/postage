//! The Postage web front end (Leptos CSR, built with trunk). Replaces the
//! browser half of `web/`.
//!
//! - `app`: the root component and routes; `pages`, `landing`, `screens` and
//!   `chrome` are the screens and shared shell.
//! - `account`, `claim_strip`, `inbox_panel`: the signed-in home screen, the
//!   claim flow and the owner's dashboard. `pending_claim` remembers a claim
//!   across reloads, `proof` proves a wallet to the server, `api` and `http`
//!   are the typed `fetch` layer, and `chain` reads the escrow contract.
//! - `challenge_page`, `challenge_actions`, `challenge_api`, `world_id`, `qr`:
//!   the page a stranger gets in a bounce email, with its three ways through
//!   (prove a person, pay, paste the message again).
//! - `network_page`, `network_api`, `network_parts`, `auto_refresh`: the public
//!   ledger at `/network`, which reads `GET /api/network` and re-reads it on a timer.
//! - `config`: build-time client configuration (the `NEXT_PUBLIC_*` values).
//! - `bridge`: typed access to the Privy and World IDKit JS SDKs.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod account;
pub mod api;
pub mod app;
pub mod auto_refresh;
pub mod bridge;
pub mod chain;
pub mod challenge_actions;
pub mod challenge_api;
pub mod challenge_page;
pub mod chrome;
pub mod claim_strip;
pub mod config;
pub mod http;
pub mod inbox_panel;
pub mod landing;
pub mod network_api;
pub mod network_page;
pub mod network_parts;
pub mod pages;
pub mod pending_claim;
pub mod privy_context;
pub mod proof;
pub mod qr;
pub mod screens;
pub mod world_id;

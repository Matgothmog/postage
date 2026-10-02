//! Pure domain logic: no network, database or clock access lives here.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod brand;
pub mod contracts;
pub mod errors;
pub mod format;
pub mod handle;
pub mod network;
pub mod pricing;
pub mod quote_types;
pub mod rp_context;
pub mod secret;
pub mod statements;
pub mod tiers;
pub mod time;
pub mod verification;
pub mod wallet_nonce;
pub mod wallet_proof;

//! Pure domain logic: no network, database or clock access lives here.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod attestation;
pub mod brand;
pub mod challenge_email;
pub mod classify;
pub mod contracts;
pub mod errors;
pub mod format;
pub mod handle;
pub mod js_number;
pub mod mail;
pub mod network;
pub mod pricing;
pub mod privy;
pub mod quote;
pub mod quote_types;
pub mod reputation;
pub mod rp_context;
pub mod rp_signature;
pub mod secret;
pub mod sender_auth;
pub mod signal;
pub mod statements;
pub mod tiers;
pub mod time;
pub mod verification;
pub mod wallet_nonce;
pub mod wallet_proof;
pub mod wallet_signature;
pub mod world_id_messages;

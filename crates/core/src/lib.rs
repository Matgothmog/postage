//! Pure domain logic: no network, database or clock access lives here.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod brand;
pub mod errors;
pub mod format;
pub mod handle;
pub mod tiers;
pub mod time;

//! Wire types shared across the Postage crates.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod release;
mod tier;
mod timing;
mod verdict;

pub use release::{RELEASE_PATH, RELEASE_SECRET_HEADER, ReleaseRequest, ReleaseResponse};
pub use tier::Tier;
pub use timing::{INBOUND_BODY_LIMIT_BYTES, INBOUND_DEADLINE_MS, INBOUND_WORKER_TIMEOUT_MS};
pub use verdict::{GatewayAction, GatewayNotice, GatewayVerdict};

//! The two numbers the mail worker and the gateway must agree on.
//!
//! The server's worst case for `POST /api/mail/inbound`, from the code:
//!
//! - classifier: 20s a try, two tries, 0.5s between them = 40.5s
//! - sender reputation: two subgraph lookups run side by side, 10s each = 10s
//! - price floor read: 10s a try, four tries, 150ms + 300ms + 600ms between
//!   them = 41.05s
//! - database: unbounded on a remote connection, so covered by the deadline
//!
//! The first three add to 91.55s. The deadline sits above that and below the
//! 120s `maxDuration` in `vercel.json`, so it only fires on a stall the
//! per-call limits do not catch. The worker waits longer still, so the
//! gateway's own fault answer reaches it instead of the worker giving up first
//! on a request that goes on to write a challenge the sender's retry repeats.

/// How long the gateway works on one inbound message before answering with
/// its fault response. The challenge row is the last write, after the price
/// is known, so a message that times out earlier leaves none. (The hourly
/// classification budget is claimed before the classifier runs and is not
/// given back.)
pub const INBOUND_DEADLINE_MS: u32 = 100_000;

/// How long the mail worker waits for the gateway: the deadline plus room for
/// the response to cross the network. Cloudflare documents no wall-clock limit
/// on a Worker's subrequests while the client stays connected.
pub const INBOUND_WORKER_TIMEOUT_MS: u32 = 110_000;

/// The largest request body Vercel Functions accept (4.5 MB), which is also
/// the largest inbound message the gateway can be sent.
pub const INBOUND_BODY_LIMIT_BYTES: usize = 4_500_000;

const _: () = assert!(INBOUND_WORKER_TIMEOUT_MS > INBOUND_DEADLINE_MS);
const _: () = assert!(INBOUND_DEADLINE_MS < 120_000);

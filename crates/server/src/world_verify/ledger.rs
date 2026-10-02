//! The `issued_rp_contexts` ledger as this route uses it: spending the context
//! a live proof answers, and taking an attempt slot in mock mode, where there
//! is no context to spend.

use alloy_primitives::{hex, keccak256};
use axum::http::StatusCode;
use postage_core::rp_context::RP_CONTEXT_TTL_SECONDS;
use rand_core::{OsRng, TryRngCore};

use super::{Stopped, describe_failure, token_fingerprint};
use crate::app::AppState;
use crate::db::Db;
use crate::db::challenges::Challenge;
use crate::db::issued_contexts::{
    consume_issued_context, purge_expired_contexts, record_issued_context,
};
use crate::log;

/// Spends the signed context this proof was issued under, and refuses the
/// proof if there is none left to spend.
///
/// This is what makes a signed rp_context single use rather than a bearer
/// credential: a harvested proof, or this one presented twice, finds nothing
/// to consume and never reaches World. Not knowing whether the nonce is still
/// good is not a reason to let the proof through, so a ledger failure fails
/// closed.
pub(crate) async fn spend_issued_context(
    state: &AppState,
    db: &Db,
    token: &str,
    nonce: &str,
) -> Result<(), Stopped> {
    let spent = match consume_issued_context(db, token, nonce, state.now()).await {
        Ok(spent) => spent,
        Err(cause) => {
            log::error(
                "issued rp_context ledger unavailable",
                &[
                    ("tokenRef", &token_fingerprint(token)),
                    ("reason", &describe_failure(state.env(), &cause)),
                ],
            );
            return Err(Stopped::refused(
                StatusCode::BAD_GATEWAY,
                "Could not verify this proof's signing context",
            ));
        }
    };
    if !spent {
        return Err(Stopped::refused(
            StatusCode::BAD_REQUEST,
            "This proof's signing context was already used, or was never issued for this challenge",
        ));
    }
    Ok(())
}

/// Whether this challenge has already been answered, and so whether mock mode
/// must skip the chain write. Read only on the mock branch.
pub(crate) fn is_already_answered(challenge: &Challenge) -> bool {
    challenge.resolved_at.is_some()
}

/// What mock mode has instead of a signed context: without it one challenge
/// token is an unbounded draw on the relayer's vault.
///
/// The same ceiling as live contexts, out of the same table, over the same
/// window. A slot is taken per attempt and never given back, so attempts are
/// counted rather than successes: a failed chain write still cost gas. Keyed
/// on the token, the bearer capability this route is judged against, so only
/// whoever holds it can spend its allowance.
pub(crate) async fn spend_mock_verification_slot(
    state: &AppState,
    db: &Db,
    token: &str,
) -> Result<(), Stopped> {
    let at = state.now();
    if !take_mock_slot(state, db, token, at).await? {
        // The signing route's words for the same ceiling: which of the two
        // refused a sender is not theirs to read.
        return Err(Stopped::refused(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many verification attempts. Wait a moment and try again.",
        ));
    }

    // Only once a slot is taken, so a refused caller cannot drive this DELETE
    // at will. Swallowed: the slot is already committed, and nothing depends
    // on the purge having run.
    if let Err(cause) = purge_expired_contexts(db, at).await {
        log::error(
            "expired rp_context purge failed",
            &[
                ("tokenRef", &token_fingerprint(token)),
                ("reason", &describe_failure(state.env(), &cause)),
            ],
        );
    }
    Ok(())
}

/// Records one attempt; fails closed when the ledger cannot say how much this
/// token has already spent.
async fn take_mock_slot(state: &AppState, db: &Db, token: &str, at: i64) -> Result<bool, Stopped> {
    let expires_at = at.saturating_add(i64::try_from(RP_CONTEXT_TTL_SECONDS).unwrap_or(i64::MAX));
    let taken = match mock_slot_nonce() {
        Ok(nonce) => record_issued_context(db, token, &nonce, at, expires_at, at)
            .await
            .map_err(|cause| describe_failure(state.env(), &cause)),
        Err(reason) => Err(reason),
    };
    taken.map_err(|reason| {
        log::error(
            "verification attempt ledger unavailable",
            &[("tokenRef", &token_fingerprint(token)), ("reason", &reason)],
        );
        Stopped::refused(StatusCode::BAD_GATEWAY, "Could not verify this challenge")
    })
}

/// `mock:` and a random version 4 UUID, so an operator reading the table can
/// tell a slot nothing ever signed from a context that was really issued.
fn mock_slot_nonce() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|error| format!("no randomness for a verification slot: {error}"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let digits = hex::encode(bytes);
    Ok(format!(
        "mock:{}-{}-{}-{}-{}",
        &digits[..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..]
    ))
}

/// Used until Selfie Check is enabled. Keyed on the sender rather than at
/// random, so the same address behaves like the same person and the
/// uniqueness a nullifier provides is still visible.
pub(crate) fn mock_nullifier(sender: &str) -> String {
    let seed = format!("mock-selfie:{}", sender.to_lowercase());
    format!("0x{}", hex::encode(keccak256(seed.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mock_nullifier_is_the_keccak_of_the_lowercased_sender() {
        // keccak256(stringToBytes("mock-selfie:sender@x.com")), from viem.
        assert_eq!(
            mock_nullifier("Sender@X.com"),
            "0x1e8ea1d97f9342836c3a5b8b612e0df42fea8c5e09e5648e6d5bd0adc442380a"
        );
        assert_ne!(mock_nullifier("a@x.com"), mock_nullifier("b@x.com"));
    }

    #[test]
    fn a_mock_slot_nonce_is_prefixed_and_shaped_like_random_uuid() {
        let nonce = mock_slot_nonce().unwrap();
        let uuid = nonce.strip_prefix("mock:").unwrap();
        let groups: Vec<usize> = uuid.split('-').map(str::len).collect();
        assert_eq!(groups, [8, 4, 4, 4, 12]);
        assert_eq!(&uuid[14..15], "4");
        assert!(matches!(&uuid[19..20], "8" | "9" | "a" | "b"), "{uuid}");
        assert_ne!(mock_slot_nonce().unwrap(), nonce);
    }
}

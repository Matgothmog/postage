//! What `/api/world/verify` does once a person is established: attest them
//! onchain, bind their nullifier to the sender, open the gate on the human
//! lane, and answer.
//!
//! The chain write goes first and nothing of ours moves until it has landed,
//! because the two writes are not equally undoable. An attestation that lands
//! and is then abandoned grants nobody anything: `humanUntil` is read by this
//! route and the network page, never by a gate. Binding the nullifier takes it
//! off whoever held it and revokes the passes they earned, so running it first
//! let a failed attestation answer "nothing was saved" over a ledger it had
//! already rewritten. The price of this order is a chain write a ledger
//! failure can orphan, which is the cheap side of the trade.

use alloy_primitives::{Address, B256};
use axum::http::StatusCode;
use axum::response::Response;
use futures_util::future::BoxFuture;
use postage_core::attestation::{CREDENTIAL_LIFETIME_SECONDS, identity_for, to_bytes32};
use serde::Serialize;

use super::attest::{AttestationFailure, record_personhood};
use super::{SettleVerifiedHuman, Stopped, VerifiedHuman, describe_failure, token_fingerprint};
use crate::app::AppState;
use crate::db::nullifiers::{NullifierClaim, claim_nullifier};
use crate::gate::{GateResult, Lane, open_gate};
use crate::log;
use crate::routes::json;

/// The settle step the route runs in production.
#[derive(Debug, Clone, Copy, Default)]
pub struct Personhood;

impl SettleVerifiedHuman for Personhood {
    fn settle<'a>(
        &'a self,
        state: &'a AppState,
        verified: VerifiedHuman,
    ) -> BoxFuture<'a, Result<Response, Stopped>> {
        Box::pin(settle(state, verified))
    }
}

/// The gate's answer with the record behind it, in the TypeScript's order:
/// `{ ...result, identity, nullifierHash, expiresAt }`.
#[derive(Serialize)]
struct Settled {
    #[serde(flatten)]
    outcome: Outcome,
    identity: String,
    #[serde(rename = "nullifierHash")]
    nullifier_hash: String,
    #[serde(rename = "expiresAt")]
    expires_at: i64,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum Outcome {
    Charged { reason: &'static str },
    Cleared { reason: String, delivered: bool },
}

async fn settle(state: &AppState, verified: VerifiedHuman) -> Result<Response, Stopped> {
    let token = verified.token.as_str();
    let nullifier_hash = to_bytes32(&verified.nullifier);
    let identity = identity_for(&nullifier_hash);
    let expires_at = state.now().saturating_add(CREDENTIAL_LIFETIME_SECONDS);

    if verified.attest_on_chain {
        attest(state, token, identity, nullifier_hash, expires_at).await?;
    }

    // Still ahead of the gate: the ledger is the only account of which address
    // holds this person's free lane, and a pass with no binding behind it is
    // the farming the ledger exists to stop.
    bind_nullifier_to_sender(state, nullifier_hash, &verified.challenge.sender, token).await?;

    let db = state.db().await.map_err(Stopped::failed)?;
    let outcome = match open_gate(db, state.mail_worker(), token, Lane::Human, state.now())
        .await
        .map_err(Stopped::failed)?
    {
        GateResult::Unknown => {
            return Err(Stopped::refused(StatusCode::NOT_FOUND, "Unknown challenge"));
        }
        GateResult::Charged => Outcome::Charged {
            reason: "dangerous",
        },
        GateResult::Cleared { reason, delivered } => Outcome::Cleared { reason, delivered },
    };
    Ok(json(
        StatusCode::OK,
        &Settled {
            outcome,
            identity: identity.to_string(),
            nullifier_hash: nullifier_hash.to_string(),
            expires_at,
        },
    ))
}

/// The chain write, answered for when it fails. What went wrong is logged
/// for whoever operates this, never returned: a transport failure can quote
/// a keyed RPC URL.
async fn attest(
    state: &AppState,
    token: &str,
    identity: Address,
    nullifier_hash: B256,
    expires_at: i64,
) -> Result<(), Stopped> {
    let recorded = match u64::try_from(expires_at) {
        Ok(expires_at) => record_personhood(state, identity, nullifier_hash, expires_at).await,
        Err(error) => Err(error.into()),
    };
    let Err(failure) = recorded else {
        return Ok(());
    };
    log::error(
        "world id attestation failed",
        &[
            ("tokenRef", &token_fingerprint(token)),
            ("identity", &identity),
            ("reason", &describe_failure(state.env(), failure.cause())),
        ],
    );
    // Two different truths. A sent transaction whose outcome never came back
    // may still mine, so "nothing was saved" would be a guess; every other
    // failure is before a hash existed or after a confirmed revert, where it
    // is a fact. Both offer another try, and paying, which is ours to offer
    // whatever the sender's World allowance.
    let error = match failure {
        AttestationFailure::OutcomeUnknown(_) => {
            "Could not confirm the attestation in time. It may still complete on its own. Try again, or pay instead"
        }
        AttestationFailure::NotRecorded(_) => {
            "Could not record the attestation. Nothing was saved. Try again, or pay instead"
        }
    };
    Err(Stopped::refused(StatusCode::BAD_GATEWAY, error))
}

/// Ties the person the nullifier names to the sender presenting them, taking
/// the binding off whoever held it before.
///
/// One person, one free lane, and the person decides which address holds it.
/// The sender a proof binds to is the address on the challenge, not whoever
/// took the selfie, so refusing a second sender made a poisoned binding
/// permanent: mail a handle from your own address, pass the link to someone,
/// and their nullifier was spent on you for good. Moving instead hands no
/// farming back, because the same write releases the first address and
/// revokes the passes it earned.
async fn bind_nullifier_to_sender(
    state: &AppState,
    nullifier_hash: B256,
    sender: &str,
    token: &str,
) -> Result<(), Stopped> {
    let nullifier_hash = nullifier_hash.to_string();
    let claimed = match state.db().await {
        Ok(db) => claim_nullifier(db, &nullifier_hash, sender, state.now()).await,
        Err(error) => Err(error),
    };
    let claim = match claimed {
        Ok(claim) => claim,
        Err(cause) => {
            // Fails closed: a lane the ledger never recorded is one nothing can
            // later see or move. The sender is left out of this line on
            // purpose: next to the nullifier hash it is exactly the
            // email-to-pseudonym link the ledger keeps from spreading.
            log::error(
                "nullifier ledger unavailable",
                &[
                    ("tokenRef", &token_fingerprint(token)),
                    ("nullifierHash", &nullifier_hash),
                    ("reason", &describe_failure(state.env(), &cause)),
                ],
            );
            return Err(Stopped::refused(
                StatusCode::BAD_GATEWAY,
                "Could not check this World ID",
            ));
        }
    };

    // Logged, never returned: which address a lane came off is somebody
    // else's, and on the path worth worrying about the one asking is the
    // attacker. A burst of these is a poisoned link being recovered from, or
    // somebody working the policy.
    if let NullifierClaim::Rebound { released_from } = claim {
        log::error(
            "world id nullifier rebound to a new sender",
            &[
                ("released_from", &released_from),
                ("claimed_by", &sender.to_lowercase()),
            ],
        );
    }
    Ok(())
}

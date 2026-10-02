//! `POST /api/world/verify` (`web/src/app/api/world/verify/route.ts`): the
//! free lane's proof of personhood, and what it buys.
//!
//! In the order the TypeScript ran them, each a stage of its own:
//!
//! 1. the body is read: `{ token, proof }` ([`proof::read_request`]);
//! 2. the challenge is looked up, and one no lane will deliver is refused;
//! 3. in live mode, the proof's shape is checked, it must carry exactly one
//!    Selfie Check credential bound to this challenge's token by its
//!    `signal_hash`, the signed context it answers is spent (the replay
//!    guard), and only then is World asked ([`world_answer`]);
//! 4. in mock mode there is no proof: a synthetic per-sender nullifier stands
//!    in, bounded by the same per-token ceiling live contexts share
//!    ([`ledger`]).
//!
//! What follows a verified person is behind [`SettleVerifiedHuman`];
//! [`Personhood`] is the one the route runs: the onchain attestation, then
//! binding the nullifier, then the gate ([`settle`]).
//!
//! Every refusal decided inside stage 3 or 4 is a [`Stopped::Refused`] with
//! the TypeScript's status and words. Anything else that goes wrong there is
//! ours (a missing setting, a bug), logged with a fingerprint of the token
//! and answered as a bare 500, as Next.js answered the rethrown error.

mod attest;
mod ledger;
mod proof;
mod settle;
mod world_answer;

#[cfg(test)]
mod settle_tests;
#[cfg(test)]
mod tests;

pub use settle::Personhood;

use std::error::Error;

use alloy_primitives::{hex, keccak256};
use axum::body::Bytes;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::future::BoxFuture;

use crate::app::AppState;
use crate::config::{Env, IdentityMode, identity_mode};
use crate::db::Db;
use crate::db::challenges::{Challenge, challenge_by_token};
use crate::faults::{BoxError, reason_chain, redact};
use crate::log;
use crate::routes::{Unhandled, refusal};

const DANGEROUS: &str = "dangerous";

/// A person established for a challenge: what the rest of the route acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedHuman {
    /// The challenge token as it was looked up.
    pub token: String,
    pub challenge: Challenge,
    /// The Selfie Check nullifier exactly as World returned it (live), or the
    /// per-sender stand-in (mock). Not yet normalised to 32 bytes.
    pub nullifier: String,
    /// Cleared on the one path that must spend no gas: mock mode answering a
    /// challenge that is already settled. Everything after still runs.
    pub attest_on_chain: bool,
}

/// Why the route stopped before answering with success.
#[derive(Debug)]
pub enum Stopped {
    /// A refusal with a body: `{ "error": error }` at `status`.
    Refused { status: StatusCode, error: String },
    /// Ours, and not for the caller to read: a bare 500, logged.
    Failed(BoxError),
}

impl Stopped {
    pub(crate) fn refused(status: StatusCode, error: &str) -> Self {
        Self::Refused {
            status,
            error: error.to_owned(),
        }
    }

    pub(crate) fn failed(error: impl Into<BoxError>) -> Self {
        Self::Failed(error.into())
    }
}

impl IntoResponse for Stopped {
    fn into_response(self) -> Response {
        match self {
            Self::Refused { status, error } => refusal(status, &error),
            Self::Failed(error) => Unhandled::from_boxed(error).into_response(),
        }
    }
}

/// Everything the route does once a person is established: bind the
/// nullifier, attest onchain, open the gate, and answer.
pub trait SettleVerifiedHuman: Send + Sync {
    fn settle<'a>(
        &'a self,
        state: &'a AppState,
        verified: VerifiedHuman,
    ) -> BoxFuture<'a, Result<Response, Stopped>>;
}

/// The whole route: establish a person, then hand them to `settle`.
pub async fn handle(state: &AppState, body: &Bytes, settle: &dyn SettleVerifiedHuman) -> Response {
    let outcome = match verify_human(state, body).await {
        Ok(verified) => settle.settle(state, verified).await,
        Err(stopped) => Err(stopped),
    };
    outcome.unwrap_or_else(IntoResponse::into_response)
}

/// Stages 1 to 4: from the raw body to a verified person, or the reason not.
pub async fn verify_human(state: &AppState, body: &Bytes) -> Result<VerifiedHuman, Stopped> {
    let request = proof::read_request(body)?;
    let db = state.db().await.map_err(Stopped::failed)?;
    let challenge = open_challenge(db, &request.token).await?;
    let person = establish_person(state, db, &request, &challenge).await?;
    Ok(VerifiedHuman {
        token: request.token,
        challenge,
        nullifier: person.nullifier,
        attest_on_chain: person.attest_on_chain,
    })
}

/// The challenge this token names, unless no lane will ever deliver it.
async fn open_challenge(db: &Db, token: &str) -> Result<Challenge, Stopped> {
    let Some(challenge) = challenge_by_token(db, token)
        .await
        .map_err(Stopped::failed)?
    else {
        return Err(Stopped::refused(StatusCode::NOT_FOUND, "Unknown challenge"));
    };
    // Judged before the replay guard, so an already-answered dangerous token
    // is still refused as dangerous rather than reported as cleared.
    if challenge.tier == DANGEROUS {
        return Err(Stopped::refused(
            StatusCode::FORBIDDEN,
            "This will not be delivered whoever sends it. Being a person does not change that",
        ));
    }
    Ok(challenge)
}

struct Person {
    nullifier: String,
    attest_on_chain: bool,
}

/// The verification block of the TypeScript's `POST`: its refusals pass
/// through, and anything else is logged here, the one place that knows which
/// request it belonged to, before it becomes a bare 500.
async fn establish_person(
    state: &AppState,
    db: &Db,
    request: &proof::VerifyRequest,
    challenge: &Challenge,
) -> Result<Person, Stopped> {
    let outcome = identify(state, db, request, challenge).await;
    if let Err(Stopped::Failed(cause)) = &outcome {
        log::error(
            "world id verification failed unexpectedly",
            &[
                ("tokenRef", &token_fingerprint(&request.token)),
                ("reason", &describe_failure(state.env(), cause.as_ref())),
            ],
        );
    }
    outcome
}

async fn identify(
    state: &AppState,
    db: &Db,
    request: &proof::VerifyRequest,
    challenge: &Challenge,
) -> Result<Person, Stopped> {
    let token = request.token.as_str();
    match identity_mode(state.env().lookup()).map_err(Stopped::failed)? {
        IdentityMode::Live => {
            let presented = proof::require_world_id_proof(request.proof.as_ref())?;
            // Checked before World is called at all, so a proof pasted in from
            // another challenge costs one string comparison, not a round trip.
            proof::require_bound_to_challenge(&presented, token)?;
            // What stops a harvested proof, or this one presented twice, from
            // ever reaching World.
            ledger::spend_issued_context(state, db, token, presented.nonce).await?;
            let nullifier = world_answer::verify_with_world(state, &presented, token).await?;
            Ok(Person {
                nullifier,
                attest_on_chain: true,
            })
        }
        IdentityMode::Mock => {
            // Nothing above runs without a proof, so what bounds the
            // relayer's exposure to one token is decided here. A request that
            // will send nothing spends no slot either.
            let attest_on_chain = !ledger::is_already_answered(challenge);
            if attest_on_chain {
                ledger::spend_mock_verification_slot(state, db, token).await?;
            }
            Ok(Person {
                nullifier: ledger::mock_nullifier(&challenge.sender),
                attest_on_chain,
            })
        }
    }
}

/// A stand-in for the challenge token wherever this route logs: the token is
/// the bearer capability that clears the challenge, so it never reaches a log
/// line, and its hash still ties lines about one request together.
pub(crate) fn token_fingerprint(token: &str) -> String {
    format!("0x{}", hex::encode(keccak256(token.as_bytes())))
}

/// An error and its causes as one line, with this deployment's secrets taken
/// out: an RPC or database failure can quote a keyed URL.
pub(crate) fn describe_failure(env: &Env, cause: &(dyn Error + Send + Sync + 'static)) -> String {
    redact(env, &reason_chain(cause))
}

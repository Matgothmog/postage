//! `recordPersonhood`: the onchain half of the free lane. Writes
//! `HumanRegistry.attest` for the person's identity, signed by the attester
//! and paid for by the relayer, and confirms it mined.
//!
//! Every chain call goes through the one [`Chain`] endpoint: the read before
//! sending, the write, the receipt wait and the read after a revert. A wait
//! that polls a provider which never saw the broadcast reports a mined
//! attestation as an unknown outcome.

use std::error::Error;
use std::fmt;

use alloy_primitives::{Address, B256, Bytes, TxHash};

use crate::app::AppState;
use crate::chain::{ATTESTATION_RECEIPT_TIMEOUT, Chain, ChainError, RECEIPT_POLL_INTERVAL};
use crate::config::{attester_signer, relayer_signer};
use crate::faults::BoxError;
use crate::log;

/// Why no personhood record can be vouched for.
#[derive(Debug)]
pub(crate) enum AttestationFailure {
    /// Nothing is on chain for this attempt: it failed before any hash
    /// existed, or a receipt confirmed the transaction reverted.
    NotRecorded(BoxError),
    /// A transaction was sent and this request never learned how it ended.
    /// It may still mine after the answer goes out.
    OutcomeUnknown(OutcomeUnknown),
}

impl AttestationFailure {
    /// The failure with everything it wraps, for the log.
    pub(crate) fn cause(&self) -> &(dyn Error + Send + Sync + 'static) {
        match self {
            Self::NotRecorded(cause) => cause.as_ref(),
            Self::OutcomeUnknown(unknown) => unknown,
        }
    }
}

impl<E: Into<BoxError>> From<E> for AttestationFailure {
    fn from(error: E) -> Self {
        Self::NotRecorded(error.into())
    }
}

/// The hash an operator reconciles by hand, with what actually went wrong
/// waiting for it as its source: a wait that timed out, a node that went
/// away and a receipt read that errored would otherwise all read the same.
#[derive(Debug)]
pub(crate) struct OutcomeUnknown {
    hash: TxHash,
    cause: ChainError,
}

impl fmt::Display for OutcomeUnknown {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "attestation {} did not confirm before the wait bound",
            self.hash
        )
    }
}

impl Error for OutcomeUnknown {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}

/// A receipt that confirmed a revert this request cannot explain away.
#[derive(Debug, thiserror::Error)]
#[error("attestation transaction {0} reverted")]
struct Reverted(TxHash);

/// Records `identity` as a person until `expires_at`, or says why not.
///
/// Returns early, sending nothing, when the registry already holds a record
/// at least this fresh: attesting again inside the same second reverts as a
/// non-extension, and the record says everything a new one would.
pub(crate) async fn record_personhood(
    state: &AppState,
    identity: Address,
    nullifier_hash: B256,
    expires_at: u64,
) -> Result<(), AttestationFailure> {
    let chain = state.chain()?;
    if chain.human_until(identity).await? >= expires_at {
        return Ok(());
    }

    let env = state.env().lookup();
    let signature = attester_signer(&env)?.sign(identity, nullifier_hash, expires_at)?;
    let relayer = relayer_signer(&env)?;
    let hash = chain
        .attest(
            &relayer,
            identity,
            nullifier_hash,
            expires_at,
            Bytes::from(signature.to_vec()),
        )
        .await?;

    // A receipt is not a success: a reverted transaction is mined too.
    let mined = chain
        .wait_for_receipt(hash, ATTESTATION_RECEIPT_TIMEOUT, RECEIPT_POLL_INTERVAL)
        .await
        .map_err(|cause| AttestationFailure::OutcomeUnknown(OutcomeUnknown { hash, cause }))?;
    if mined {
        return Ok(());
    }
    judge_revert(chain, hash, identity, expires_at).await
}

/// A revert does not always mean nothing was recorded. Two verifications of
/// one nullifier can both pass the pre-send read, both send, and the second
/// to land reverts `NotAnExtension` against the record the first just wrote.
/// That person is recorded, by the race's winner.
///
/// So the registry is read again and judged against `expires_at`, the
/// threshold the pre-send read used and the one `NotAnExtension` is written
/// in. Asking whether any live record exists would also excuse an
/// `InvalidSignature` from a rotated attester key, for everyone already
/// attested. The identity comes from the nullifier this request proved, so
/// finding it fresh can only mean a request presenting that same nullifier
/// got a valid attestation through.
async fn judge_revert(
    chain: &Chain,
    hash: TxHash,
    identity: Address,
    expires_at: u64,
) -> Result<(), AttestationFailure> {
    let settled = chain.human_until(identity).await?;
    let granted_anyway = settled >= expires_at;
    // Logged either way: an attester key that stopped producing valid
    // signatures fails only people with no record yet and succeeds silently
    // for everyone else, and this line is what makes that visible.
    log::error(
        "attestation transaction reverted",
        &[
            ("hash", &hash),
            ("identity", &identity),
            ("humanUntil", &settled),
            ("expiresAt", &expires_at),
            ("grantedAnyway", &granted_anyway),
        ],
    );
    if granted_anyway {
        return Ok(());
    }
    Err(Reverted(hash).into())
}

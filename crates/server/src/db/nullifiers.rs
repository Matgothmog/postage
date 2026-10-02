//! Which sender a World ID nullifier is bound to right now, and every time that
//! binding moved.
//!
//! A nullifier is one person. The free lane grants one distinct human one
//! standing way through, so the ledger holds one sender per nullifier - never
//! two - and that exclusivity is the whole of what stops one person farming a
//! lane per address they own.
//!
//! The binding is exclusive but not permanent, and the difference matters. A
//! proof binds to the sender named on the challenge token, which is whoever
//! mailed the handle rather than whoever took the selfie: anyone can mail a
//! handle from their own address, hand the resulting `/c/<token>` link to a
//! stranger under any pretext, and have that stranger's nullifier written down
//! against the attacker's address. A permanent binding made that theft
//! unrecoverable. Letting the binding move turns it into a nuisance: the person
//! verifies again from their own address and takes it back.
//!
//! Moving it does not reopen farming, because taking a nullifier onto a second
//! address is the same write that takes it off the first - and
//! [`claim_nullifier`] also revokes whatever passes that address earned by
//! proving personhood, in the same transaction as the move.
//!
//! Recording the holder rather than the proof is deliberate. Storing "this
//! exact proof was seen" would refuse a person their own second challenge,
//! which is the thing the credential is supposed to buy them.

use libsql::params;
use serde::Deserialize;

use super::passes::EARNED_PASS_CONDITION;
use super::{Db, DbError};

/// What a claim did to the ledger.
///
/// `released_from` is the sender the nullifier was taken off. It is reported
/// because it is the one thing a caller cannot work out for itself afterwards:
/// by the time the write has returned, the ledger names only the new holder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NullifierClaim {
    Fresh,
    Rebound { released_from: String },
}

impl NullifierClaim {
    pub fn is_rebound(&self) -> bool {
        matches!(self, Self::Rebound { .. })
    }
}

/// One move of a binding from one sender to another.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NullifierRebind {
    pub from_sender: String,
    pub to_sender: String,
    pub at: i64,
}

#[derive(Debug, Deserialize)]
struct ReleasedFrom {
    from_sender: String,
}

#[derive(Debug, Deserialize)]
struct Holder {
    sender: String,
}

/// Binds this nullifier to this sender, taking it off whoever held it before
/// and revoking whatever passes that address earned by proving personhood,
/// and says whether anybody was displaced.
///
/// Three statements rather than one because three tables have to move
/// together, and one transaction rather than three calls because none of them
/// may move without the other two: a rebind that revoked no passes would leave
/// a farmer standing in a lane the ledger claims to have closed, and a
/// revocation with no matching rebind would take a pass from someone the
/// ledger never actually displaced. The first statement is what makes this
/// safe under concurrency: it reads the outgoing holder and writes the hop in
/// the same breath, so the answer it hands back is the holder that this write
/// displaced rather than whoever happened to be there when we last looked. The
/// second statement reads the same pre-move row the first one does, for the
/// same reason: it must fire exactly when a hop is really happening, on the
/// sender the hop is really taking it from, and never on a same-sender
/// re-presentation.
pub async fn claim_nullifier(
    db: &Db,
    nullifier_hash: &str,
    sender: &str,
    now: i64,
) -> Result<NullifierClaim, DbError> {
    let key = nullifier_hash.to_lowercase();
    let claimant = sender.to_lowercase();

    let transaction = db.transaction().await?;

    // Selects from `nullifiers`, so it writes nothing at all when the
    // nullifier is free or already this sender's - a hop is only a hop when the
    // holder actually changes.
    let recorded: Vec<ReleasedFrom> = transaction
        .all(
            "INSERT INTO nullifier_rebinds (nullifier_hash, from_sender, to_sender, at)
              SELECT nullifier_hash, sender, ?, ? FROM nullifiers
              WHERE nullifier_hash = ? AND sender <> ?
              RETURNING from_sender",
            params![claimant.as_str(), now, key.as_str(), claimant.as_str()],
        )
        .await?;

    // Revokes the outgoing holder's earned passes before `nullifiers` itself is
    // rewritten below, so this still sees who the outgoing holder is rather
    // than the claimant the next statement is about to install.
    // `EARNED_PASS_CONDITION` is what keeps a paid pass off this statement's
    // reach no matter what `reason` says, or whether the money only ever
    // stretched an already-open window rather than buying a count.
    transaction
        .run(
            &format!(
                "DELETE FROM passes
              WHERE {EARNED_PASS_CONDITION}
                AND sender IN (
                  SELECT sender FROM nullifiers WHERE nullifier_hash = ? AND sender <> ?
                )"
            ),
            params![key.as_str(), claimant.as_str()],
        )
        .await?;

    // `claimed_at` is when the current holder took it, so presenting again
    // leaves it where it was: nothing was claimed that was not already held,
    // and moving the stamp would erase how long they have had it.
    transaction
        .run(
            "INSERT INTO nullifiers (nullifier_hash, sender, claimed_at) VALUES (?, ?, ?)
              ON CONFLICT (nullifier_hash) DO UPDATE SET
                sender = excluded.sender,
                claimed_at = CASE WHEN nullifiers.sender = excluded.sender
                                  THEN nullifiers.claimed_at ELSE excluded.claimed_at END",
            params![key.as_str(), claimant.as_str(), now],
        )
        .await?;

    transaction.commit().await?;

    Ok(match recorded.into_iter().next() {
        Some(row) => NullifierClaim::Rebound {
            released_from: row.from_sender,
        },
        None => NullifierClaim::Fresh,
    })
}

pub async fn sender_holding_nullifier(
    db: &Db,
    nullifier_hash: &str,
) -> Result<Option<String>, DbError> {
    let rows: Vec<Holder> = db
        .all(
            "SELECT sender FROM nullifiers WHERE nullifier_hash = ?",
            params![nullifier_hash.to_lowercase()],
        )
        .await?;
    Ok(rows.into_iter().next().map(|row| row.sender))
}

/// Every hand this nullifier has passed through, oldest first. An audit trail
/// nothing can read is not one, and this is what answers "was this lane taken
/// from someone, and how many times has it changed hands".
pub async fn rebinds_of(db: &Db, nullifier_hash: &str) -> Result<Vec<NullifierRebind>, DbError> {
    db.all(
        "SELECT from_sender, to_sender, at FROM nullifier_rebinds
     WHERE nullifier_hash = ? ORDER BY id",
        params![nullifier_hash.to_lowercase()],
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use super::*;
    use crate::db::passes::{add_paid_use, grant_pass, has_live_pass};
    use crate::db::testing::TestDb;

    const NOW: i64 = 1_760_000_000;
    const HANDLE: &str = "demo";
    const OTHER_HANDLE: &str = "second-demo";
    const SENDER: &str = "first@x.com";
    const OTHER_SENDER: &str = "second@x.com";

    fn nullifier() -> String {
        format!("0x{}", "ab".repeat(32))
    }

    fn other_nullifier() -> String {
        format!("0x{}", "cd".repeat(32))
    }

    fn rebound(from: &str) -> NullifierClaim {
        NullifierClaim::Rebound {
            released_from: from.to_owned(),
        }
    }

    #[derive(Debug, Deserialize)]
    struct UsesLeft {
        uses_left: Option<i64>,
    }

    async fn uses_left_of(db: &Db, handle: &str, sender: &str) -> Option<i64> {
        let rows: Vec<UsesLeft> = db
            .all(
                "SELECT uses_left FROM passes WHERE handle = ? AND sender = ?",
                params![handle, sender],
            )
            .await
            .unwrap();
        rows.into_iter().next().and_then(|row| row.uses_left)
    }

    /// The hops as "who to whom", which is what every assertion about the trail
    /// is actually about; the timestamp gets its own test.
    async fn hops_of(db: &Db, nullifier_hash: &str) -> Vec<String> {
        rebinds_of(db, nullifier_hash)
            .await
            .unwrap()
            .into_iter()
            .map(|hop| format!("{} -> {}", hop.from_sender, hop.to_sender))
            .collect()
    }

    async fn claim(db: &Db, nullifier_hash: &str, sender: &str) -> NullifierClaim {
        claim_nullifier(db, nullifier_hash, sender, NOW)
            .await
            .unwrap()
    }

    async fn live(db: &Db, handle: &str, sender: &str) -> bool {
        has_live_pass(db, handle, sender, NOW).await.unwrap()
    }

    async fn human_pass(db: &Db, handle: &str, sender: &str) {
        grant_pass(db, handle, sender, "human", None, NOW)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_nullifier_nobody_holds_binds_to_the_sender_presenting_it() {
        let db = TestDb::fresh().await;

        assert_eq!(
            claim(&db, &nullifier(), SENDER).await,
            NullifierClaim::Fresh
        );
        assert_eq!(
            sender_holding_nullifier(&db, &nullifier()).await.unwrap(),
            Some(SENDER.to_owned())
        );
    }

    #[tokio::test]
    async fn the_sender_already_holding_a_nullifier_may_present_it_again_and_nothing_moves() {
        let db = TestDb::fresh().await;
        claim(&db, &nullifier(), SENDER).await;

        assert_eq!(
            claim(&db, &nullifier(), SENDER).await,
            NullifierClaim::Fresh
        );
        assert!(hops_of(&db, &nullifier()).await.is_empty());
    }

    #[tokio::test]
    async fn a_different_sender_presenting_the_same_nullifier_takes_the_binding_over() {
        let db = TestDb::fresh().await;
        claim(&db, &nullifier(), SENDER).await;

        assert_eq!(
            claim(&db, &nullifier(), OTHER_SENDER).await,
            rebound(SENDER)
        );
        assert_eq!(
            sender_holding_nullifier(&db, &nullifier()).await.unwrap(),
            Some(OTHER_SENDER.to_owned())
        );
    }

    /// The poisoning this policy exists for: a proof binds to the sender named
    /// on the challenge token, not whoever took the selfie. Presenting again
    /// from the real holder's own address is the whole of the remedy.
    #[tokio::test]
    async fn a_sender_whose_nullifier_was_spent_under_somebody_elses_address_takes_it_back() {
        let db = TestDb::fresh().await;
        claim(&db, &nullifier(), OTHER_SENDER).await;

        assert_eq!(
            claim(&db, &nullifier(), SENDER).await,
            rebound(OTHER_SENDER)
        );
        assert_eq!(
            sender_holding_nullifier(&db, &nullifier()).await.unwrap(),
            Some(SENDER.to_owned())
        );
    }

    #[tokio::test]
    async fn a_takeover_is_written_down_so_a_moved_binding_is_visible_after_the_fact() {
        let db = TestDb::fresh().await;
        claim(&db, &nullifier(), SENDER).await;
        claim(&db, &nullifier(), OTHER_SENDER).await;

        assert_eq!(
            hops_of(&db, &nullifier()).await,
            vec![format!("{SENDER} -> {OTHER_SENDER}")]
        );
    }

    #[tokio::test]
    async fn every_hop_is_kept_so_a_binding_passed_back_and_forth_reads_as_a_chain() {
        let db = TestDb::fresh().await;
        claim(&db, &nullifier(), SENDER).await;
        claim(&db, &nullifier(), OTHER_SENDER).await;
        claim(&db, &nullifier(), SENDER).await;

        assert_eq!(
            hops_of(&db, &nullifier()).await,
            vec![
                format!("{SENDER} -> {OTHER_SENDER}"),
                format!("{OTHER_SENDER} -> {SENDER}"),
            ]
        );
    }

    #[tokio::test]
    async fn a_hop_is_stamped_with_when_it_happened() {
        let db = TestDb::fresh().await;
        claim_nullifier(&db, &nullifier(), SENDER, NOW)
            .await
            .unwrap();
        claim_nullifier(&db, &nullifier(), OTHER_SENDER, NOW + 42)
            .await
            .unwrap();

        let hops = rebinds_of(&db, &nullifier()).await.unwrap();
        assert_eq!(hops.len(), 1);
        assert_eq!(hops[0].at, NOW + 42);
    }

    #[tokio::test]
    async fn one_persons_nullifier_says_nothing_about_anothers() {
        let db = TestDb::fresh().await;
        claim(&db, &nullifier(), SENDER).await;

        assert_eq!(
            claim(&db, &other_nullifier(), OTHER_SENDER).await,
            NullifierClaim::Fresh
        );
        assert_eq!(
            sender_holding_nullifier(&db, &nullifier()).await.unwrap(),
            Some(SENDER.to_owned())
        );
    }

    #[tokio::test]
    async fn case_is_not_a_second_identity_re_presenting_in_another_casing_moves_nothing() {
        let db = TestDb::fresh().await;
        claim(&db, &nullifier(), SENDER).await;

        let shouted = nullifier().to_uppercase().replace("0X", "0x");
        assert_eq!(
            claim(&db, &shouted, &SENDER.to_uppercase()).await,
            NullifierClaim::Fresh
        );
        assert!(hops_of(&db, &nullifier()).await.is_empty());
    }

    #[tokio::test]
    async fn an_unclaimed_nullifier_is_held_by_nobody_and_has_moved_nowhere() {
        let db = TestDb::fresh().await;

        assert_eq!(
            sender_holding_nullifier(&db, &nullifier()).await.unwrap(),
            None
        );
        assert!(hops_of(&db, &nullifier()).await.is_empty());
    }

    /// Two handles to one file, so the claims genuinely race for the write
    /// lock rather than queueing behind one connection.
    async fn race_claims(
        senders: Vec<String>,
        first: Arc<TestDb>,
        second: Arc<Db>,
    ) -> Vec<NullifierClaim> {
        let racers: Vec<_> = senders
            .into_iter()
            .enumerate()
            .map(|(index, sender)| {
                let first = Arc::clone(&first);
                let second = Arc::clone(&second);
                tokio::spawn(async move {
                    let db: &Db = if index % 2 == 0 { &first } else { &second };
                    claim_nullifier(db, &nullifier(), &sender, NOW).await
                })
            })
            .collect();
        let mut claims = Vec::new();
        for racer in racers {
            claims.push(racer.await.unwrap().unwrap());
        }
        claims
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_senders_racing_for_a_free_nullifier_leave_one_holder_and_one_recorded_move() {
        let first = Arc::new(TestDb::fresh().await);
        let second = Arc::new(first.second_handle().await);

        let claims = race_claims(
            vec![SENDER.to_owned(), OTHER_SENDER.to_owned()],
            Arc::clone(&first),
            second,
        )
        .await;

        // Which of the two ends up holding it is not decided here. What is: one
        // of them created the binding and the other moved it.
        assert_eq!(claims.iter().filter(|claim| claim.is_rebound()).count(), 1);
        let hops = rebinds_of(&first, &nullifier()).await.unwrap();
        assert_eq!(hops.len(), 1);
        assert_eq!(
            Some(hops[0].to_sender.clone()),
            sender_holding_nullifier(&first, &nullifier())
                .await
                .unwrap()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn forty_senders_racing_for_one_nullifier_leave_an_unbroken_chain_ending_at_the_holder() {
        let first = Arc::new(TestDb::fresh().await);
        let second = Arc::new(first.second_handle().await);
        let senders: Vec<String> = (0..40)
            .map(|index| format!("racer-{index}@x.com"))
            .collect();

        let claims = race_claims(senders.clone(), Arc::clone(&first), second).await;

        // Exactly one claim finds the nullifier free; every other one must
        // displace precisely the sender the claim before it installed.
        assert_eq!(
            claims.iter().filter(|claim| claim.is_rebound()).count(),
            senders.len() - 1
        );
        let hops = rebinds_of(&first, &nullifier()).await.unwrap();
        assert_eq!(hops.len(), senders.len() - 1);
        for index in 1..hops.len() {
            assert_eq!(
                hops[index].from_sender,
                hops[index - 1].to_sender,
                "hop {index} does not follow hop {}",
                index - 1
            );
        }
        assert_eq!(
            hops.last().map(|hop| hop.to_sender.clone()),
            sender_holding_nullifier(&first, &nullifier())
                .await
                .unwrap()
        );
        let takers: HashSet<_> = hops.iter().map(|hop| hop.to_sender.as_str()).collect();
        assert_eq!(takers.len(), hops.len(), "a sender took the binding twice");
    }

    #[tokio::test]
    async fn a_rebind_revokes_the_released_senders_earned_pass() {
        let db = TestDb::fresh().await;
        human_pass(&db, HANDLE, SENDER).await;
        claim(&db, &nullifier(), SENDER).await;

        claim(&db, &nullifier(), OTHER_SENDER).await;

        assert!(!live(&db, HANDLE, SENDER).await);
    }

    #[tokio::test]
    async fn a_rebind_never_touches_the_released_senders_paid_pass() {
        let db = TestDb::fresh().await;
        add_paid_use(&db, HANDLE, SENDER, NOW).await.unwrap();
        claim(&db, &nullifier(), SENDER).await;

        claim(&db, &nullifier(), OTHER_SENDER).await;

        assert!(live(&db, HANDLE, SENDER).await);
        assert_eq!(uses_left_of(&db, HANDLE, SENDER).await, Some(1));
    }

    /// The money bug: `add_paid_use` against a row that already reads
    /// `uses_left IS NULL` only pushes `expires_at` out. Before
    /// `paid_extended_at` existed that row was indistinguishable from one
    /// purely earned, and the rebind's DELETE took it along.
    #[tokio::test]
    async fn a_rebind_never_touches_an_unlimited_window_that_payment_has_extended() {
        let db = TestDb::fresh().await;
        human_pass(&db, HANDLE, SENDER).await;
        add_paid_use(&db, HANDLE, SENDER, NOW).await.unwrap();
        claim(&db, &nullifier(), SENDER).await;

        claim(&db, &nullifier(), OTHER_SENDER).await;

        assert!(live(&db, HANDLE, SENDER).await);
    }

    #[tokio::test]
    async fn a_rebind_never_touches_a_paid_balance_even_after_its_reason_is_overwritten_to_human() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "paid", Some(3), NOW)
            .await
            .unwrap();
        human_pass(&db, HANDLE, SENDER).await;
        assert_eq!(
            uses_left_of(&db, HANDLE, SENDER).await,
            Some(3),
            "test setup: the paid count must have carried over"
        );
        claim(&db, &nullifier(), SENDER).await;

        claim(&db, &nullifier(), OTHER_SENDER).await;

        assert!(live(&db, HANDLE, SENDER).await);
        assert_eq!(uses_left_of(&db, HANDLE, SENDER).await, Some(3));
    }

    #[tokio::test]
    async fn a_rebind_revokes_the_released_senders_earned_passes_across_every_handle_not_just_one()
    {
        let db = TestDb::fresh().await;
        human_pass(&db, HANDLE, SENDER).await;
        human_pass(&db, OTHER_HANDLE, SENDER).await;
        claim(&db, &nullifier(), SENDER).await;

        claim(&db, &nullifier(), OTHER_SENDER).await;

        assert!(!live(&db, HANDLE, SENDER).await);
        assert!(!live(&db, OTHER_HANDLE, SENDER).await);
    }

    #[tokio::test]
    async fn a_same_sender_re_claim_revokes_nothing() {
        let db = TestDb::fresh().await;
        human_pass(&db, HANDLE, SENDER).await;
        claim(&db, &nullifier(), SENDER).await;

        claim(&db, &nullifier(), SENDER).await;

        assert!(live(&db, HANDLE, SENDER).await);
    }

    #[tokio::test]
    async fn a_claim_on_an_unrelated_nullifier_does_not_revoke_a_senders_pass() {
        let db = TestDb::fresh().await;
        human_pass(&db, HANDLE, SENDER).await;

        claim(&db, &other_nullifier(), OTHER_SENDER).await;

        assert!(live(&db, HANDLE, SENDER).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_senders_racing_for_a_free_nullifier_revoke_exactly_the_one_who_loses_it() {
        let first = Arc::new(TestDb::fresh().await);
        let second = Arc::new(first.second_handle().await);
        human_pass(&first, HANDLE, SENDER).await;
        human_pass(&first, HANDLE, OTHER_SENDER).await;

        // Whichever sender does NOT end up holding the nullifier must have had
        // their pass revoked, and the one who does must still have theirs.
        let claims = race_claims(
            vec![SENDER.to_owned(), OTHER_SENDER.to_owned()],
            Arc::clone(&first),
            second,
        )
        .await;

        assert_eq!(
            claims.iter().filter(|claim| claim.is_rebound()).count(),
            1,
            "exactly one of the two claims displaces the other"
        );
        let holder = sender_holding_nullifier(&first, &nullifier())
            .await
            .unwrap()
            .unwrap();
        let displaced = if holder == SENDER {
            OTHER_SENDER
        } else {
            SENDER
        };
        assert!(
            live(&first, HANDLE, &holder).await,
            "the winner's own pass must survive, untouched"
        );
        assert!(
            !live(&first, HANDLE, displaced).await,
            "the loser's pass must be revoked, not merely left be"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn forty_senders_racing_for_one_nullifier_leave_passes_revoked_everywhere_but_the_final_holder()
     {
        let first = Arc::new(TestDb::fresh().await);
        let second = Arc::new(first.second_handle().await);
        let senders: Vec<String> = (0..40)
            .map(|index| format!("pass-racer-{index}@x.com"))
            .collect();
        for sender in &senders {
            human_pass(&first, HANDLE, sender).await;
        }

        race_claims(senders.clone(), Arc::clone(&first), second).await;

        let holder = sender_holding_nullifier(&first, &nullifier())
            .await
            .unwrap()
            .unwrap();
        for sender in &senders {
            assert_eq!(
                live(&first, HANDLE, sender).await,
                *sender == holder,
                "{sender} should hold a live pass only if they are the final holder ({holder})"
            );
        }
    }
}

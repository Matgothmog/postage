//! Passes: the standing permission for a sender to reach a handle, either a
//! counted delivery bought by paying or a timed window earned by proving
//! personhood.
//!
//! Every function takes `now` (epoch seconds) from its caller; nothing here
//! reads a clock.

use libsql::params;
use serde::Deserialize;

use super::{Db, DbError};

/// How long proving personhood keeps the gate open. Long enough to send the
/// message that was just refused, short enough that the proof is about now.
pub const PASS_WINDOW_SECONDS: i64 = 15 * 60;

/// Pushes a pass's expiry back out, but only once it is nearly gone. Used when
/// the gate is willing and something on our side is not, so an outage cannot
/// quietly run out the clock on someone who has already paid - while polling
/// cannot hold a window open forever either.
const EXTEND_WHEN_UNDER_SECONDS: i64 = 5 * 60;

/// The furthest a pass may be carried past when it was earned. An outage should
/// not cost someone the delivery they paid for, but a window that renews on
/// demand is not a window - the proof behind it described one moment, and this
/// is how long that moment is allowed to be stretched.
const EXTEND_NO_LATER_THAN_SECONDS: i64 = 60 * 60;

/// What marks a pass as earned by proving personhood rather than ever having
/// taken money. Two columns, not one - either alone can lie.
///
/// `reason` cannot be trusted at all: [`grant_pass`]'s own `ON CONFLICT` keeps
/// whatever `uses_left` a row already had whenever it is positive, even while
/// overwriting `reason` to `"human"` on top of it, so a row that reads
/// `reason = 'human'` can still be sitting on a paid, unspent balance.
///
/// `uses_left IS NULL` alone is not enough either, for the mirror-image
/// reason: [`add_paid_use`]'s unlimited-window branch pays for a window that is
/// already better than a count by pushing `expires_at` out and nothing else.
/// That row now holds a payment and still reads `uses_left IS NULL`.
/// `paid_extended_at` is the thing that says otherwise: stamped the moment
/// that branch runs, never cleared once set.
///
/// Public as text because `nullifiers::claim_nullifier` needs this exact
/// condition inside its own transaction; keeping both call sites on one
/// constant is what stops them drifting into different definitions of
/// "earned".
pub const EARNED_PASS_CONDITION: &str = "uses_left IS NULL AND paid_extended_at IS NULL";

/// A live pass as it stood when it was read. For a counted pass `uses_left` is
/// the count *before* the delivery being spent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Pass {
    pub reason: String,
    pub expires_at: i64,
    pub uses_left: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct Present {}

/// Takes one delivery from a live pass. A pass bought by paying carries a
/// single use and is spent here; one earned by proving personhood carries none,
/// and lasts until it expires. `counted_only` ignores the uncounted kind.
pub async fn spend_pass(
    db: &Db,
    handle: &str,
    sender: &str,
    counted_only: bool,
    now: i64,
) -> Result<Option<Pass>, DbError> {
    let handle = handle.to_lowercase();
    let sender = sender.to_lowercase();
    let rows: Vec<Pass> = db
        .all(
            "SELECT reason, expires_at, uses_left FROM passes
     WHERE handle = ? AND sender = ? AND expires_at > ?
       AND (uses_left > 0 OR (uses_left IS NULL AND ? = 0))",
            params![
                handle.as_str(),
                sender.as_str(),
                now,
                if counted_only { 1_i64 } else { 0_i64 }
            ],
        )
        .await?;
    let Some(pass) = rows.into_iter().next() else {
        return Ok(None);
    };
    if pass.uses_left.is_none() {
        return Ok(Some(pass));
    }

    // Spent by a conditional update, so two messages arriving together cannot
    // both read one remaining use and both be delivered.
    let spent = db
        .run(
            "UPDATE passes SET uses_left = uses_left - 1
          WHERE handle = ? AND sender = ? AND uses_left > 0 AND expires_at > ?",
            params![handle.as_str(), sender.as_str(), now],
        )
        .await?;
    Ok((spent > 0).then_some(pass))
}

pub async fn grant_pass(
    db: &Db,
    handle: &str,
    sender: &str,
    reason: &str,
    uses_left: Option<i64>,
    now: i64,
) -> Result<(), DbError> {
    db.run(
        "INSERT INTO passes (handle, sender, reason, expires_at, uses_left, created_at)
     VALUES (?, ?, ?, ?, ?, ?)
     ON CONFLICT (handle, sender) DO UPDATE SET
       reason = excluded.reason,
       expires_at = MAX(passes.expires_at, excluded.expires_at),
       -- Never takes away a delivery already bought. An unlimited window is
       -- more permissive than a count while it lasts, but overwriting the count
       -- with it means the payment is gone the moment the window lapses.
       uses_left = CASE
         WHEN passes.uses_left IS NOT NULL AND passes.uses_left > 0 THEN passes.uses_left
         ELSE excluded.uses_left
       END,
       created_at = excluded.created_at",
        params![
            handle.to_lowercase(),
            sender.to_lowercase(),
            reason,
            now + PASS_WINDOW_SECONDS,
            uses_left,
            now
        ],
    )
    .await?;
    Ok(())
}

/// Gives back a use that was taken for a delivery that never happened.
///
/// Only restores a use that was actually spent. A blind increment would land on
/// whatever pass holds that row by the time it ran, so a sender who cleared a
/// second challenge while a relay was still in flight would be handed a
/// delivery nobody paid for.
pub async fn refund_pass(db: &Db, handle: &str, sender: &str, now: i64) -> Result<(), DbError> {
    db.run(
        "UPDATE passes SET uses_left = uses_left + 1
     WHERE handle = ? AND sender = ? AND uses_left IS NOT NULL AND expires_at > ?",
        params![handle.to_lowercase(), sender.to_lowercase(), now],
    )
    .await?;
    Ok(())
}

pub async fn extend_pass_if_expiring(
    db: &Db,
    handle: &str,
    sender: &str,
    now: i64,
) -> Result<(), DbError> {
    db.run(
        "UPDATE passes SET expires_at = ?
     WHERE handle = ? AND sender = ? AND expires_at > ? AND expires_at < ?
       AND created_at > ?",
        params![
            now + PASS_WINDOW_SECONDS,
            handle.to_lowercase(),
            sender.to_lowercase(),
            now,
            now + EXTEND_WHEN_UNDER_SECONDS,
            now - EXTEND_NO_LATER_THAN_SECONDS
        ],
    )
    .await?;
    Ok(())
}

/// Adds one paid delivery. Never reduces what is already there: an unlimited
/// window earned by proving personhood is left alone, a counted pass gains a
/// use - even one whose window has lapsed, which comes back fresh with the
/// unspent uses plus this one - and a sender with neither gets one. Overwriting instead would let a
/// second payment land on a pass that already had a use and buy nothing.
///
/// The unlimited-window branch also stamps `paid_extended_at`. There is no
/// count to add a use to there, so the payment cannot show up as anything but
/// a later `expires_at` - leaving `uses_left` NULL, same as a window nobody
/// ever paid for. `paid_extended_at` is the only record that money touched
/// this row; [`EARNED_PASS_CONDITION`] reads it for exactly that reason.
pub async fn add_paid_use(db: &Db, handle: &str, sender: &str, now: i64) -> Result<(), DbError> {
    let handle_key = handle.to_lowercase();
    let sender_key = sender.to_lowercase();
    let topped = db
        .run(
            "UPDATE passes SET
            uses_left = uses_left + 1,
            expires_at = ?,
            reason = CASE WHEN expires_at > ? THEN reason ELSE 'paid' END,
            created_at = CASE WHEN expires_at > ? THEN created_at ELSE ? END
          WHERE handle = ? AND sender = ? AND uses_left IS NOT NULL
            AND (expires_at > ? OR uses_left > 0)",
            params![
                now + PASS_WINDOW_SECONDS,
                now,
                now,
                now,
                handle_key.as_str(),
                sender_key.as_str(),
                now
            ],
        )
        .await?;
    if topped > 0 {
        return Ok(());
    }

    // An unlimited window is already better than a use, so it is not replaced.
    // It is pushed out instead, because the payment has to buy something:
    // without this a sender who paid a minute before their free window lapsed
    // would be left holding nothing for money that has already left their
    // wallet.
    let extended = db
        .run(
            "UPDATE passes SET expires_at = ?, paid_extended_at = ?
          WHERE handle = ? AND sender = ? AND expires_at > ? AND uses_left IS NULL",
            params![
                now + PASS_WINDOW_SECONDS,
                now,
                handle_key.as_str(),
                sender_key.as_str(),
                now
            ],
        )
        .await?;
    if extended > 0 {
        return Ok(());
    }

    grant_pass(db, handle, sender, "paid", Some(1), now).await
}

/// Whether a usable pass exists, without spending it.
pub async fn has_live_pass(db: &Db, handle: &str, sender: &str, now: i64) -> Result<bool, DbError> {
    let rows: Vec<Present> = db
        .all(
            "SELECT 1 FROM passes
     WHERE handle = ? AND sender = ? AND expires_at > ? AND (uses_left IS NULL OR uses_left > 0)",
            params![handle.to_lowercase(), sender.to_lowercase(), now],
        )
        .await?;
    Ok(!rows.is_empty())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::testing::TestDb;

    const NOW: i64 = 1_760_000_000;
    const HANDLE: &str = "demo";
    const SENDER: &str = "farmer@x.com";

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

    #[derive(Debug, PartialEq, Eq, Deserialize)]
    struct StoredPass {
        reason: String,
        expires_at: i64,
        uses_left: Option<i64>,
        created_at: i64,
    }

    async fn stored_pass(db: &Db) -> StoredPass {
        let rows: Vec<StoredPass> = db
            .all(
                "SELECT reason, expires_at, uses_left, created_at FROM passes
                 WHERE handle = ? AND sender = ?",
                params![HANDLE, SENDER],
            )
            .await
            .unwrap();
        rows.into_iter().next().expect("the pass row must exist")
    }

    const AFTER_EXPIRY: i64 = NOW + PASS_WINDOW_SECONDS + 100;

    async fn matches_earned_condition(db: &Db, handle: &str, sender: &str) -> bool {
        let rows: Vec<Present> = db
            .all(
                &format!(
                    "SELECT 1 FROM passes WHERE handle = ? AND sender = ? AND {EARNED_PASS_CONDITION}"
                ),
                params![handle, sender],
            )
            .await
            .unwrap();
        !rows.is_empty()
    }

    #[tokio::test]
    async fn a_pass_earned_by_proving_personhood_alone_matches_earned_pass_condition() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "human", None, NOW)
            .await
            .unwrap();

        assert!(matches_earned_condition(&db, HANDLE, SENDER).await);
    }

    #[tokio::test]
    async fn a_freshly_bought_pass_never_matches_earned_pass_condition() {
        let db = TestDb::fresh().await;
        add_paid_use(&db, HANDLE, SENDER, NOW).await.unwrap();

        assert!(!matches_earned_condition(&db, HANDLE, SENDER).await);
    }

    /// The case the whole charge exists to get right: the unlimited-window
    /// branch of `add_paid_use` pushes `expires_at` out and nothing else, so
    /// `uses_left` stays NULL on a row money has touched.
    #[tokio::test]
    async fn a_window_extended_by_payment_stops_matching_earned_pass_condition_though_uses_left_stays_null()
     {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "human", None, NOW)
            .await
            .unwrap();

        add_paid_use(&db, HANDLE, SENDER, NOW).await.unwrap();

        assert_eq!(
            uses_left_of(&db, HANDLE, SENDER).await,
            None,
            "test setup: the window must still read as uncounted"
        );
        assert!(!matches_earned_condition(&db, HANDLE, SENDER).await);
    }

    /// `grant_pass`'s own `ON CONFLICT` keeps a positive `uses_left` even while
    /// overwriting `reason` to `"human"` on top of it.
    #[tokio::test]
    async fn a_paid_balance_survives_even_under_a_pass_whose_reason_now_reads_human() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "paid", Some(3), NOW)
            .await
            .unwrap();
        grant_pass(&db, HANDLE, SENDER, "human", None, NOW)
            .await
            .unwrap();

        assert_eq!(
            uses_left_of(&db, HANDLE, SENDER).await,
            Some(3),
            "test setup: the paid count must have carried over"
        );
        assert!(!matches_earned_condition(&db, HANDLE, SENDER).await);
    }

    // Behaviour the TypeScript had no test for but relies on.

    #[tokio::test]
    async fn spending_a_counted_pass_takes_its_one_use_and_then_refuses() {
        let db = TestDb::fresh().await;
        add_paid_use(&db, HANDLE, SENDER, NOW).await.unwrap();

        let first = spend_pass(&db, HANDLE, SENDER, false, NOW).await.unwrap();
        let second = spend_pass(&db, HANDLE, SENDER, false, NOW).await.unwrap();

        assert_eq!(first.map(|pass| pass.uses_left), Some(Some(1)));
        assert_eq!(second, None);
    }

    #[tokio::test]
    async fn counted_only_ignores_an_uncounted_window() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "human", None, NOW)
            .await
            .unwrap();

        assert_eq!(
            spend_pass(&db, HANDLE, SENDER, true, NOW).await.unwrap(),
            None
        );
        assert!(
            spend_pass(&db, HANDLE, SENDER, false, NOW)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn refund_restores_a_spent_use_and_extend_respects_its_ceiling() {
        let db = TestDb::fresh().await;
        add_paid_use(&db, HANDLE, SENDER, NOW).await.unwrap();
        spend_pass(&db, HANDLE, SENDER, false, NOW).await.unwrap();

        refund_pass(&db, HANDLE, SENDER, NOW).await.unwrap();
        assert_eq!(uses_left_of(&db, HANDLE, SENDER).await, Some(1));

        // Nearly gone and recently earned: carried forward.
        let late = NOW + PASS_WINDOW_SECONDS - 60;
        extend_pass_if_expiring(&db, HANDLE, SENDER, late)
            .await
            .unwrap();
        assert!(
            has_live_pass(&db, HANDLE, SENDER, late + PASS_WINDOW_SECONDS - 120)
                .await
                .unwrap()
        );
        // Earned over an hour ago: not carried any further.
        let much_later = NOW + EXTEND_NO_LATER_THAN_SECONDS + PASS_WINDOW_SECONDS;
        extend_pass_if_expiring(&db, HANDLE, SENDER, much_later)
            .await
            .unwrap();
        assert!(
            !has_live_pass(&db, HANDLE, SENDER, much_later + 1)
                .await
                .unwrap()
        );
    }

    /// Two handles to one file, both spending the single use of one pass: the
    /// conditional update must let exactly one of them through.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_concurrent_spends_of_one_pass_deliver_exactly_once() {
        let first = Arc::new(TestDb::fresh().await);
        let second = Arc::new(first.second_handle().await);
        add_paid_use(&first, HANDLE, SENDER, NOW).await.unwrap();

        let racers: Vec<_> = (0..8)
            .map(|index| {
                let first = Arc::clone(&first);
                let second = Arc::clone(&second);
                tokio::spawn(async move {
                    if index % 2 == 0 {
                        spend_pass(&first, HANDLE, SENDER, false, NOW).await
                    } else {
                        spend_pass(&second, HANDLE, SENDER, false, NOW).await
                    }
                })
            })
            .collect();
        let mut delivered = 0;
        for racer in racers {
            if racer.await.unwrap().unwrap().is_some() {
                delivered += 1;
            }
        }

        assert_eq!(delivered, 1);
        assert_eq!(uses_left_of(&first, HANDLE, SENDER).await, Some(0));
    }

    #[tokio::test]
    async fn a_payment_on_an_expired_pass_with_uses_left_adds_one_use_in_a_fresh_window() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "paid", Some(3), NOW)
            .await
            .unwrap();

        add_paid_use(&db, HANDLE, SENDER, AFTER_EXPIRY)
            .await
            .unwrap();

        assert_eq!(
            stored_pass(&db).await,
            StoredPass {
                reason: "paid".to_owned(),
                expires_at: AFTER_EXPIRY + PASS_WINDOW_SECONDS,
                uses_left: Some(4),
                created_at: AFTER_EXPIRY,
            }
        );
    }

    #[tokio::test]
    async fn a_payment_on_a_live_counted_pass_adds_one_use_and_renews_the_window() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "paid", Some(2), NOW)
            .await
            .unwrap();

        add_paid_use(&db, HANDLE, SENDER, NOW + 10).await.unwrap();

        assert_eq!(
            stored_pass(&db).await,
            StoredPass {
                reason: "paid".to_owned(),
                expires_at: NOW + 10 + PASS_WINDOW_SECONDS,
                uses_left: Some(3),
                created_at: NOW,
            }
        );
    }

    #[tokio::test]
    async fn a_payment_with_no_pass_creates_one_with_a_single_use() {
        let db = TestDb::fresh().await;

        add_paid_use(&db, HANDLE, SENDER, NOW).await.unwrap();

        assert_eq!(
            stored_pass(&db).await,
            StoredPass {
                reason: "paid".to_owned(),
                expires_at: NOW + PASS_WINDOW_SECONDS,
                uses_left: Some(1),
                created_at: NOW,
            }
        );
    }

    #[tokio::test]
    async fn a_payment_on_a_live_unlimited_window_extends_it_and_leaves_uses_left_null() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "human", None, NOW)
            .await
            .unwrap();

        add_paid_use(&db, HANDLE, SENDER, NOW + 10).await.unwrap();

        assert_eq!(
            stored_pass(&db).await,
            StoredPass {
                reason: "human".to_owned(),
                expires_at: NOW + 10 + PASS_WINDOW_SECONDS,
                uses_left: None,
                created_at: NOW,
            }
        );
    }

    #[tokio::test]
    async fn a_payment_on_an_expired_pass_with_no_uses_left_grants_a_single_use() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "paid", Some(1), NOW)
            .await
            .unwrap();
        spend_pass(&db, HANDLE, SENDER, false, NOW).await.unwrap();

        add_paid_use(&db, HANDLE, SENDER, AFTER_EXPIRY)
            .await
            .unwrap();

        assert_eq!(
            stored_pass(&db).await,
            StoredPass {
                reason: "paid".to_owned(),
                expires_at: AFTER_EXPIRY + PASS_WINDOW_SECONDS,
                uses_left: Some(1),
                created_at: AFTER_EXPIRY,
            }
        );
    }

    #[tokio::test]
    async fn a_payment_on_an_expired_unlimited_window_grants_a_single_use() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "human", None, NOW)
            .await
            .unwrap();

        add_paid_use(&db, HANDLE, SENDER, AFTER_EXPIRY)
            .await
            .unwrap();

        assert_eq!(
            stored_pass(&db).await,
            StoredPass {
                reason: "paid".to_owned(),
                expires_at: AFTER_EXPIRY + PASS_WINDOW_SECONDS,
                uses_left: Some(1),
                created_at: AFTER_EXPIRY,
            }
        );
    }

    /// Two payments arriving together on a lapsed pass must both count: the
    /// first renews the window, the second lands on the renewed pass.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_concurrent_payments_on_an_expired_pass_add_two_uses() {
        let first = Arc::new(TestDb::fresh().await);
        let second = Arc::new(first.second_handle().await);
        grant_pass(&first, HANDLE, SENDER, "paid", Some(3), NOW)
            .await
            .unwrap();

        let racers: Vec<_> = (0..2)
            .map(|index| {
                let first = Arc::clone(&first);
                let second = Arc::clone(&second);
                tokio::spawn(async move {
                    if index == 0 {
                        add_paid_use(&first, HANDLE, SENDER, AFTER_EXPIRY).await
                    } else {
                        add_paid_use(&second, HANDLE, SENDER, AFTER_EXPIRY).await
                    }
                })
            })
            .collect();
        for racer in racers {
            racer.await.unwrap().unwrap();
        }

        assert_eq!(uses_left_of(&first, HANDLE, SENDER).await, Some(5));
    }
}

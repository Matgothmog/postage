//! The wallet nonces that have already been answered, which is what turns a
//! signature into a single-use credential rather than one good for as long as
//! its timestamp stays fresh.
//!
//! Kept in the database rather than in memory because that answer has to
//! survive the request that produced it: the replay may well land on an
//! instance that has never seen the original.
//!
//! Nothing here identifies anyone. A nonce is a server-minted random value and
//! its own MAC, spent by the time it is written, and it names no wallet.

use libsql::params;

use super::{Db, DbError};

/// Records that this nonce has now been answered, and says whether this caller
/// is the one who got to answer it.
///
/// The condition and the write are one statement, so two requests racing with
/// the same captured signature cannot both come away believing they spent it -
/// `INSERT OR IGNORE` leaves the primary key to decide, and only one insert
/// affects a row.
pub async fn spend_wallet_nonce(db: &Db, nonce: &str, expires_at: i64) -> Result<bool, DbError> {
    let spent = db
        .run(
            "INSERT OR IGNORE INTO spent_wallet_nonces (nonce, expires_at) VALUES (?, ?)",
            params![nonce, expires_at],
        )
        .await?;
    Ok(spent > 0)
}

/// Drops nonces whose window has closed, rather than leaving a row per sign-in
/// forever.
///
/// Called from the spend, not from the route that mints: minting a wallet
/// nonce touches no database at all and must keep not touching one, or an open
/// endpoint gains a `DELETE` anyone can call as often as they like.
///
/// Nothing depends on it having run: an expired nonce is refused on the MAC's
/// own timestamp before this table is ever consulted.
pub async fn purge_spent_wallet_nonces(db: &Db, now: i64) -> Result<(), DbError> {
    db.run(
        "DELETE FROM spent_wallet_nonces WHERE expires_at <= ?",
        params![now],
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::testing::TestDb;

    const NOW: i64 = 1_760_000_000;
    const LIVE_FOR: i64 = 120;

    fn nonce(n: u8) -> String {
        format!(
            "{}.{}.{}",
            NOW + LIVE_FOR,
            format!("{n:02}").repeat(16),
            "cd".repeat(32)
        )
    }

    #[tokio::test]
    async fn a_nonce_nobody_has_answered_yet_is_spendable() {
        let db = TestDb::fresh().await;

        assert!(
            spend_wallet_nonce(&db, &nonce(1), NOW + LIVE_FOR)
                .await
                .unwrap()
        );
    }

    /// The whole point of the table. The second presentation of one signature
    /// carries the same nonce, and finds it gone.
    #[tokio::test]
    async fn the_same_nonce_cannot_be_spent_twice() {
        let db = TestDb::fresh().await;
        let offered = nonce(1);

        assert!(
            spend_wallet_nonce(&db, &offered, NOW + LIVE_FOR)
                .await
                .unwrap()
        );
        assert!(
            !spend_wallet_nonce(&db, &offered, NOW + LIVE_FOR)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn spending_one_nonce_leaves_another_alone() {
        let db = TestDb::fresh().await;
        spend_wallet_nonce(&db, &nonce(1), NOW + LIVE_FOR)
            .await
            .unwrap();

        assert!(
            spend_wallet_nonce(&db, &nonce(2), NOW + LIVE_FOR)
                .await
                .unwrap()
        );
    }

    /// Only the rows past their window go. A purge that took a live one would
    /// hand a captured signature a second life.
    #[tokio::test]
    async fn the_purge_drops_nonces_whose_window_has_closed_and_keeps_the_rest() {
        let db = TestDb::fresh().await;
        let (stale, live) = (nonce(1), nonce(2));
        spend_wallet_nonce(&db, &stale, NOW - 1).await.unwrap();
        spend_wallet_nonce(&db, &live, NOW + LIVE_FOR)
            .await
            .unwrap();

        purge_spent_wallet_nonces(&db, NOW).await.unwrap();

        assert!(
            spend_wallet_nonce(&db, &stale, NOW + LIVE_FOR)
                .await
                .unwrap(),
            "the stale row survived"
        );
        assert!(
            !spend_wallet_nonce(&db, &live, NOW + LIVE_FOR)
                .await
                .unwrap(),
            "a live row was dropped"
        );
    }

    /// Exactly the boundary, because `<=` and `<` differ by one second here.
    #[tokio::test]
    async fn a_nonce_expiring_exactly_now_is_dropped_by_the_purge() {
        let db = TestDb::fresh().await;
        let offered = nonce(1);
        spend_wallet_nonce(&db, &offered, NOW).await.unwrap();

        purge_spent_wallet_nonces(&db, NOW).await.unwrap();

        assert!(
            spend_wallet_nonce(&db, &offered, NOW + LIVE_FOR)
                .await
                .unwrap()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_spends_of_one_nonce_through_two_handles_succeed_exactly_once() {
        let first = Arc::new(TestDb::fresh().await);
        let second = Arc::new(first.second_handle().await);
        let offered = Arc::new(nonce(7));

        let racers: Vec<_> = (0..8)
            .map(|index| {
                let first = Arc::clone(&first);
                let second = Arc::clone(&second);
                let offered = Arc::clone(&offered);
                tokio::spawn(async move {
                    if index % 2 == 0 {
                        spend_wallet_nonce(&first, &offered, NOW + LIVE_FOR).await
                    } else {
                        spend_wallet_nonce(&second, &offered, NOW + LIVE_FOR).await
                    }
                })
            })
            .collect();
        let mut winners = 0;
        for racer in racers {
            if racer.await.unwrap().unwrap() {
                winners += 1;
            }
        }

        assert_eq!(winners, 1);
    }
}

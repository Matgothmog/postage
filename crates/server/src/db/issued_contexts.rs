//! Every rp_context this server has signed, and which challenge it was signed
//! for.
//!
//! Two questions matter about a proof coming back: did we ask for this one,
//! and have we already been paid with it. Both are answered here rather than
//! in the route, because the answer has to survive the request that asked -
//! the next call may land on an instance that has never seen the previous one,
//! and any ceiling kept in process memory would count to one forever.
//!
//! Nothing here identifies anyone. A nonce is 32 random bytes and a challenge
//! token is already a bearer capability the sender holds.

use libsql::params;

use super::{Db, DbError};

/// How many unexpired contexts one challenge token may hold at a time.
///
/// Set above a retry rather than at it: a sender who dismisses World App,
/// loses signal and tries again has spent three before anything is wrong.
/// What it stops is the other case - one leaked token minting signed requests
/// for as long as anyone cares to ask.
pub const MAX_LIVE_CONTEXTS_PER_TOKEN: i64 = 5;

/// Writes down a context we are about to hand out, and says whether this token
/// had a slot left for it.
///
/// Counting and recording are one statement. Read-then-write lets a burst -
/// which is the case a ceiling exists for - all see room and all take it, so
/// the real limit becomes the cap plus however many arrived together.
pub async fn record_issued_context(
    db: &Db,
    token: &str,
    nonce: &str,
    created_at: i64,
    expires_at: i64,
    now: i64,
) -> Result<bool, DbError> {
    let issued = db
        .run(
            "INSERT INTO issued_rp_contexts (nonce, token, created_at, expires_at)
          SELECT ?, ?, ?, ?
          WHERE (SELECT COUNT(*) FROM issued_rp_contexts
                 WHERE token = ? AND expires_at > ?) < ?",
            params![
                nonce,
                token,
                created_at,
                expires_at,
                token,
                now,
                MAX_LIVE_CONTEXTS_PER_TOKEN
            ],
        )
        .await?;
    Ok(issued > 0)
}

/// Spends the context a proof was produced under, and says whether this caller
/// is the one who got to spend it.
///
/// Every World ID proof carries back the nonce of the request it answers, so
/// this is what turns a signed context into something single use: a proof
/// harvested by a third party under our rp and action can only be presented
/// against the token it was signed for, and only before another presentation
/// of the same proof. The condition and the write are one statement, so two
/// requests racing on one nonce cannot both come away believing they spent it.
pub async fn consume_issued_context(
    db: &Db,
    token: &str,
    nonce: &str,
    now: i64,
) -> Result<bool, DbError> {
    let consumed = db
        .run(
            "UPDATE issued_rp_contexts SET consumed_at = ?
          WHERE nonce = ? AND token = ? AND consumed_at IS NULL AND expires_at > ?",
            params![now, nonce, token, now],
        )
        .await?;
    Ok(consumed > 0)
}

/// Drops contexts whose window has closed, rather than leaving a row per
/// verification attempt forever under a COUNT every attempt pays for. Nothing
/// depends on it having run: an expired row is already refused by both
/// statements above.
pub async fn purge_expired_contexts(db: &Db, now: i64) -> Result<(), DbError> {
    db.run(
        "DELETE FROM issued_rp_contexts WHERE expires_at <= ?",
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
    const TOKEN: &str = "tok";
    const LIVE_FOR: i64 = 300;

    fn nonce(n: i64) -> String {
        format!("0x{}", format!("{n:02}").repeat(32))
    }

    async fn issue(db: &Db, token: &str, id: i64) -> bool {
        record_issued_context(db, token, &nonce(id), NOW, NOW + LIVE_FOR, NOW)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn caps_how_many_live_contexts_one_challenge_token_may_hold_at_once() {
        let db = TestDb::fresh().await;
        for issued in 0..MAX_LIVE_CONTEXTS_PER_TOKEN {
            assert!(
                issue(&db, TOKEN, issued).await,
                "context {issued} should have been issued"
            );
        }

        assert!(!issue(&db, TOKEN, MAX_LIVE_CONTEXTS_PER_TOKEN).await);
    }

    #[tokio::test]
    async fn counts_the_cap_per_token_so_one_sender_cannot_close_anothers_lane() {
        let db = TestDb::fresh().await;
        for issued in 0..MAX_LIVE_CONTEXTS_PER_TOKEN {
            issue(&db, TOKEN, issued).await;
        }

        assert!(
            record_issued_context(&db, "other", &nonce(99), NOW, NOW + LIVE_FOR, NOW)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn an_expired_context_frees_its_slot_so_an_honest_sender_can_always_retry() {
        let db = TestDb::fresh().await;
        let expired = NOW - 10;
        for issued in 0..MAX_LIVE_CONTEXTS_PER_TOKEN {
            record_issued_context(&db, TOKEN, &nonce(issued), expired - LIVE_FOR, expired, NOW)
                .await
                .unwrap();
        }

        assert!(issue(&db, TOKEN, MAX_LIVE_CONTEXTS_PER_TOKEN).await);
    }

    #[tokio::test]
    async fn a_nonce_is_spendable_exactly_once() {
        let db = TestDb::fresh().await;
        issue(&db, TOKEN, 1).await;

        assert!(
            consume_issued_context(&db, TOKEN, &nonce(1), NOW)
                .await
                .unwrap()
        );
        assert!(
            !consume_issued_context(&db, TOKEN, &nonce(1), NOW)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn refuses_a_nonce_this_route_never_issued() {
        let db = TestDb::fresh().await;

        assert!(
            !consume_issued_context(&db, TOKEN, &nonce(1), NOW)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn refuses_a_nonce_issued_against_a_different_challenge_token() {
        let db = TestDb::fresh().await;
        issue(&db, "other", 1).await;

        assert!(
            !consume_issued_context(&db, TOKEN, &nonce(1), NOW)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn refuses_a_nonce_whose_signing_window_has_already_closed() {
        let db = TestDb::fresh().await;
        let expired = NOW - 1;
        record_issued_context(&db, TOKEN, &nonce(1), expired - LIVE_FOR, expired, NOW)
            .await
            .unwrap();

        assert!(
            !consume_issued_context(&db, TOKEN, &nonce(1), NOW)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn purging_drops_closed_windows_and_leaves_open_ones_spendable() {
        let db = TestDb::fresh().await;
        let expired = NOW - 1;
        record_issued_context(&db, TOKEN, &nonce(1), expired - LIVE_FOR, expired, NOW)
            .await
            .unwrap();
        issue(&db, TOKEN, 2).await;

        purge_expired_contexts(&db, NOW).await.unwrap();

        assert!(
            consume_issued_context(&db, TOKEN, &nonce(2), NOW)
                .await
                .unwrap()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_consumes_of_one_nonce_through_two_handles_succeed_exactly_once() {
        let first = Arc::new(TestDb::fresh().await);
        let second = Arc::new(first.second_handle().await);
        issue(&first, TOKEN, 1).await;

        let racers: Vec<_> = (0..8)
            .map(|index| {
                let first = Arc::clone(&first);
                let second = Arc::clone(&second);
                tokio::spawn(async move {
                    let db: &Db = if index % 2 == 0 { &first } else { &second };
                    consume_issued_context(db, TOKEN, &nonce(1), NOW).await
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_burst_of_issues_through_two_handles_never_exceeds_the_cap() {
        let first = Arc::new(TestDb::fresh().await);
        let second = Arc::new(first.second_handle().await);

        let racers: Vec<_> = (0..12)
            .map(|index| {
                let first = Arc::clone(&first);
                let second = Arc::clone(&second);
                tokio::spawn(async move {
                    let db: &Db = if index % 2 == 0 { &first } else { &second };
                    record_issued_context(db, TOKEN, &nonce(index), NOW, NOW + LIVE_FOR, NOW).await
                })
            })
            .collect();
        let mut issued = 0;
        for racer in racers {
            if racer.await.unwrap().unwrap() {
                issued += 1;
            }
        }

        assert_eq!(issued, MAX_LIVE_CONTEXTS_PER_TOKEN);
    }
}

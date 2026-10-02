//! Challenges: the link emailed to a sender whose message is being held, and
//! the single-use rights hanging off it (release the hold, claim the answer,
//! be entitled to a delivery, record that it happened).
//!
//! Every right is taken by one conditional statement, so two requests racing on
//! the same token cannot both come away believing they hold it. `now` comes
//! from the caller; nothing here reads a clock.

use libsql::params;
use serde::Deserialize;

use super::{Db, DbError};

/// Longer than the pass window on purpose. A pass measures how recently someone
/// proved they were there; a hold measures how long a person takes to read the
/// mail asking them. Anything shorter and a sender who answers over lunch finds
/// their message gone and has to write it again.
pub const HOLD_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Challenge {
    pub token: String,
    pub handle: String,
    pub sender: String,
    pub message_id: String,
    pub tier: String,
    pub amount: String,
    /// Set while the worker still holds the message. None once released,
    /// expired, or never held at all.
    pub held_until: Option<i64>,
    /// The enclave-signed quote, kept so the sender can pay it from the
    /// challenge page without us re-pricing the message we no longer hold.
    pub quote_json: String,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
    pub delivered_at: Option<i64>,
    pub entitled_at: Option<i64>,
    pub settled_by: Option<String>,
}

/// A challenge as it is first written, before anything has resolved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewChallenge {
    pub token: String,
    pub handle: String,
    pub sender: String,
    pub message_id: String,
    pub tier: String,
    pub amount: String,
    pub held_until: Option<i64>,
    pub quote_json: String,
    pub created_at: i64,
}

pub async fn create_challenge(db: &Db, challenge: &NewChallenge) -> Result<(), DbError> {
    db.run(
        "INSERT INTO challenges
       (token, handle, sender, message_id, tier, amount, quote_json, held_until, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            challenge.token.as_str(),
            challenge.handle.as_str(),
            challenge.sender.as_str(),
            challenge.message_id.as_str(),
            challenge.tier.as_str(),
            challenge.amount.as_str(),
            challenge.quote_json.as_str(),
            challenge.held_until,
            challenge.created_at
        ],
    )
    .await?;
    Ok(())
}

/// Takes the right to release a held message, once. The condition and the write
/// are one statement, so two requests racing on the same token cannot both come
/// away believing they may send it - only the one that changed a row may.
pub async fn claim_hold(db: &Db, token: &str, now: i64) -> Result<bool, DbError> {
    let claimed = db
        .run(
            "UPDATE challenges SET held_until = NULL WHERE token = ? AND held_until > ?",
            params![token, now],
        )
        .await?;
    Ok(claimed > 0)
}

/// Forgets every hold that ran out. The message itself is dropped by the worker
/// at its deadline whatever happens here; this only clears our record that one
/// was outstanding, so a challenge page stops offering to release something that
/// is already gone.
pub async fn purge_expired_holds(db: &Db, now: i64) -> Result<(), DbError> {
    db.run(
        "UPDATE challenges SET held_until = NULL WHERE held_until IS NOT NULL AND held_until <= ?",
        params![now],
    )
    .await?;
    Ok(())
}

pub async fn challenge_by_token(db: &Db, token: &str) -> Result<Option<Challenge>, DbError> {
    let rows: Vec<Challenge> = db
        .all("SELECT * FROM challenges WHERE token = ?", params![token])
        .await?;
    Ok(rows.into_iter().next())
}

/// Claims a challenge, and says whether this caller is the one who got it.
///
/// The link arrives by email and opening it twice is ordinary, so the check and
/// the write have to be one statement. Two callers reading "not settled" and
/// both granting would reset the pass after the first had already spent it -
/// one payment, two deliveries.
pub async fn claim_challenge(
    db: &Db,
    token: &str,
    settled_by: &str,
    now: i64,
) -> Result<bool, DbError> {
    // A paid claim takes the entitlement with it. Deciding that separately let
    // the loser of the race read "nobody has been entitled yet" during the
    // hundreds of milliseconds the winner spends handing the message to the
    // worker, and hand out a second delivery for one payment.
    let claimed = db
        .run(
            "UPDATE challenges
          SET resolved_at = ?, settled_by = ?,
              entitled_at = CASE WHEN ? = 'paid' THEN ? ELSE entitled_at END
          WHERE token = ? AND resolved_at IS NULL",
            params![now, settled_by, settled_by, now, token],
        )
        .await?;
    Ok(claimed > 0)
}

/// Puts a claimed challenge back. Whatever the claim was taken for did not
/// happen, and a payment that cannot be made twice must not leave the only way
/// through it bought closed behind it.
///
/// Gives back only the claim this caller took. A blind rollback also cleared an
/// entitlement another request had taken in the meantime, which let that request
/// take it a second time and grant a second delivery for one payment.
pub async fn release_challenge_claim(
    db: &Db,
    token: &str,
    settled_by: &str,
) -> Result<(), DbError> {
    db.run(
        "UPDATE challenges
     SET resolved_at = NULL, settled_by = NULL,
         entitled_at = CASE WHEN ? = 'paid' THEN NULL ELSE entitled_at END
     WHERE token = ? AND settled_by = ?",
        params![settled_by, token, settled_by],
    )
    .await?;
    Ok(())
}

/// Records that this challenge has issued what it owed, so it cannot issue it
/// again. A payment settles onchain forever, and without this the sender could
/// spend the delivery it bought and then ask for another.
pub async fn mark_entitled(db: &Db, token: &str, now: i64) -> Result<bool, DbError> {
    let marked = db
        .run(
            "UPDATE challenges SET entitled_at = ? WHERE token = ? AND entitled_at IS NULL",
            params![now, token],
        )
        .await?;
    Ok(marked > 0)
}

/// Records that the held message actually reached the recipient, so nobody has
/// to guess afterwards. Pass state cannot answer this: a human pass from an
/// earlier message looks the same as one granted because delivery failed.
///
/// Unguarded, where [`mark_entitled`] just above refuses to write twice,
/// because the two are reached under different rules. An entitlement is taken
/// by whoever answers the challenge, and the sender's page answers repeatedly
/// while it waits for the payment to mine, so there the check and the write
/// have to be one statement. A delivery is recorded only by the caller that
/// just made one, and the right to make one is taken upstream by
/// [`claim_hold`], a single conditional update turning a live hold into a
/// released one,
/// which nothing turns back. It succeeds once per token for good, so this
/// statement runs once per token for good and `delivered_at IS NULL` would
/// always hold.
pub async fn mark_delivered(db: &Db, token: &str, now: i64) -> Result<(), DbError> {
    db.run(
        "UPDATE challenges SET delivered_at = ? WHERE token = ?",
        params![now, token],
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
    const TOKEN: &str = "tok-1";

    fn held_challenge(token: &str, held_until: Option<i64>) -> NewChallenge {
        NewChallenge {
            token: token.to_owned(),
            handle: "demo".to_owned(),
            sender: "Sender@Example.com".to_owned(),
            message_id: "<m1@example.com>".to_owned(),
            tier: "stranger".to_owned(),
            amount: "50".to_owned(),
            held_until,
            quote_json: "{\"q\":1}".to_owned(),
            created_at: NOW,
        }
    }

    async fn db_with_challenge(held_until: Option<i64>) -> TestDb {
        let db = TestDb::fresh().await;
        create_challenge(&db, &held_challenge(TOKEN, held_until))
            .await
            .unwrap();
        db
    }

    async fn stored(db: &Db) -> Challenge {
        challenge_by_token(db, TOKEN).await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn a_created_challenge_reads_back_unresolved_and_verbatim() {
        let db = db_with_challenge(Some(NOW + HOLD_SECONDS)).await;

        assert_eq!(
            stored(&db).await,
            Challenge {
                token: TOKEN.to_owned(),
                handle: "demo".to_owned(),
                sender: "Sender@Example.com".to_owned(),
                message_id: "<m1@example.com>".to_owned(),
                tier: "stranger".to_owned(),
                amount: "50".to_owned(),
                held_until: Some(NOW + HOLD_SECONDS),
                quote_json: "{\"q\":1}".to_owned(),
                created_at: NOW,
                resolved_at: None,
                delivered_at: None,
                entitled_at: None,
                settled_by: None,
            }
        );
    }

    #[tokio::test]
    async fn an_unknown_token_is_none() {
        let db = TestDb::fresh().await;

        assert_eq!(challenge_by_token(&db, "nope").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_challenge_that_was_never_held_stores_no_deadline() {
        let db = db_with_challenge(None).await;

        assert_eq!(stored(&db).await.held_until, None);
    }

    #[tokio::test]
    async fn creating_the_same_token_twice_is_an_error() {
        let db = db_with_challenge(None).await;

        let again = create_challenge(&db, &held_challenge(TOKEN, None)).await;

        assert!(again.is_err());
    }

    #[tokio::test]
    async fn a_live_hold_can_be_claimed_once() {
        let db = db_with_challenge(Some(NOW + 100)).await;

        assert!(claim_hold(&db, TOKEN, NOW).await.unwrap());
        assert!(!claim_hold(&db, TOKEN, NOW).await.unwrap());
        assert_eq!(stored(&db).await.held_until, None);
    }

    #[tokio::test]
    async fn a_hold_is_dead_at_its_deadline_and_live_one_second_before() {
        let db = db_with_challenge(Some(NOW + 100)).await;

        assert!(!claim_hold(&db, TOKEN, NOW + 100).await.unwrap());
        assert!(claim_hold(&db, TOKEN, NOW + 99).await.unwrap());
    }

    #[tokio::test]
    async fn claiming_the_hold_of_a_challenge_that_never_had_one_fails() {
        let db = db_with_challenge(None).await;

        assert!(!claim_hold(&db, TOKEN, NOW).await.unwrap());
        assert!(!claim_hold(&db, "unknown", NOW).await.unwrap());
    }

    #[tokio::test]
    async fn purging_clears_holds_up_to_and_including_now_and_leaves_later_ones() {
        let db = TestDb::fresh().await;
        for (token, held_until) in [("past", NOW - 1), ("edge", NOW), ("later", NOW + 1)] {
            create_challenge(&db, &held_challenge(token, Some(held_until)))
                .await
                .unwrap();
        }

        purge_expired_holds(&db, NOW).await.unwrap();

        let held = |token: &'static str| {
            let db = &db;
            async move {
                challenge_by_token(db, token)
                    .await
                    .unwrap()
                    .unwrap()
                    .held_until
            }
        };
        assert_eq!(held("past").await, None);
        assert_eq!(held("edge").await, None);
        assert_eq!(held("later").await, Some(NOW + 1));
    }

    #[tokio::test]
    async fn the_first_claim_wins_and_records_who_settled_it() {
        let db = db_with_challenge(None).await;

        assert!(claim_challenge(&db, TOKEN, "human", NOW + 5).await.unwrap());
        assert!(!claim_challenge(&db, TOKEN, "paid", NOW + 6).await.unwrap());

        let challenge = stored(&db).await;
        assert_eq!(challenge.resolved_at, Some(NOW + 5));
        assert_eq!(challenge.settled_by.as_deref(), Some("human"));
        assert_eq!(
            challenge.entitled_at, None,
            "a human claim is not an entitlement"
        );
    }

    #[tokio::test]
    async fn a_paid_claim_takes_the_entitlement_with_it() {
        let db = db_with_challenge(None).await;

        assert!(claim_challenge(&db, TOKEN, "paid", NOW + 5).await.unwrap());

        assert_eq!(stored(&db).await.entitled_at, Some(NOW + 5));
        assert!(!mark_entitled(&db, TOKEN, NOW + 6).await.unwrap());
    }

    #[tokio::test]
    async fn claiming_an_unknown_token_changes_nothing() {
        let db = TestDb::fresh().await;

        assert!(!claim_challenge(&db, "nope", "paid", NOW).await.unwrap());
    }

    #[tokio::test]
    async fn releasing_a_paid_claim_reopens_it_and_clears_the_entitlement() {
        let db = db_with_challenge(None).await;
        claim_challenge(&db, TOKEN, "paid", NOW + 5).await.unwrap();

        release_challenge_claim(&db, TOKEN, "paid").await.unwrap();

        let challenge = stored(&db).await;
        assert_eq!(challenge.resolved_at, None);
        assert_eq!(challenge.settled_by, None);
        assert_eq!(challenge.entitled_at, None);
        assert!(claim_challenge(&db, TOKEN, "paid", NOW + 9).await.unwrap());
    }

    #[tokio::test]
    async fn releasing_a_human_claim_keeps_an_entitlement_taken_meanwhile() {
        let db = db_with_challenge(None).await;
        claim_challenge(&db, TOKEN, "human", NOW + 5).await.unwrap();
        mark_entitled(&db, TOKEN, NOW + 6).await.unwrap();

        release_challenge_claim(&db, TOKEN, "human").await.unwrap();

        let challenge = stored(&db).await;
        assert_eq!(challenge.resolved_at, None);
        assert_eq!(challenge.entitled_at, Some(NOW + 6));
    }

    #[tokio::test]
    async fn releasing_gives_back_only_the_claim_the_caller_took() {
        let db = db_with_challenge(None).await;
        claim_challenge(&db, TOKEN, "paid", NOW + 5).await.unwrap();

        release_challenge_claim(&db, TOKEN, "human").await.unwrap();

        let challenge = stored(&db).await;
        assert_eq!(challenge.settled_by.as_deref(), Some("paid"));
        assert_eq!(challenge.entitled_at, Some(NOW + 5));
    }

    #[tokio::test]
    async fn an_entitlement_is_marked_once() {
        let db = db_with_challenge(None).await;

        assert!(mark_entitled(&db, TOKEN, NOW + 1).await.unwrap());
        assert!(!mark_entitled(&db, TOKEN, NOW + 2).await.unwrap());
        assert_eq!(stored(&db).await.entitled_at, Some(NOW + 1));
    }

    #[tokio::test]
    async fn marking_a_challenge_delivered_records_the_time_and_is_unguarded() {
        let db = db_with_challenge(None).await;

        mark_delivered(&db, TOKEN, NOW + 7).await.unwrap();
        assert_eq!(stored(&db).await.delivered_at, Some(NOW + 7));

        mark_delivered(&db, TOKEN, NOW + 8).await.unwrap();
        assert_eq!(stored(&db).await.delivered_at, Some(NOW + 8));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn racing_claims_of_one_challenge_have_exactly_one_winner() {
        let first = Arc::new(db_with_challenge(Some(NOW + 100)).await);
        let second = Arc::new(first.second_handle().await);

        let racers: Vec<_> = (0..8)
            .map(|index| {
                let first = Arc::clone(&first);
                let second = Arc::clone(&second);
                tokio::spawn(async move {
                    if index % 2 == 0 {
                        claim_challenge(&first, TOKEN, "paid", NOW).await
                    } else {
                        claim_challenge(&second, TOKEN, "paid", NOW).await
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn racing_claims_of_one_hold_have_exactly_one_winner() {
        let first = Arc::new(db_with_challenge(Some(NOW + 100)).await);
        let second = Arc::new(first.second_handle().await);

        let racers: Vec<_> = (0..8)
            .map(|index| {
                let first = Arc::clone(&first);
                let second = Arc::clone(&second);
                tokio::spawn(async move {
                    if index % 2 == 0 {
                        claim_hold(&first, TOKEN, NOW).await
                    } else {
                        claim_hold(&second, TOKEN, NOW).await
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

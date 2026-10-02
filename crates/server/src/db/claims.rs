//! Inbox claims: a handle someone is part way through claiming, the ledger of
//! confirmation mails that throttles claiming, and the ration of questions a
//! claim may put to Cloudflare.
//!
//! A claim becomes a row in `inboxes` only once its owner has proved they can
//! read the address they point it at, so an unfinished claim forwards nothing.
//! `now` comes from the caller; nothing here reads a clock.

use libsql::params;
use serde::Deserialize;

use super::{Db, DbError};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InboxClaim {
    pub handle: String,
    pub destination: String,
    pub wallet: String,
    pub code_hash: String,
    /// When the emailed code stops being accepted, and until then how long the
    /// handle is held against another wallet. It is not a deadline on the claim:
    /// promotion deliberately ignores it, because the code is checked against it
    /// when it is entered and Cloudflare's own link has no deadline of ours. A
    /// claim whose code went in at minute fourteen must still go live when its
    /// owner clicks that link over lunch.
    pub expires_at: i64,
    pub attempts: i64,
    pub code_verified_at: Option<i64>,
    pub cf_address_id: Option<String>,
    pub cf_verified_at: Option<i64>,
    pub cf_checked_at: Option<i64>,
    pub cf_checks: i64,
    pub created_at: i64,
}

/// The fields a claim is started with; the rest begin empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewClaim {
    pub handle: String,
    pub destination: String,
    pub wallet: String,
    pub code_hash: String,
    pub expires_at: i64,
    pub cf_address_id: Option<String>,
    pub cf_verified_at: Option<i64>,
}

/// Starts a claim, or starts an existing handle's claim over.
pub async fn start_claim(db: &Db, claim: &NewClaim, now: i64) -> Result<(), DbError> {
    db.run(
        "INSERT INTO inbox_claims
       (handle, destination, wallet, code_hash, expires_at, attempts, code_verified_at,
        cf_address_id, cf_verified_at, created_at)
     VALUES (?, ?, ?, ?, ?, 0, NULL, ?, ?, ?)
     ON CONFLICT (handle) DO UPDATE SET
       destination = excluded.destination,
       wallet = excluded.wallet,
       code_hash = excluded.code_hash,
       expires_at = excluded.expires_at,
       attempts = 0,
       code_verified_at = NULL,
       cf_address_id = excluded.cf_address_id,
       cf_verified_at = excluded.cf_verified_at,
       -- Starting again is starting again. Without this a claim that spent its
       -- budget could never be retried, only abandoned.
       cf_checked_at = NULL,
       cf_checks = 0",
        params![
            claim.handle.to_lowercase(),
            claim.destination.to_lowercase(),
            claim.wallet.to_lowercase(),
            claim.code_hash.as_str(),
            claim.expires_at,
            claim.cf_address_id.as_deref(),
            claim.cf_verified_at,
            now
        ],
    )
    .await?;
    Ok(())
}

pub async fn record_claim_send(
    db: &Db,
    destination: &str,
    wallet: &str,
    now: i64,
) -> Result<(), DbError> {
    db.run(
        "INSERT INTO claim_sends (destination, wallet, sent_at) VALUES (?, ?, ?)",
        params![destination.to_lowercase(), wallet.to_lowercase(), now],
    )
    .await?;
    Ok(())
}

/// How often this mailbox has been asked to confirm. The address is the victim
/// of an email bomb, so it is counted whoever aimed it.
pub async fn recent_claims_to(
    db: &Db,
    destination: &str,
    window_seconds: i64,
    now: i64,
) -> Result<i64, DbError> {
    count_claims(
        db,
        SendColumn::Destination,
        destination,
        window_seconds,
        now,
    )
    .await
}

/// How many claims this wallet has started. The destination throttle cannot see
/// this: one wallet naming a different address each time passes it every time,
/// and every claim that gets as far as a code registers a Cloudflare
/// destination, which the account has a hard cap on and no way to delete.
pub async fn recent_claims_from(
    db: &Db,
    wallet: &str,
    window_seconds: i64,
    now: i64,
) -> Result<i64, DbError> {
    count_claims(db, SendColumn::Wallet, wallet, window_seconds, now).await
}

#[derive(Debug, Clone, Copy)]
enum SendColumn {
    Destination,
    Wallet,
}

impl SendColumn {
    fn name(self) -> &'static str {
        match self {
            Self::Destination => "destination",
            Self::Wallet => "wallet",
        }
    }
}

async fn count_claims(
    db: &Db,
    column: SendColumn,
    value: &str,
    window_seconds: i64,
    now: i64,
) -> Result<i64, DbError> {
    #[derive(Deserialize)]
    struct Count {
        n: i64,
    }
    let rows: Vec<Count> = db
        .all(
            &format!(
                "SELECT COUNT(*) AS n FROM claim_sends WHERE {} = ? AND sent_at > ?",
                column.name()
            ),
            params![value.to_lowercase(), now - window_seconds],
        )
        .await?;
    Ok(rows.first().map_or(0, |row| row.n))
}

/// Drops rows past the longest window anything throttles over. Called on the way
/// past, like the classification purge, rather than left to grow a row per
/// signup attempt forever under a COUNT every later attempt pays for.
pub async fn purge_old_claim_sends(db: &Db, window_seconds: i64, now: i64) -> Result<(), DbError> {
    db.run(
        "DELETE FROM claim_sends WHERE sent_at <= ?",
        params![now - window_seconds],
    )
    .await?;
    Ok(())
}

pub async fn claim_by_handle(db: &Db, handle: &str) -> Result<Option<InboxClaim>, DbError> {
    let rows: Vec<InboxClaim> = db
        .all(
            "SELECT * FROM inbox_claims WHERE handle = ?",
            params![handle.to_lowercase()],
        )
        .await?;
    Ok(rows.into_iter().next())
}

/// Takes a guess before checking the code rather than after, so concurrent
/// requests cannot all read the same count and slip past the ceiling together.
/// Returns false once the allowance is spent.
pub async fn consume_attempt(db: &Db, handle: &str, max: i64) -> Result<bool, DbError> {
    let consumed = db
        .run(
            "UPDATE inbox_claims SET attempts = attempts + 1 WHERE handle = ? AND attempts < ?",
            params![handle.to_lowercase(), max],
        )
        .await?;
    Ok(consumed > 0)
}

pub async fn mark_code_verified(db: &Db, handle: &str, now: i64) -> Result<(), DbError> {
    db.run(
        "UPDATE inbox_claims SET code_verified_at = ? WHERE handle = ? AND code_verified_at IS NULL",
        params![now, handle.to_lowercase()],
    )
    .await?;
    Ok(())
}

/// Registering the address with Cloudflare answers the same question a check
/// does, so it is recorded as one. Otherwise `settleClaim`, which runs moments
/// later on both signup paths, spends a second call asking what we just learned.
pub async fn attach_destination(
    db: &Db,
    handle: &str,
    address_id: &str,
    verified_at: Option<i64>,
    now: i64,
) -> Result<(), DbError> {
    db.run(
        "UPDATE inbox_claims
     SET cf_address_id = ?, cf_verified_at = ?, cf_checked_at = ?, cf_checks = cf_checks + 1
     WHERE handle = ?",
        params![address_id, verified_at, now, handle.to_lowercase()],
    )
    .await?;
    Ok(())
}

/// The least time between two questions to Cloudflare about one claim, and the
/// most questions a single claim may ever cause.
///
/// `GET /api/inbox/verify` is polled by an anonymous browser, and it used to
/// spend one Cloudflare API call per request against a limit that belongs to the
/// whole account. Anyone who knew a handle mid-claim could spend it in a loop.
///
/// The interval matches the page's own poll, so one person waiting is not slowed
/// down at all, and a thousand requests for the same claim now cost what one
/// does. The budget is what makes it bounded rather than merely slow: nobody can
/// keep a claim answering questions forever, and after it the claim simply stops
/// being asked about until it is started again.
pub const CF_CHECK_INTERVAL_SECONDS: i64 = 4;
pub const CF_CHECK_BUDGET: i64 = 200;

/// Takes the right to ask Cloudflare about this claim, once.
///
/// The condition and the write are one statement, so a burst of pollers cannot
/// all read the same last-checked time and all go and ask. A claim already
/// verified never needs asking again, so it is refused here rather than in the
/// caller.
pub async fn take_cloudflare_check(db: &Db, handle: &str, now: i64) -> Result<bool, DbError> {
    let taken = db
        .run(
            "UPDATE inbox_claims
          SET cf_checked_at = ?, cf_checks = cf_checks + 1
          WHERE handle = ?
            AND cf_verified_at IS NULL
            AND cf_checks < ?
            AND (cf_checked_at IS NULL OR cf_checked_at <= ?)",
            params![
                now,
                handle.to_lowercase(),
                CF_CHECK_BUDGET,
                now - CF_CHECK_INTERVAL_SECONDS
            ],
        )
        .await?;
    Ok(taken > 0)
}

/// Whether this claim has spent its whole budget without Cloudflare ever
/// confirming. Nothing will ask again, so the page is told rather than left
/// polling something that has stopped answering.
pub async fn cloudflare_checks_exhausted(db: &Db, handle: &str) -> Result<bool, DbError> {
    #[derive(Deserialize)]
    struct Checks {
        cf_checks: i64,
    }
    let rows: Vec<Checks> = db
        .all(
            "SELECT cf_checks FROM inbox_claims WHERE handle = ? AND cf_verified_at IS NULL",
            params![handle.to_lowercase()],
        )
        .await?;
    Ok(rows
        .first()
        .is_some_and(|row| row.cf_checks >= CF_CHECK_BUDGET))
}

/// Pinned to the address the status was read for. Without that, a slow reply
/// about one destination could stamp a claim that has since been repointed at
/// another, marking an address Cloudflare never verified as verified.
pub async fn mark_cloudflare_verified(
    db: &Db,
    handle: &str,
    address_id: &str,
    verified_at: i64,
) -> Result<(), DbError> {
    db.run(
        "UPDATE inbox_claims SET cf_verified_at = ? WHERE handle = ? AND cf_address_id = ?",
        params![verified_at, handle.to_lowercase(), address_id],
    )
    .await?;
    Ok(())
}

/// Dropped once the handle is a real inbox, so a used code hash is not kept.
pub async fn clear_claim(db: &Db, handle: &str) -> Result<(), DbError> {
    db.run(
        "DELETE FROM inbox_claims WHERE handle = ?",
        params![handle.to_lowercase()],
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
    const HOUR: i64 = 3600;

    fn wallet() -> String {
        format!("0x{}", "11".repeat(20))
    }

    fn new_claim(handle: &str, destination: &str, wallet: &str) -> NewClaim {
        NewClaim {
            handle: handle.to_owned(),
            destination: destination.to_owned(),
            wallet: wallet.to_owned(),
            code_hash: "deadbeef".to_owned(),
            expires_at: NOW + 900,
            cf_address_id: None,
            cf_verified_at: None,
        }
    }

    async fn claim(db: &Db, handle: &str) {
        claim_to(db, handle, "someone@example.com", &wallet()).await;
    }

    async fn claim_to(db: &Db, handle: &str, destination: &str, wallet: &str) {
        start_claim(db, &new_claim(handle, destination, wallet), NOW)
            .await
            .unwrap();
    }

    /// Moves the claim's last-checked time into the past, so the interval has
    /// elapsed without a test waiting it out.
    async fn wind_back(db: &Db, handle: &str) {
        db.run(
            "UPDATE inbox_claims SET cf_checked_at = ? WHERE handle = ?",
            params![NOW - CF_CHECK_INTERVAL_SECONDS - 1, handle],
        )
        .await
        .unwrap();
    }

    async fn set_checks(db: &Db, handle: &str, checks: i64) {
        db.run(
            "UPDATE inbox_claims SET cf_checks = ? WHERE handle = ?",
            params![checks, handle],
        )
        .await
        .unwrap();
    }

    async fn stored(db: &Db, handle: &str) -> InboxClaim {
        claim_by_handle(db, handle).await.unwrap().unwrap()
    }

    /// The route that leads to this is polled by an anonymous browser and every
    /// request used to spend one Cloudflare call against an account-wide quota.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_thousand_pollers_cost_one_cloudflare_call_not_a_thousand() {
        let first = Arc::new(TestDb::fresh().await);
        let second = Arc::new(first.second_handle().await);
        claim(&first, "demo").await;

        let pollers: Vec<_> = (0..1000)
            .map(|index| {
                let first = Arc::clone(&first);
                let second = Arc::clone(&second);
                tokio::spawn(async move {
                    if index % 2 == 0 {
                        take_cloudflare_check(&first, "demo", NOW).await
                    } else {
                        take_cloudflare_check(&second, "demo", NOW).await
                    }
                })
            })
            .collect();
        let mut taken = 0;
        for poller in pollers {
            if poller.await.unwrap().unwrap() {
                taken += 1;
            }
        }

        assert_eq!(
            taken, 1,
            "concurrent pollers must not all read the same last-checked time and all go and ask"
        );
    }

    #[tokio::test]
    async fn the_ration_is_per_claim_so_one_claim_cannot_starve_another() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;
        claim_to(&db, "other", "someone-else@example.com", &wallet()).await;

        assert!(take_cloudflare_check(&db, "demo", NOW).await.unwrap());
        assert!(take_cloudflare_check(&db, "other", NOW).await.unwrap());
    }

    /// Rate alone only makes a loop slow. The budget is what makes it finite.
    #[tokio::test]
    async fn a_claim_stops_answering_once_its_budget_is_spent() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;

        // Walked up to the last one rather than looped through all of them:
        // the rule under test is the boundary.
        set_checks(&db, "demo", CF_CHECK_BUDGET - 1).await;
        wind_back(&db, "demo").await;
        assert!(
            take_cloudflare_check(&db, "demo", NOW).await.unwrap(),
            "the last of the budget is still budget"
        );

        wind_back(&db, "demo").await;
        assert!(
            !take_cloudflare_check(&db, "demo", NOW).await.unwrap(),
            "the budget is a ceiling, not a rate"
        );
        assert!(cloudflare_checks_exhausted(&db, "demo").await.unwrap());
    }

    /// Registering the address is itself a question to Cloudflare, and
    /// `settleClaim` runs moments later on both signup paths. Counting it stops
    /// every signup spending two calls to learn one thing.
    #[tokio::test]
    async fn registering_the_address_counts_as_having_just_asked() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;
        attach_destination(&db, "demo", "addr-1", None, NOW)
            .await
            .unwrap();

        assert!(
            !take_cloudflare_check(&db, "demo", NOW).await.unwrap(),
            "the answer is seconds old, so nothing should go and ask for it again"
        );
    }

    #[tokio::test]
    async fn a_claim_cloudflare_has_already_confirmed_is_never_asked_about_again() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;
        attach_destination(&db, "demo", "addr-1", None, NOW)
            .await
            .unwrap();
        mark_cloudflare_verified(&db, "demo", "addr-1", NOW)
            .await
            .unwrap();

        assert!(
            !take_cloudflare_check(&db, "demo", NOW + HOUR)
                .await
                .unwrap()
        );
        assert!(
            !cloudflare_checks_exhausted(&db, "demo").await.unwrap(),
            "a confirmed claim is finished, not stalled"
        );
    }

    #[tokio::test]
    async fn starting_again_gives_the_claim_its_budget_back() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;
        set_checks(&db, "demo", CF_CHECK_BUDGET).await;
        assert!(cloudflare_checks_exhausted(&db, "demo").await.unwrap());

        claim(&db, "demo").await;

        assert!(take_cloudflare_check(&db, "demo", NOW).await.unwrap());
    }

    /// The destination throttle counts who was mailed. It sees nothing at all
    /// when one wallet names a fresh address every time - and every claim that
    /// reaches a code registers a Cloudflare destination the account can never
    /// delete.
    #[tokio::test]
    async fn one_wallet_naming_a_new_address_each_time_is_still_counted() {
        let db = TestDb::fresh().await;
        for n in 0..4 {
            let destination = format!("fresh{n}@example.com");
            record_claim_send(&db, &destination, &wallet(), NOW)
                .await
                .unwrap();
            assert_eq!(
                recent_claims_to(&db, &destination, HOUR, NOW)
                    .await
                    .unwrap(),
                1
            );
        }

        assert_eq!(
            recent_claims_from(&db, &wallet(), HOUR, NOW).await.unwrap(),
            4
        );
    }

    #[tokio::test]
    async fn one_wallets_claims_are_not_counted_against_anothers() {
        let db = TestDb::fresh().await;
        record_claim_send(&db, "a@example.com", &wallet(), NOW)
            .await
            .unwrap();

        let other = format!("0x{}", "22".repeat(20));
        assert_eq!(recent_claims_from(&db, &other, HOUR, NOW).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn the_throttle_ledger_does_not_grow_forever() {
        let db = TestDb::fresh().await;
        record_claim_send(&db, "old@example.com", &wallet(), NOW)
            .await
            .unwrap();

        purge_old_claim_sends(&db, -1, NOW).await.unwrap();

        assert_eq!(
            recent_claims_from(&db, &wallet(), HOUR, NOW).await.unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn a_send_exactly_one_window_old_no_longer_counts_but_one_second_newer_does() {
        let db = TestDb::fresh().await;
        record_claim_send(&db, "edge@example.com", &wallet(), NOW - HOUR)
            .await
            .unwrap();
        record_claim_send(&db, "edge@example.com", &wallet(), NOW - HOUR + 1)
            .await
            .unwrap();

        assert_eq!(
            recent_claims_to(&db, "EDGE@example.com", HOUR, NOW)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn purging_keeps_sends_inside_the_window() {
        let db = TestDb::fresh().await;
        record_claim_send(&db, "a@example.com", &wallet(), NOW - HOUR)
            .await
            .unwrap();
        record_claim_send(&db, "b@example.com", &wallet(), NOW - HOUR + 1)
            .await
            .unwrap();

        purge_old_claim_sends(&db, HOUR, NOW).await.unwrap();

        assert_eq!(
            recent_claims_from(&db, &wallet(), 2 * HOUR, NOW)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn a_started_claim_reads_back_lower_cased_with_empty_progress() {
        let db = TestDb::fresh().await;
        start_claim(&db, &new_claim("Demo", "Owner@Example.com", "0xABC"), NOW)
            .await
            .unwrap();

        assert_eq!(
            stored(&db, "DEMO").await,
            InboxClaim {
                handle: "demo".to_owned(),
                destination: "owner@example.com".to_owned(),
                wallet: "0xabc".to_owned(),
                code_hash: "deadbeef".to_owned(),
                expires_at: NOW + 900,
                attempts: 0,
                code_verified_at: None,
                cf_address_id: None,
                cf_verified_at: None,
                cf_checked_at: None,
                cf_checks: 0,
                created_at: NOW,
            }
        );
    }

    #[tokio::test]
    async fn an_unknown_handle_has_no_claim() {
        let db = TestDb::fresh().await;

        assert_eq!(claim_by_handle(&db, "nobody").await.unwrap(), None);
    }

    #[tokio::test]
    async fn starting_again_resets_attempts_verification_and_checks() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;
        consume_attempt(&db, "demo", 5).await.unwrap();
        mark_code_verified(&db, "demo", NOW).await.unwrap();
        attach_destination(&db, "demo", "addr-1", Some(NOW), NOW)
            .await
            .unwrap();

        start_claim(&db, &new_claim("demo", "new@example.com", "0x2"), NOW + 50)
            .await
            .unwrap();

        let restarted = stored(&db, "demo").await;
        assert_eq!(restarted.destination, "new@example.com");
        assert_eq!(restarted.attempts, 0);
        assert_eq!(restarted.code_verified_at, None);
        assert_eq!(restarted.cf_address_id, None);
        assert_eq!(restarted.cf_verified_at, None);
        assert_eq!(restarted.cf_checked_at, None);
        assert_eq!(restarted.cf_checks, 0);
        assert_eq!(
            restarted.created_at, NOW,
            "the row keeps its first creation time"
        );
    }

    #[tokio::test]
    async fn attempts_stop_being_granted_at_the_maximum() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;

        assert!(consume_attempt(&db, "demo", 2).await.unwrap());
        assert!(consume_attempt(&db, "demo", 2).await.unwrap());
        assert!(!consume_attempt(&db, "demo", 2).await.unwrap());
        assert_eq!(stored(&db, "demo").await.attempts, 2);
    }

    #[tokio::test]
    async fn consuming_an_attempt_on_an_unknown_claim_is_refused() {
        let db = TestDb::fresh().await;

        assert!(!consume_attempt(&db, "nobody", 5).await.unwrap());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn racing_guesses_never_exceed_the_attempt_ceiling() {
        let first = Arc::new(TestDb::fresh().await);
        let second = Arc::new(first.second_handle().await);
        claim(&first, "demo").await;

        let guesses: Vec<_> = (0..20)
            .map(|index| {
                let first = Arc::clone(&first);
                let second = Arc::clone(&second);
                tokio::spawn(async move {
                    if index % 2 == 0 {
                        consume_attempt(&first, "demo", 5).await
                    } else {
                        consume_attempt(&second, "demo", 5).await
                    }
                })
            })
            .collect();
        let mut granted = 0;
        for guess in guesses {
            if guess.await.unwrap().unwrap() {
                granted += 1;
            }
        }

        assert_eq!(granted, 5);
    }

    #[tokio::test]
    async fn the_code_is_marked_verified_once_and_the_first_time_sticks() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;

        mark_code_verified(&db, "demo", NOW + 1).await.unwrap();
        mark_code_verified(&db, "demo", NOW + 2).await.unwrap();

        assert_eq!(stored(&db, "demo").await.code_verified_at, Some(NOW + 1));
    }

    #[tokio::test]
    async fn attaching_a_destination_records_it_and_counts_as_a_check() {
        let db = TestDb::fresh().await;
        claim(&db, "Demo").await;

        attach_destination(&db, "DEMO", "addr-1", Some(NOW + 3), NOW + 2)
            .await
            .unwrap();

        let attached = stored(&db, "demo").await;
        assert_eq!(attached.cf_address_id.as_deref(), Some("addr-1"));
        assert_eq!(attached.cf_verified_at, Some(NOW + 3));
        assert_eq!(attached.cf_checked_at, Some(NOW + 2));
        assert_eq!(attached.cf_checks, 1);
    }

    #[tokio::test]
    async fn a_check_is_granted_again_exactly_when_the_interval_has_elapsed() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;
        assert!(take_cloudflare_check(&db, "demo", NOW).await.unwrap());

        let one_early = take_cloudflare_check(&db, "demo", NOW + CF_CHECK_INTERVAL_SECONDS - 1)
            .await
            .unwrap();
        let on_time = take_cloudflare_check(&db, "demo", NOW + CF_CHECK_INTERVAL_SECONDS)
            .await
            .unwrap();

        assert!(!one_early);
        assert!(on_time);
    }

    #[tokio::test]
    async fn an_unknown_claim_is_neither_askable_nor_exhausted() {
        let db = TestDb::fresh().await;

        assert!(!take_cloudflare_check(&db, "nobody", NOW).await.unwrap());
        assert!(!cloudflare_checks_exhausted(&db, "nobody").await.unwrap());
    }

    #[tokio::test]
    async fn a_claim_one_check_short_of_the_budget_is_not_exhausted() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;
        set_checks(&db, "demo", CF_CHECK_BUDGET - 1).await;

        assert!(!cloudflare_checks_exhausted(&db, "demo").await.unwrap());
    }

    #[tokio::test]
    async fn verification_is_stamped_only_for_the_address_it_was_read_for() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;
        attach_destination(&db, "demo", "addr-new", None, NOW)
            .await
            .unwrap();

        mark_cloudflare_verified(&db, "demo", "addr-old", NOW + 1)
            .await
            .unwrap();
        assert_eq!(stored(&db, "demo").await.cf_verified_at, None);

        mark_cloudflare_verified(&db, "demo", "addr-new", NOW + 2)
            .await
            .unwrap();
        assert_eq!(stored(&db, "demo").await.cf_verified_at, Some(NOW + 2));
    }

    #[tokio::test]
    async fn clearing_a_claim_drops_it_and_clearing_a_missing_one_is_fine() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;

        clear_claim(&db, "demo").await.unwrap();
        clear_claim(&db, "demo").await.unwrap();

        assert_eq!(claim_by_handle(&db, "demo").await.unwrap(), None);
    }
}

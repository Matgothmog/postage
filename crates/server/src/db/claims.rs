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

/// Starts a claim only if the handle is still free to this wallet, deciding
/// that in the same statement that writes it; `false` when it is not.
///
/// Free means what `/api/inbox` checks before it gets this far: no inbox
/// held by another wallet, and no claim by another wallet that has either
/// had its code answered or not yet expired. The TypeScript checked that and
/// then wrote unconditionally, so two wallets could both pass the check and
/// the later write took the handle, and a claim promoted to an inbox between
/// the check and the write was overwritten. Here a write that lost that race
/// changes nothing.
///
/// `code_verified_at` is set by the short signup, whose address Privy has
/// already confirmed. Writing it here rather than in a second statement leaves
/// no moment in which the claim exists unverified and another request can
/// start it over.
pub async fn start_claim_if_free(
    db: &Db,
    claim: &NewClaim,
    code_verified_at: Option<i64>,
    now: i64,
) -> Result<bool, DbError> {
    let started = db
        .run(
            "INSERT INTO inbox_claims
       (handle, destination, wallet, code_hash, expires_at, attempts, code_verified_at,
        cf_address_id, cf_verified_at, created_at)
     SELECT ?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9
     WHERE NOT EXISTS (SELECT 1 FROM inboxes WHERE handle = ?1 AND wallet IS NOT ?3)
     ON CONFLICT (handle) DO UPDATE SET
       destination = excluded.destination,
       wallet = excluded.wallet,
       code_hash = excluded.code_hash,
       expires_at = excluded.expires_at,
       attempts = 0,
       code_verified_at = excluded.code_verified_at,
       cf_address_id = excluded.cf_address_id,
       cf_verified_at = excluded.cf_verified_at,
       cf_checked_at = NULL,
       cf_checks = 0
     WHERE inbox_claims.wallet = excluded.wallet
        OR (inbox_claims.code_verified_at IS NULL AND inbox_claims.expires_at <= ?9)",
            params![
                claim.handle.to_lowercase(),
                claim.destination.to_lowercase(),
                claim.wallet.to_lowercase(),
                claim.code_hash.as_str(),
                claim.expires_at,
                code_verified_at,
                claim.cf_address_id.as_deref(),
                claim.cf_verified_at,
                now
            ],
        )
        .await?;
    Ok(started > 0)
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

/// The ceilings [`record_claim_send_within`] holds a new send under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimSendLimits {
    pub window_seconds: i64,
    pub per_destination: i64,
    pub per_wallet: i64,
}

/// Records a send only while both counts are still under their ceilings, and
/// says whether it did. One statement counts and writes, so a burst of
/// concurrent claims cannot all read the same counts and all send; the
/// TypeScript counted and then wrote, and every request in such a burst got
/// its mail out.
pub async fn record_claim_send_within(
    db: &Db,
    destination: &str,
    wallet: &str,
    limits: ClaimSendLimits,
    now: i64,
) -> Result<bool, DbError> {
    let recorded = db
        .run(
            "INSERT INTO claim_sends (destination, wallet, sent_at)
     SELECT ?1, ?2, ?3
     WHERE (SELECT COUNT(*) FROM claim_sends WHERE destination = ?1 AND sent_at > ?4) < ?5
       AND (SELECT COUNT(*) FROM claim_sends WHERE wallet = ?2 AND sent_at > ?4) < ?6",
            params![
                destination.to_lowercase(),
                wallet.to_lowercase(),
                now,
                now - limits.window_seconds,
                limits.per_destination,
                limits.per_wallet
            ],
        )
        .await?;
    Ok(recorded > 0)
}

/// Points the claim at a registered Cloudflare address, but only while it is
/// still the claim that was started with `code_hash`. Every start writes a
/// fresh hash, so a request that started the claim cannot attach its address
/// to a claim a later request has since started over.
pub async fn attach_destination_to_claim(
    db: &Db,
    handle: &str,
    code_hash: &str,
    address_id: &str,
    verified_at: Option<i64>,
    now: i64,
) -> Result<(), DbError> {
    db.run(
        "UPDATE inbox_claims
     SET cf_address_id = ?, cf_verified_at = ?, cf_checked_at = ?, cf_checks = cf_checks + 1
     WHERE handle = ? AND code_hash = ?",
        params![
            address_id,
            verified_at,
            now,
            handle.to_lowercase(),
            code_hash
        ],
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

/// What became of an attempt taken against one particular claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    Taken,
    /// The claim is still the one asked about and has no guesses left.
    Exhausted,
    /// The claim was started over, taken by another wallet, or has expired
    /// since it was read, so a guess against it would be credited elsewhere.
    ClaimMoved,
}

/// [`consume_attempt`] for the claim a request has already read: the guess is
/// counted only while that exact claim, the same code and wallet and not yet
/// expired, is still what the handle holds. Otherwise a claim started over
/// between the read and the write would be charged a guess it never saw, and
/// the code would be judged against a claim that no longer exists.
pub async fn consume_attempt_on_claim(
    db: &Db,
    claim: &InboxClaim,
    max: i64,
    now: i64,
) -> Result<AttemptOutcome, DbError> {
    let consumed = db
        .run(
            "UPDATE inbox_claims SET attempts = attempts + 1
     WHERE handle = ? AND code_hash = ? AND wallet = ? AND expires_at > ? AND attempts < ?",
            params![
                claim.handle.to_lowercase(),
                claim.code_hash.as_str(),
                claim.wallet.to_lowercase(),
                now,
                max
            ],
        )
        .await?;
    if consumed > 0 {
        return Ok(AttemptOutcome::Taken);
    }
    // Only names the refusal; the guess was already refused above.
    let still_this_claim = claim_by_handle(db, &claim.handle)
        .await?
        .is_some_and(|held| {
            held.code_hash == claim.code_hash
                && held.wallet == claim.wallet
                && held.expires_at > now
        });
    Ok(if still_this_claim {
        AttemptOutcome::Exhausted
    } else {
        AttemptOutcome::ClaimMoved
    })
}

pub async fn mark_code_verified(db: &Db, handle: &str, now: i64) -> Result<(), DbError> {
    db.run(
        "UPDATE inbox_claims SET code_verified_at = ? WHERE handle = ? AND code_verified_at IS NULL",
        params![now, handle.to_lowercase()],
    )
    .await?;
    Ok(())
}

/// [`mark_code_verified`] for the claim whose code was just checked, and
/// `false` when that claim is no longer what the handle holds. A claim that
/// expired and was restarted by another wallet must not inherit a proof it
/// never made. Already-verified counts as success, and keeps its first time.
pub async fn mark_code_verified_on_claim(
    db: &Db,
    claim: &InboxClaim,
    now: i64,
) -> Result<bool, DbError> {
    let marked = db
        .run(
            "UPDATE inbox_claims SET code_verified_at = COALESCE(code_verified_at, ?)
     WHERE handle = ? AND code_hash = ? AND wallet = ? AND expires_at > ?",
            params![
                now,
                claim.handle.to_lowercase(),
                claim.code_hash.as_str(),
                claim.wallet.to_lowercase(),
                now
            ],
        )
        .await?;
    Ok(marked > 0)
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

    fn other_wallet() -> String {
        format!("0x{}", "22".repeat(20))
    }

    async fn start_if_free(db: &Db, wallet: &str, now: i64) -> bool {
        start_claim_if_free(db, &new_claim("demo", "x@example.com", wallet), None, now)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_free_handle_is_started_and_its_own_wallet_may_start_it_over() {
        let db = TestDb::fresh().await;

        assert!(start_if_free(&db, &wallet(), NOW).await);
        assert!(start_if_free(&db, &wallet(), NOW + 1).await);
        assert_eq!(stored(&db, "demo").await.wallet, wallet());
    }

    #[tokio::test]
    async fn another_wallets_live_claim_is_not_overwritten() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;

        assert!(!start_if_free(&db, &other_wallet(), NOW).await);
        assert_eq!(stored(&db, "demo").await.wallet, wallet());
    }

    #[tokio::test]
    async fn another_wallets_claim_is_released_exactly_when_it_expires() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;

        assert!(!start_if_free(&db, &other_wallet(), NOW + 899).await);
        assert!(start_if_free(&db, &other_wallet(), NOW + 900).await);
        assert_eq!(stored(&db, "demo").await.wallet, other_wallet());
    }

    #[tokio::test]
    async fn an_answered_claim_is_held_even_after_it_expires() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;
        mark_code_verified(&db, "demo", NOW).await.unwrap();

        assert!(!start_if_free(&db, &other_wallet(), NOW + 10 * HOUR).await);
    }

    #[tokio::test]
    async fn a_handle_that_is_another_wallets_inbox_is_not_claimed() {
        let db = TestDb::fresh().await;
        crate::db::inboxes::create_inbox(&db, "demo", "o@example.com", Some(&wallet()), NOW)
            .await
            .unwrap();

        assert!(!start_if_free(&db, &other_wallet(), NOW).await);
        assert!(
            start_if_free(&db, &wallet(), NOW).await,
            "its owner may repoint it"
        );
    }

    #[tokio::test]
    async fn an_inbox_with_no_wallet_is_nobodys_to_claim() {
        let db = TestDb::fresh().await;
        crate::db::inboxes::create_inbox(&db, "demo", "o@example.com", None, NOW)
            .await
            .unwrap();

        assert!(!start_if_free(&db, &wallet(), NOW).await);
    }

    #[tokio::test]
    async fn the_short_signup_starts_its_claim_already_verified() {
        let db = TestDb::fresh().await;

        start_claim_if_free(
            &db,
            &new_claim("demo", "x@example.com", &wallet()),
            Some(NOW),
            NOW,
        )
        .await
        .unwrap();

        assert_eq!(stored(&db, "demo").await.code_verified_at, Some(NOW));
    }

    /// Two wallets racing for one handle: whichever writes first holds it,
    /// and the other changes nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn of_many_wallets_racing_for_one_handle_exactly_one_starts_a_claim() {
        let db = Arc::new(TestDb::fresh().await);
        let racers: Vec<_> = (1..=16_u8)
            .map(|n| {
                let db = db.clone();
                tokio::spawn(async move {
                    let wallet = format!("0x{}", format!("{n:02x}").repeat(20));
                    start_if_free(&db, &wallet, NOW).await
                })
            })
            .collect();
        let mut started = 0;
        for racer in racers {
            if racer.await.unwrap() {
                started += 1;
            }
        }

        assert_eq!(started, 1);
    }

    fn limits() -> ClaimSendLimits {
        ClaimSendLimits {
            window_seconds: HOUR,
            per_destination: 3,
            per_wallet: 5,
        }
    }

    #[tokio::test]
    async fn a_send_is_recorded_until_either_ceiling_is_reached() {
        let db = TestDb::fresh().await;
        for _ in 0..3 {
            assert!(
                record_claim_send_within(&db, "x@example.com", &wallet(), limits(), NOW)
                    .await
                    .unwrap()
            );
        }
        assert!(
            !record_claim_send_within(&db, "X@example.com", &other_wallet(), limits(), NOW)
                .await
                .unwrap()
        );

        for n in 0..2 {
            let to = format!("y{n}@example.com");
            assert!(
                record_claim_send_within(&db, &to, &wallet(), limits(), NOW)
                    .await
                    .unwrap()
            );
        }
        assert!(
            !record_claim_send_within(&db, "z@example.com", &wallet(), limits(), NOW)
                .await
                .unwrap()
        );
        assert_eq!(
            recent_claims_from(&db, &wallet(), HOUR, NOW).await.unwrap(),
            5
        );
    }

    #[tokio::test]
    async fn sends_outside_the_window_do_not_count_toward_a_ceiling() {
        let db = TestDb::fresh().await;
        for _ in 0..3 {
            record_claim_send(&db, "x@example.com", &wallet(), NOW - HOUR)
                .await
                .unwrap();
        }

        assert!(
            record_claim_send_within(&db, "x@example.com", &wallet(), limits(), NOW)
                .await
                .unwrap()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_burst_of_concurrent_sends_from_one_wallet_stops_at_its_ceiling() {
        let db = Arc::new(TestDb::fresh().await);
        let sends: Vec<_> = (0..20)
            .map(|n| {
                let db = db.clone();
                tokio::spawn(async move {
                    let to = format!("v{n}@example.com");
                    record_claim_send_within(&db, &to, &wallet(), limits(), NOW)
                        .await
                        .unwrap()
                })
            })
            .collect();
        let mut recorded = 0;
        for send in sends {
            if send.await.unwrap() {
                recorded += 1;
            }
        }

        assert_eq!(recorded, 5);
    }

    #[tokio::test]
    async fn an_address_is_attached_only_to_the_claim_it_was_registered_for() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;

        attach_destination_to_claim(&db, "DEMO", "someone-elses-hash", "addr-1", Some(NOW), NOW)
            .await
            .unwrap();
        assert_eq!(stored(&db, "demo").await.cf_address_id, None);

        attach_destination_to_claim(&db, "demo", "deadbeef", "addr-1", Some(NOW), NOW)
            .await
            .unwrap();
        let attached = stored(&db, "demo").await;
        assert_eq!(attached.cf_address_id.as_deref(), Some("addr-1"));
        assert_eq!(attached.cf_checks, 1);
    }

    /// The claim the first wallet read, then the handle taken over by another
    /// wallet once it had expired.
    async fn claim_taken_over_after_expiry(db: &Db) -> (InboxClaim, InboxClaim) {
        claim(db, "demo").await;
        let read = stored(db, "demo").await;
        let mut takeover = new_claim("demo", "thief@example.com", &other_wallet());
        takeover.code_hash = "thieves-hash".to_owned();
        assert!(
            start_claim_if_free(db, &takeover, None, read.expires_at)
                .await
                .unwrap()
        );
        (read, stored(db, "demo").await)
    }

    #[tokio::test]
    async fn a_verified_mark_for_a_replaced_claim_is_refused_and_leaves_the_new_one_unproven() {
        let db = TestDb::fresh().await;
        let (read, _) = claim_taken_over_after_expiry(&db).await;

        let marked = mark_code_verified_on_claim(&db, &read, read.expires_at - 1)
            .await
            .unwrap();

        assert!(!marked);
        assert_eq!(stored(&db, "demo").await.code_verified_at, None);
    }

    #[tokio::test]
    async fn a_verified_mark_for_the_current_claim_sticks_with_its_first_time() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;
        let read = stored(&db, "demo").await;

        assert!(
            mark_code_verified_on_claim(&db, &read, NOW + 1)
                .await
                .unwrap()
        );
        assert!(
            mark_code_verified_on_claim(&db, &read, NOW + 2)
                .await
                .unwrap()
        );

        assert_eq!(stored(&db, "demo").await.code_verified_at, Some(NOW + 1));
    }

    #[tokio::test]
    async fn a_verified_mark_is_refused_for_a_claim_that_has_expired() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;
        let read = stored(&db, "demo").await;

        let marked = mark_code_verified_on_claim(&db, &read, read.expires_at)
            .await
            .unwrap();

        assert!(!marked);
        assert_eq!(stored(&db, "demo").await.code_verified_at, None);
    }

    #[tokio::test]
    async fn an_attempt_against_a_replaced_claim_is_not_charged_to_its_replacement() {
        let db = TestDb::fresh().await;
        let (read, _) = claim_taken_over_after_expiry(&db).await;

        let outcome = consume_attempt_on_claim(&db, &read, 5, read.expires_at - 1)
            .await
            .unwrap();

        assert_eq!(outcome, AttemptOutcome::ClaimMoved);
        assert_eq!(stored(&db, "demo").await.attempts, 0);
    }

    #[tokio::test]
    async fn attempts_on_the_current_claim_run_out_as_exhausted_not_moved() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;
        let read = stored(&db, "demo").await;

        for _ in 0..2 {
            let taken = consume_attempt_on_claim(&db, &read, 2, NOW).await.unwrap();
            assert_eq!(taken, AttemptOutcome::Taken);
        }
        let spent = consume_attempt_on_claim(&db, &read, 2, NOW).await.unwrap();

        assert_eq!(spent, AttemptOutcome::Exhausted);
    }

    #[tokio::test]
    async fn an_attempt_on_an_expired_or_missing_claim_is_a_moved_claim() {
        let db = TestDb::fresh().await;
        claim(&db, "demo").await;
        let read = stored(&db, "demo").await;

        let expired = consume_attempt_on_claim(&db, &read, 5, read.expires_at)
            .await
            .unwrap();
        clear_claim(&db, "demo").await.unwrap();
        let missing = consume_attempt_on_claim(&db, &read, 5, NOW).await.unwrap();

        assert_eq!(expired, AttemptOutcome::ClaimMoved);
        assert_eq!(missing, AttemptOutcome::ClaimMoved);
    }

    /// A guess racing a takeover is either counted against the claim it read
    /// before the takeover, or refused; it never lands on the replacement.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn racing_a_takeover_never_charges_or_verifies_the_replacement() {
        for round in 0..20 {
            let first = Arc::new(TestDb::fresh().await);
            let second = Arc::new(first.second_handle().await);
            claim(&first, "demo").await;
            let read = stored(&first, "demo").await;
            let at = read.expires_at - 1;
            let mut takeover = new_claim("demo", "thief@example.com", &other_wallet());
            takeover.code_hash = format!("thieves-hash-{round}");

            let guesser = {
                let (db, read) = (Arc::clone(&first), read.clone());
                tokio::spawn(async move {
                    let outcome = consume_attempt_on_claim(&db, &read, 5, at).await.unwrap();
                    let marked = mark_code_verified_on_claim(&db, &read, at).await.unwrap();
                    (outcome, marked)
                })
            };
            let taker = {
                let db = Arc::clone(&second);
                tokio::spawn(async move {
                    // Released at the moment the read claim expires.
                    start_claim_if_free(&db, &takeover, None, read.expires_at)
                        .await
                        .unwrap()
                })
            };
            let (outcome, marked) = guesser.await.unwrap();
            let took_over = taker.await.unwrap();

            let held = stored(&first, "demo").await;
            if held.wallet == other_wallet() {
                assert!(took_over);
                assert_eq!(held.attempts, 0, "round {round}");
                assert_eq!(held.code_verified_at, None, "round {round}");
                assert!(outcome == AttemptOutcome::Taken || outcome == AttemptOutcome::ClaimMoved);
                assert!(!marked || outcome == AttemptOutcome::Taken);
            } else {
                assert_eq!(outcome, AttemptOutcome::Taken);
                assert!(marked);
            }
        }
    }
}

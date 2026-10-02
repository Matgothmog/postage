//! The hourly budget for reading mail with a model.
//!
//! One row per message we paid a model to read, kept only long enough to cap
//! what a flood can cost. `now` comes from the caller; nothing here reads a
//! clock.

use libsql::params;
use serde::Deserialize;

use super::{Db, DbError};

/// What one handle, one domain writing to it, and one address at that domain
/// may cost in model calls per hour. Reading every message is what makes the
/// gate work, but nothing else stands between a flood and an unbounded bill,
/// because classification now happens before any pass is consulted.
///
/// Three ceilings rather than two, and the middle one is why. Only mail the
/// receiving server could authenticate may spend any of this, and
/// authentication is a statement about a domain - so a domain is the smallest
/// unit of a sender that cannot be cycled for nothing. A local part is free to
/// invent; a domain that passes DKIM costs a registration and a published key.
/// Keyed on the address alone, one party emptied a handle's whole hour by
/// inventing ten local parts, and everything downstream that read "this pool is
/// empty" as "somebody else's doing" was reading a state that party had
/// arranged. Keyed on the domain as well, emptying the pool takes as many
/// separate registered domains as the ratio between these two numbers.
pub const CLASSIFY_PER_HANDLE_HOURLY: i64 = 200;
pub const CLASSIFY_PER_DOMAIN_HOURLY: i64 = 60;
pub const CLASSIFY_PER_SENDER_HOURLY: i64 = 20;

/// The one window all three hourly ceilings above are counted over.
const CLASSIFY_WINDOW_SECONDS: i64 = 60 * 60;

/// Why a message was not read, when it was not.
///
/// Which ceiling bit matters, because the three are reached by different
/// people. An address's own slice is spent by that address alone, so anything
/// it unlocks is something that sender chose. A domain's slice rations a cost
/// across everyone who writes from one name - it is the unit a flood is paid
/// for in, but it is not a culprit: `gmail.com` is millions of unrelated
/// people. A handle's pool is spent by every domain writing to that inbox
/// between them.
///
/// None of the three buys a delivery: a budget that ran out is held apart from
/// a model that failed, and only the second is an outage this gateway owes
/// anyone free mail for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetState {
    SpentBySender,
    SpentByDomain,
    SpentByHandle,
}

/// Drops rows past the window they are counted over. Called on the way past,
/// like the held-message purge, rather than left to grow a row per message
/// forever under a COUNT that every inbound message pays for.
pub async fn purge_old_classifications(db: &Db, now: i64) -> Result<(), DbError> {
    db.run(
        "DELETE FROM classifications WHERE at <= ?",
        params![now - CLASSIFY_WINDOW_SECONDS],
    )
    .await?;
    Ok(())
}

/// Everything from the last `@` on - `"@example.com"` for `"a@example.com"`.
/// The separator is the last `@` and not the first, because a quoted local part
/// may contain one and a domain may not.
///
/// Kept as a suffix including the `@` so that the comparison below is an exact
/// tail match: `"@example.com"` cannot be reached from `"notexample.com"`, and
/// a subdomain is a different name that is counted as one.
///
/// An address with no `@` at all is its own suffix, so it buckets with itself
/// rather than being handed a domain's allowance it never named.
fn domain_suffix(sender: &str) -> &str {
    sender.rfind('@').map_or(sender, |at| &sender[at..])
}

/// Takes a slice of the hourly budget. `None` means there was one to take;
/// otherwise the ceiling that refused it.
///
/// Counting and recording are one statement on purpose. Read-then-write lets a
/// burst - which is the case the budget exists for - all see room and all spend
/// it, so the real ceiling becomes the limit plus however many arrived at once.
/// That statement is also the only place the thresholds are compared, so asking
/// first would be the same rule written twice with a race between them.
pub async fn claim_classification(
    db: &Db,
    handle: &str,
    sender: &str,
    now: i64,
) -> Result<Option<BudgetState>, DbError> {
    if take_classification_slot(db, handle, sender, now).await? {
        return Ok(None);
    }
    which_limit_bit(db, handle, sender, now).await.map(Some)
}

#[derive(Debug, Deserialize)]
struct Spent {
    #[serde(rename = "forSender")]
    for_sender: Option<i64>,
    #[serde(rename = "forDomain")]
    for_domain: Option<i64>,
}

/// Which of the three ceilings refused the slot, narrowest first. Looked up
/// rather than assumed: reporting a sender's own exhaustion as the handle's
/// would unlock the things that state exists to shut.
async fn which_limit_bit(
    db: &Db,
    handle: &str,
    sender: &str,
    now: i64,
) -> Result<BudgetState, DbError> {
    let writer = sender.to_lowercase();
    let domain = domain_suffix(&writer);
    let rows: Vec<Spent> = db
        .all(
            "SELECT SUM(CASE WHEN sender = ? THEN 1 ELSE 0 END) AS forSender,
            SUM(CASE WHEN substr(sender, -length(?)) = ? THEN 1 ELSE 0 END) AS forDomain
     FROM classifications WHERE handle = ? AND at > ?",
            params![
                writer.as_str(),
                domain,
                domain,
                handle.to_lowercase(),
                now - CLASSIFY_WINDOW_SECONDS
            ],
        )
        .await?;

    // SUM over no rows is NULL, which the ceilings treat as zero.
    let row = rows.first();
    if row.and_then(|row| row.for_sender).unwrap_or(0) >= CLASSIFY_PER_SENDER_HOURLY {
        return Ok(BudgetState::SpentBySender);
    }
    if row.and_then(|row| row.for_domain).unwrap_or(0) >= CLASSIFY_PER_DOMAIN_HOURLY {
        return Ok(BudgetState::SpentByDomain);
    }
    Ok(BudgetState::SpentByHandle)
}

/// Hands back the most recent slot taken by this pair, for work that never
/// reached the model.
pub async fn release_classification_slot(
    db: &Db,
    handle: &str,
    sender: &str,
) -> Result<(), DbError> {
    db.run(
        "DELETE FROM classifications
     WHERE id = (SELECT id FROM classifications
                 WHERE handle = ? AND sender = ? ORDER BY at DESC, id DESC LIMIT 1)",
        params![handle.to_lowercase(), sender.to_lowercase()],
    )
    .await?;
    Ok(())
}

async fn take_classification_slot(
    db: &Db,
    handle: &str,
    sender: &str,
    now: i64,
) -> Result<bool, DbError> {
    let since = now - CLASSIFY_WINDOW_SECONDS;
    let inbox = handle.to_lowercase();
    let writer = sender.to_lowercase();
    let domain = domain_suffix(&writer);

    // The per-domain count is a tail comparison rather than a stored column:
    // the domain is derivable from what is already recorded, and the rows this
    // reads are one handle's single hour, which the purge keeps small.
    let claimed = db
        .run(
            "INSERT INTO classifications (handle, sender, at)
          SELECT ?, ?, ?
          WHERE (SELECT COUNT(*) FROM classifications
                 WHERE handle = ? AND at > ?) < ?
            AND (SELECT COUNT(*) FROM classifications
                 WHERE handle = ? AND at > ? AND substr(sender, -length(?)) = ?) < ?
            AND (SELECT COUNT(*) FROM classifications
                 WHERE handle = ? AND sender = ? AND at > ?) < ?",
            params![
                inbox.as_str(),
                writer.as_str(),
                now,
                inbox.as_str(),
                since,
                CLASSIFY_PER_HANDLE_HOURLY,
                inbox.as_str(),
                since,
                domain,
                domain,
                CLASSIFY_PER_DOMAIN_HOURLY,
                inbox.as_str(),
                writer.as_str(),
                since,
                CLASSIFY_PER_SENDER_HOURLY
            ],
        )
        .await?;
    Ok(claimed > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::TestDb;
    use crate::db::{Statement, Value};

    const NOW: i64 = 1_760_000_000;

    async fn claim(db: &Db, sender: &str) -> Option<BudgetState> {
        claim_classification(db, "demo", sender, NOW).await.unwrap()
    }

    /// Writes the slots a run of claims would have taken in one transaction.
    /// The limits are what is under test, not how many fsyncs a few hundred
    /// single inserts cost, which on a busy disk outlasts the busy timeout.
    async fn seed(db: &Db, senders: impl Iterator<Item = String>) {
        let statements = senders
            .map(|sender| {
                Statement::new(
                    "INSERT INTO classifications (handle, sender, at) VALUES (?, ?, ?)",
                    vec![Value::from("demo"), Value::from(sender), Value::from(NOW)],
                )
            })
            .collect();
        db.batch(statements).await.unwrap();
    }

    async fn spend(db: &Db, times: i64, sender: &str) {
        seed(db, (0..times).map(|_| sender.to_owned())).await;
    }

    /// One address per slot, so the per-sender ceiling never binds before the
    /// per-domain one the test is actually about.
    async fn spend_across_domain(db: &Db, times: i64, domain: &str) {
        seed(db, (0..times).map(|n| format!("writer{n}@{domain}"))).await;
    }

    /// Empties a handle's whole hour without any single domain having done it,
    /// which is the only way it can be emptied at all: each domain is filled to
    /// its own ceiling and the next one takes over.
    async fn drain_handle(db: &Db, slots: i64) {
        seed(
            db,
            (0..slots).map(|taken| {
                let domain = taken / CLASSIFY_PER_DOMAIN_HOURLY;
                format!("writer{taken}@drain{domain}.example")
            }),
        )
        .await;
    }

    async fn rows_for(db: &Db, sender: &str) -> i64 {
        #[derive(Deserialize)]
        struct Count {
            n: i64,
        }
        let rows: Vec<Count> = db
            .all(
                "SELECT COUNT(*) AS n FROM classifications WHERE sender = ?",
                params![sender],
            )
            .await
            .unwrap();
        rows[0].n
    }

    /// The distinction the whole degraded path rests on. A sender spending
    /// their own slice chose to; a handle's pool is spent by whoever writes to
    /// that inbox. Collapsing the two either hands a sender a way to switch off
    /// their own classification, or blames a recipient's own mail on them.
    #[tokio::test]
    async fn a_sender_who_spends_their_own_slice_is_told_it_was_theirs() {
        let db = TestDb::fresh().await;
        spend(&db, CLASSIFY_PER_SENDER_HOURLY, "greedy@x.com").await;

        assert_eq!(
            claim(&db, "greedy@x.com").await,
            Some(BudgetState::SpentBySender)
        );
    }

    #[tokio::test]
    async fn a_handle_drained_by_many_domains_is_not_blamed_on_the_next_sender() {
        let db = TestDb::fresh().await;
        drain_handle(&db, CLASSIFY_PER_HANDLE_HOURLY).await;

        assert_eq!(
            claim(&db, "innocent@x.com").await,
            Some(BudgetState::SpentByHandle),
            "a stranger's flood must not read as this sender's own doing"
        );
    }

    /// The ceiling that makes the outer one hard to reach: without it one
    /// domain could empty a handle's whole hour on its own.
    #[tokio::test]
    async fn one_domain_cannot_spend_more_than_its_own_share_of_a_handles_hour() {
        let db = TestDb::fresh().await;
        spend_across_domain(&db, CLASSIFY_PER_DOMAIN_HOURLY, "flood.example").await;

        assert_eq!(
            claim(&db, "another@flood.example").await,
            Some(BudgetState::SpentByDomain),
            "a fresh local part at a spent domain must not open a fresh allowance"
        );
    }

    #[tokio::test]
    async fn a_domain_that_spent_its_share_leaves_the_rest_of_the_hour_to_everyone_else() {
        let db = TestDb::fresh().await;
        spend_across_domain(&db, CLASSIFY_PER_DOMAIN_HOURLY, "flood.example").await;

        assert_eq!(
            claim(&db, "quiet@elsewhere.example").await,
            None,
            "an unrelated domain must still be read"
        );
    }

    #[tokio::test]
    async fn a_sender_who_spends_their_own_slice_does_not_spend_their_neighbours() {
        let db = TestDb::fresh().await;
        spend(&db, CLASSIFY_PER_SENDER_HOURLY, "greedy@shared.example").await;

        assert_eq!(claim(&db, "neighbour@shared.example").await, None);
    }

    /// Guards the suffix the per-domain count is matched on. Counting every
    /// address that merely ends the same way would let anyone drain
    /// `example.com`'s allowance from `notexample.com`.
    #[tokio::test]
    async fn a_domains_ceiling_counts_that_domain_not_one_whose_name_ends_the_same_way() {
        let db = TestDb::fresh().await;
        spend_across_domain(&db, CLASSIFY_PER_DOMAIN_HOURLY, "example.com").await;

        assert_eq!(claim(&db, "someone@notexample.com").await, None);
    }

    /// A subdomain is a separate name with a separate key, and is counted as one.
    #[tokio::test]
    async fn a_subdomain_does_not_draw_on_the_allowance_of_the_domain_below_it() {
        let db = TestDb::fresh().await;
        spend_across_domain(&db, CLASSIFY_PER_DOMAIN_HOURLY, "example.com").await;

        assert_eq!(claim(&db, "someone@mail.example.com").await, None);
    }

    #[tokio::test]
    async fn the_last_slot_inside_a_senders_own_limit_is_still_granted() {
        let db = TestDb::fresh().await;
        spend(&db, CLASSIFY_PER_SENDER_HOURLY - 1, "chatty@x.com").await;

        assert_eq!(
            claim(&db, "chatty@x.com").await,
            None,
            "the limit is inclusive"
        );
        assert_eq!(
            claim(&db, "chatty@x.com").await,
            Some(BudgetState::SpentBySender)
        );
    }

    #[tokio::test]
    async fn the_last_slot_inside_a_domains_limit_is_still_granted() {
        let db = TestDb::fresh().await;
        spend_across_domain(&db, CLASSIFY_PER_DOMAIN_HOURLY - 1, "busy.example").await;

        assert_eq!(
            claim(&db, "last@busy.example").await,
            None,
            "the limit is inclusive"
        );
        assert_eq!(
            claim(&db, "over@busy.example").await,
            Some(BudgetState::SpentByDomain)
        );
    }

    #[tokio::test]
    async fn the_last_slot_inside_a_handles_pool_is_still_granted() {
        let db = TestDb::fresh().await;
        drain_handle(&db, CLASSIFY_PER_HANDLE_HOURLY - 1).await;

        assert_eq!(
            claim(&db, "last@fresh.example").await,
            None,
            "the limit is inclusive"
        );
        assert_eq!(
            claim(&db, "over@later.example").await,
            Some(BudgetState::SpentByHandle)
        );
    }

    #[tokio::test]
    async fn room_left_reads_as_room_left() {
        let db = TestDb::fresh().await;

        assert_eq!(claim(&db, "quiet@x.com").await, None);
    }

    #[tokio::test]
    async fn a_slot_handed_back_can_be_taken_again() {
        let db = TestDb::fresh().await;
        spend(&db, CLASSIFY_PER_SENDER_HOURLY, "retry@x.com").await;
        assert_eq!(
            claim(&db, "retry@x.com").await,
            Some(BudgetState::SpentBySender)
        );

        release_classification_slot(&db, "demo", "retry@x.com")
            .await
            .unwrap();

        assert_eq!(
            claim(&db, "retry@x.com").await,
            None,
            "work that never reached the model must not cost the sender their hour"
        );
    }

    #[tokio::test]
    async fn releasing_removes_only_the_most_recent_slot_of_that_pair() {
        let db = TestDb::fresh().await;
        for offset in 0..3 {
            claim_classification(&db, "demo", "a@x.com", NOW + offset)
                .await
                .unwrap();
        }
        claim(&db, "b@x.com").await;

        release_classification_slot(&db, "DEMO", "A@x.com")
            .await
            .unwrap();

        assert_eq!(rows_for(&db, "a@x.com").await, 2);
        assert_eq!(rows_for(&db, "b@x.com").await, 1);
    }

    #[tokio::test]
    async fn releasing_a_slot_nobody_took_is_a_no_op() {
        let db = TestDb::fresh().await;

        release_classification_slot(&db, "demo", "ghost@x.com")
            .await
            .unwrap();

        assert_eq!(rows_for(&db, "ghost@x.com").await, 0);
    }

    #[tokio::test]
    async fn slots_older_than_the_hour_stop_counting() {
        let db = TestDb::fresh().await;
        for _ in 0..CLASSIFY_PER_SENDER_HOURLY {
            claim_classification(&db, "demo", "old@x.com", NOW)
                .await
                .unwrap();
        }

        let at_the_edge =
            claim_classification(&db, "demo", "old@x.com", NOW + CLASSIFY_WINDOW_SECONDS)
                .await
                .unwrap();
        let just_inside =
            claim_classification(&db, "demo", "old@x.com", NOW + CLASSIFY_WINDOW_SECONDS - 1)
                .await
                .unwrap();

        assert_eq!(
            at_the_edge, None,
            "a row exactly an hour old is outside the window"
        );
        assert_eq!(just_inside, Some(BudgetState::SpentBySender));
    }

    #[tokio::test]
    async fn purging_drops_rows_an_hour_old_and_keeps_newer_ones() {
        let db = TestDb::fresh().await;
        claim_classification(&db, "demo", "old@x.com", NOW - CLASSIFY_WINDOW_SECONDS)
            .await
            .unwrap();
        claim_classification(&db, "demo", "new@x.com", NOW - CLASSIFY_WINDOW_SECONDS + 1)
            .await
            .unwrap();

        purge_old_classifications(&db, NOW).await.unwrap();

        assert_eq!(rows_for(&db, "old@x.com").await, 0);
        assert_eq!(rows_for(&db, "new@x.com").await, 1);
    }

    #[test]
    fn the_domain_is_everything_from_the_last_at_sign() {
        assert_eq!(domain_suffix("a@example.com"), "@example.com");
        assert_eq!(domain_suffix("\"a@b\"@example.com"), "@example.com");
        assert_eq!(domain_suffix("no-at-sign"), "no-at-sign");
    }

    /// Every racer gets a connection of its own. A refused claim goes on to
    /// read, and a read in flight on a connection other tasks are writing
    /// through blocks that connection's own commit - a local-file artefact
    /// that is not what is under test.
    async fn race(db: &TestDb, tasks: usize, sender: impl Fn(usize) -> String) -> usize {
        let mut racers = Vec::new();
        for index in 0..tasks {
            let handle = db.second_handle().await;
            let sender = sender(index);
            racers.push(tokio::spawn(async move {
                claim_classification(&handle, "demo", &sender, NOW).await
            }));
        }
        let mut granted = 0;
        for racer in racers {
            if racer.await.unwrap().unwrap().is_none() {
                granted += 1;
            }
        }
        granted
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_burst_of_one_sender_never_exceeds_the_senders_ceiling() {
        let db = TestDb::fresh().await;

        let granted = race(&db, 24, |_| "burst@x.com".to_owned()).await;

        assert_eq!(granted, CLASSIFY_PER_SENDER_HOURLY as usize);
        assert_eq!(
            rows_for(&db, "burst@x.com").await,
            CLASSIFY_PER_SENDER_HOURLY
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_burst_across_one_domain_never_exceeds_the_domains_ceiling() {
        let db = TestDb::fresh().await;
        spend_across_domain(&db, CLASSIFY_PER_DOMAIN_HOURLY - 4, "one.example").await;

        let granted = race(&db, 24, |n| format!("w{n}@one.example")).await;

        assert_eq!(granted, 4);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_burst_across_many_domains_never_exceeds_the_handles_pool() {
        let db = TestDb::fresh().await;
        let near_the_limit = CLASSIFY_PER_HANDLE_HOURLY - 3;
        drain_handle(&db, near_the_limit).await;

        let granted = race(&db, 16, |n| format!("w{n}@burst{n}.example")).await;

        assert_eq!(granted, 3);
    }
}

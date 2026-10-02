//! Columns for tables that already exist somewhere without them.
//!
//! `CREATE TABLE IF NOT EXISTS` does nothing at all to a table that is already
//! there, so every column added after a database was first created is a column
//! that database never gets. `held_until` is the one that bit: a deployment
//! whose `challenges` table predates holding kept answering every held message
//! with a 500, because the statement meant to erase expired holds named a
//! column it did not have.
//!
//! Every column here is also in [`super::schema::SCHEMA`], so a database
//! created today is correct without running any of this. Listing it twice is
//! the price of the two cases being genuinely different: one describes the
//! shape, the other repairs a database that was made before the shape said so.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use super::{Db, DbError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AddedColumn {
    table: &'static str,
    column: &'static str,
    column_type: &'static str,
    backfill: Option<&'static str>,
}

const fn added(
    table: &'static str,
    column: &'static str,
    column_type: &'static str,
) -> AddedColumn {
    AddedColumn {
        table,
        column,
        column_type,
        backfill: None,
    }
}

const ADDED_COLUMNS: [AddedColumn; 9] = [
    AddedColumn {
        // Anything already settled has had whatever it was going to get.
        backfill: Some(
            "UPDATE challenges SET entitled_at = resolved_at WHERE resolved_at IS NOT NULL",
        ),
        ..added("challenges", "entitled_at", "INTEGER")
    },
    added("challenges", "settled_by", "TEXT"),
    AddedColumn {
        // Backfilled together with entitled_at, because a challenge settled
        // before either column existed has to read as closed on both counts.
        // Stamping one alone tells a legacy sender their message never arrived
        // and then refuses the paste box the answer sends them to.
        backfill: Some(
            "UPDATE challenges SET delivered_at = resolved_at WHERE resolved_at IS NOT NULL",
        ),
        ..added("challenges", "delivered_at", "INTEGER")
    },
    added("challenges", "held_until", "INTEGER"),
    added("inbox_claims", "cf_checked_at", "INTEGER"),
    added("inbox_claims", "cf_checks", "INTEGER NOT NULL DEFAULT 0"),
    // Left nullable: rows written before this cannot say which wallet started
    // them, and guessing would throttle a wallet for somebody else's claim.
    added("claim_sends", "wallet", "TEXT"),
    // No backfill: a database from before this column existed has no record of
    // which of its NULL-`uses_left` rows were purely earned versus paid for and
    // only ever extended, and there is no way to recover that after the fact.
    // Guessing "paid" for everything would make every pre-existing earned pass
    // permanently unrevokable.
    added("passes", "paid_extended_at", "INTEGER"),
    // Left nullable: a database from before this column existed has no record
    // of which wallet claimed a handle, and guessing one would send someone
    // else's earnings to the wrong owner.
    added("inboxes", "wallet", "TEXT"),
];

/// What a migration run left undone, so a caller (and a test) can see it rather
/// than only a log line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// Tables named in `ADDED_COLUMNS` that did not exist, and so got none of
    /// their columns.
    pub skipped_tables: Vec<&'static str>,
}

#[derive(Debug, Deserialize)]
struct ColumnName {
    name: String,
}

/// The columns a table has, or an empty set if there is no such table -
/// `PRAGMA table_info` answers a name it does not know with no rows rather
/// than an error, so the two cases arrive looking identical and are separated
/// by the caller.
async fn existing_columns(db: &Db, table: &'static str) -> Result<HashSet<String>, DbError> {
    let rows: Vec<ColumnName> = db.all(&format!("PRAGMA table_info({table})"), ()).await?;
    Ok(rows.into_iter().map(|row| row.name).collect())
}

/// SQLite's own words for a column that is already there. Matched on the
/// message rather than on the error code, which `no such table` shares - and
/// that one has to stay loud, because it means a table reached this before
/// anything created it.
fn is_duplicate_column(error: &DbError) -> bool {
    match error {
        DbError::Query { source, .. } => source
            .to_string()
            .to_ascii_lowercase()
            .contains("duplicate column name"),
        _ => false,
    }
}

/// Adds one column, treating a column that is already there as done.
///
/// Every instance of a deploy cold-starts at once and every one of them runs
/// this, so two can read `PRAGMA table_info` before either has altered
/// anything: both see the column missing, both alter, and the loser is handed
/// `duplicate column name`. Nothing else is swallowed.
async fn add_column(db: &Db, added: &AddedColumn) -> Result<(), DbError> {
    let AddedColumn {
        table,
        column,
        column_type,
        ..
    } = added;
    let alter = format!("ALTER TABLE {table} ADD COLUMN {column} {column_type}");
    match db.run(&alter, ()).await {
        Ok(_) => Ok(()),
        Err(error) if is_duplicate_column(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Adds every column in `ADDED_COLUMNS` that its table is missing, then runs
/// that column's backfill.
pub async fn add_missing_columns(db: &Db) -> Result<MigrationReport, DbError> {
    let mut known: HashMap<&'static str, HashSet<String>> = HashMap::new();
    let mut report = MigrationReport::default();

    for added in &ADDED_COLUMNS {
        let columns = match known.entry(added.table) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let columns = existing_columns(db, added.table).await?;
                if columns.is_empty() {
                    eprintln!(
                        "schema migration skipped a table nothing has created: {}",
                        added.table
                    );
                    report.skipped_tables.push(added.table);
                }
                entry.insert(columns)
            }
        };
        // A table with no columns does not exist yet. Skipping costs that one
        // table its added columns; falling through to `ALTER TABLE` would throw
        // `no such table` and cost every cold start from here on, with nothing
        // left able to repair it.
        if columns.is_empty() || columns.contains(added.column) {
            continue;
        }

        add_column(db, added).await?;
        // Run even when the column was already there a moment ago: the instance
        // that won the race may not have reached its own backfill yet, and every
        // one of these is an idempotent UPDATE over rows that are already
        // settled.
        if let Some(backfill) = added.backfill {
            db.run(backfill, ()).await?;
        }
        columns.insert(added.column.to_owned());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::super::inboxes;
    use super::super::schema::SCHEMA;
    use super::super::testing::{ColumnInfo, TestDb, columns_of};
    use super::*;

    /// The same missing-column shape as the outage, on `inboxes` rather than
    /// the `claim_sends` table that actually failed: a table created before
    /// `wallet` was ever added to it.
    const LEGACY_INBOXES: &str = "CREATE TABLE inboxes (
  handle TEXT PRIMARY KEY,
  destination TEXT NOT NULL,
  created_at INTEGER NOT NULL
)";

    /// Every table at its current shape except `inboxes`, which predates
    /// `wallet`. `absent_table` leaves one table out entirely, for the case
    /// where a phase mistake means the migration meets a table nothing has
    /// created.
    async fn legacy_database(absent_table: Option<&str>) -> TestDb {
        let test_db = TestDb::empty().await;
        for statement in SCHEMA {
            let sql = statement.sql();
            if sql.contains("inboxes") || absent_table.is_some_and(|table| sql.contains(table)) {
                continue;
            }
            test_db.run(sql, ()).await.unwrap();
        }
        test_db.run(LEGACY_INBOXES, ()).await.unwrap();
        let columns = columns_of(&test_db, "inboxes").await;
        assert!(
            !columns.iter().any(|column| column.name == "wallet"),
            "test setup: the table must start without wallet, the shape a real deployment's did"
        );
        test_db
    }

    fn wallet_column(columns: &[ColumnInfo]) -> Option<&ColumnInfo> {
        columns.iter().find(|column| column.name == "wallet")
    }

    #[tokio::test]
    async fn adds_inboxes_wallet_to_a_table_that_predates_the_column() {
        let test_db = legacy_database(None).await;

        add_missing_columns(&test_db).await.unwrap();

        let columns = columns_of(&test_db, "inboxes").await;
        let wallet = wallet_column(&columns).expect("wallet must exist once the migration has run");
        assert_eq!(
            wallet.notnull, 0,
            "wallet must stay nullable, matching the schema"
        );
        assert_eq!(
            wallet.dflt_value, None,
            "wallet must have no default, matching the schema"
        );
    }

    /// One cold start after another, which is the easy half. The racing case
    /// below is the half that bit.
    #[tokio::test]
    async fn does_not_error_the_second_time_it_runs() {
        let test_db = legacy_database(None).await;
        add_missing_columns(&test_db).await.unwrap();

        let second = add_missing_columns(&test_db).await;

        assert!(
            second.is_ok(),
            "a migration that already ran must be safe to run again on cold start: {second:?}"
        );
    }

    #[tokio::test]
    async fn queries_that_reference_wallet_succeed_once_the_column_has_been_added() {
        let test_db = legacy_database(None).await;
        add_missing_columns(&test_db).await.unwrap();

        inboxes::create_inbox(&test_db, "demo", "owner@example.com", Some("0xabc"), 1)
            .await
            .unwrap();

        let by_handle = inboxes::inbox_by_handle(&test_db, "demo").await.unwrap();
        assert_eq!(
            by_handle.and_then(|inbox| inbox.wallet).as_deref(),
            Some("0xabc"),
            "creating an inbox and reading it by handle must round-trip the wallet"
        );
        let by_wallet = inboxes::inbox_by_wallet(&test_db, "0xabc").await.unwrap();
        assert_eq!(
            by_wallet.map(|inbox| inbox.handle).as_deref(),
            Some("demo"),
            "reading by wallet must find the row it was just written to"
        );
    }

    /// The migration runs on the critical path of every cold start, and a
    /// deploy brings several instances up at once: two separate connections to
    /// one database, both reading the column as missing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_cold_starts_racing_to_add_the_same_column_both_come_up() {
        let racing = legacy_database(None).await;
        let other = racing.second_handle().await;

        let (first, second) =
            tokio::join!(add_missing_columns(&racing), add_missing_columns(&other));

        assert!(
            first.is_ok() && second.is_ok(),
            "a deploy boots several instances at once and only one of them can win the ALTER: \
             {first:?} {second:?}"
        );
        let columns = columns_of(&racing, "inboxes").await;
        assert!(
            wallet_column(&columns).is_some(),
            "the column still has to be there once the race is over"
        );
    }

    /// The race's losing half, made deterministic: the column is read as
    /// missing, then someone else adds it before this instance's `ALTER`.
    #[tokio::test]
    async fn the_loser_of_the_race_treats_a_duplicate_column_as_done() {
        let test_db = legacy_database(None).await;
        add_missing_columns(&test_db).await.unwrap();

        let wallet = ADDED_COLUMNS
            .iter()
            .find(|added| added.table == "inboxes" && added.column == "wallet")
            .unwrap();
        let outcome = add_column(&test_db, wallet).await;

        assert!(outcome.is_ok(), "{outcome:?}");
    }

    /// A table that lands in the wrong bootstrap phase reaches this with
    /// nothing created yet. `inbox_claims` is named in `ADDED_COLUMNS` before
    /// `inboxes`, so the cost of getting this wrong is every table after it,
    /// not just the one.
    #[tokio::test]
    async fn a_table_nothing_has_created_costs_its_own_columns_and_no_others() {
        let partial = legacy_database(Some("inbox_claims")).await;

        let report = add_missing_columns(&partial).await;

        let report =
            report.expect("one table nobody created must not cost every table listed after it");
        let columns = columns_of(&partial, "inboxes").await;
        assert!(
            wallet_column(&columns).is_some(),
            "the tables after the absent one still have to be migrated"
        );
        assert_eq!(
            report.skipped_tables,
            vec!["inbox_claims"],
            "a skipped table has to be said out loud, or it is a silent half-migration"
        );
    }
}

//! The one database handle every table module queries through: Turso over
//! HTTP in production, a local SQLite file in development and tests.
//!
//! Raw SQL with `?` parameters, as the TypeScript wrote it - no ORM. Rows come
//! back decoded into a caller's `#[derive(Deserialize)]` struct, so a row whose
//! shape does not match is an error at the query rather than a wrong value
//! somewhere downstream.

pub mod issued_contexts;
pub mod migrations;
pub mod nullifiers;
pub mod passes;
pub mod schema;
pub mod spent_nonces;
#[cfg(test)]
pub mod testing;

use std::borrow::Cow;
use std::fmt;
use std::time::Duration;

use libsql::params::IntoParams;
use libsql::{Connection, Database, TransactionBehavior};
use serde::de::DeserializeOwned;
use tokio::sync::OnceCell;

pub use libsql::Value;

/// Where `DATABASE_URL` points when it is unset: a file beside the app, the
/// same default the TypeScript had.
pub const DEFAULT_DATABASE_URL: &str = "file:.data/postage.db";

/// How long a local connection waits for another one's write lock before
/// giving up. Transactions open a connection of their own, so on a local file
/// two writers can genuinely meet; Turso serialises writes on its side.
const LOCAL_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// Carries only the scheme: a URL can embed credentials.
    #[error(
        "DATABASE_URL has an unsupported scheme \"{scheme}\"; expected file:, libsql:, https: or http:"
    )]
    UnsupportedUrl { scheme: String },
    #[error("opening the database failed: {0}")]
    Open(#[source] libsql::Error),
    /// Carries the SQL but never the parameters, which may be secrets.
    #[error("query failed: {source} (in `{sql}`)")]
    Query {
        sql: String,
        #[source]
        source: libsql::Error,
    },
    #[error("a row did not match the expected shape: {source} (in `{sql}`)")]
    Decode {
        sql: String,
        #[source]
        source: serde::de::value::Error,
    },
    #[error("reset() empties every table and is refused in production")]
    ResetRefused,
}

/// One statement of a [`Db::batch`].
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub sql: Cow<'static, str>,
    pub params: Vec<Value>,
}

impl Statement {
    pub fn new(sql: impl Into<Cow<'static, str>>, params: Vec<Value>) -> Self {
        Self {
            sql: sql.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Location {
    LocalFile,
    Remote,
}

pub struct Db {
    database: Database,
    /// Shared by every plain query. Transactions never use it: a `BEGIN` on a
    /// connection other requests are also using would sweep their statements
    /// into this transaction.
    connection: Connection,
    location: Location,
}

impl fmt::Debug for Db {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Db")
            .field("location", &self.location)
            .finish_non_exhaustive()
    }
}

impl Db {
    /// Opens a connection without touching the schema. `url` is
    /// `file:<path>` for a local file, or a `libsql://`/`https://` Turso URL.
    pub async fn connect(url: &str, auth_token: &str) -> Result<Self, DbError> {
        let (database, location) = match local_path(url) {
            Some(path) => (
                libsql::Builder::new_local(path).build().await,
                Location::LocalFile,
            ),
            None => {
                check_remote_scheme(url)?;
                (
                    libsql::Builder::new_remote(url.to_owned(), auth_token.to_owned())
                        .build()
                        .await,
                    Location::Remote,
                )
            }
        };
        let database = database.map_err(DbError::Open)?;
        let connection = new_connection(&database, location)?;
        Ok(Self {
            database,
            connection,
            location,
        })
    }

    /// Opens a connection and brings the database up to the shape the code
    /// expects. On failure the handle is dropped, which closes it - there is
    /// no half-open connection left behind for a retry to leak.
    pub async fn open(url: &str, auth_token: &str) -> Result<Self, DbError> {
        let db = Self::connect(url, auth_token).await?;
        db.bootstrap().await?;
        Ok(db)
    }

    /// [`Db::open`] on `DATABASE_URL` (default [`DEFAULT_DATABASE_URL`]) and
    /// `DATABASE_AUTH_TOKEN` (default empty), read through `env`.
    pub async fn open_from_env<F>(env: F) -> Result<Self, DbError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let url = env("DATABASE_URL").unwrap_or_else(|| DEFAULT_DATABASE_URL.to_owned());
        let auth_token = env("DATABASE_AUTH_TOKEN").unwrap_or_default();
        Self::open(&url, &auth_token).await
    }

    /// Brings a database up to the shape the code expects, in the one order
    /// that works for a database created today *and* one created before a
    /// column it now has existed.
    ///
    /// The three phases cannot be collapsed into two. Running the whole schema
    /// first and migrating afterwards leaves a legacy database permanently
    /// unopenable: `CREATE INDEX ... ON claim_sends (wallet, sent_at)` throws
    /// `no such column: wallet` against a table that predates the column,
    /// before the migration that would have added it ever runs - this is what
    /// took production down. Migrating first instead breaks the other case,
    /// where `ALTER TABLE` names a table nothing has created yet.
    ///
    /// Only ever creates what is missing: a table the schema does not name
    /// (the live `allowlist`) is never touched.
    pub async fn bootstrap(&self) -> Result<(), DbError> {
        for sql in schema::table_statements() {
            self.run(sql, ()).await?;
        }
        migrations::add_missing_columns(self).await?;
        for sql in schema::column_dependent_statements() {
            self.run(sql, ()).await?;
        }
        Ok(())
    }

    /// Every row `sql` returns, each decoded into `T` by column name.
    pub async fn all<T>(&self, sql: &str, params: impl IntoParams) -> Result<Vec<T>, DbError>
    where
        T: DeserializeOwned,
    {
        query_all(&self.connection, sql, params).await
    }

    /// Runs a statement that returns no rows, and says how many it changed.
    pub async fn run(&self, sql: &str, params: impl IntoParams) -> Result<u64, DbError> {
        execute(&self.connection, sql, params).await
    }

    /// Runs every statement or none of them.
    pub async fn batch(&self, statements: Vec<Statement>) -> Result<(), DbError> {
        let transaction = self.transaction().await?;
        for statement in statements {
            transaction.run(&statement.sql, statement.params).await?;
        }
        transaction.commit().await
    }

    /// Starts a write transaction on a connection of its own. Dropping the
    /// [`Transaction`] without committing rolls it back.
    pub async fn transaction(&self) -> Result<Transaction, DbError> {
        let connection = new_connection(&self.database, self.location)?;
        let inner = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await
            .map_err(|source| query_error("BEGIN IMMEDIATE", source))?;
        Ok(Transaction { inner })
    }

    /// Empties every table, including ones the schema does not name. Test
    /// support only: it is not compiled into the shipped binary at all, and it
    /// still refuses when `NODE_ENV` is `production`, the guard the
    /// TypeScript relied on.
    #[cfg(test)]
    pub async fn reset<F>(&self, env: F) -> Result<(), DbError>
    where
        F: Fn(&str) -> Option<String>,
    {
        if env("NODE_ENV").as_deref() == Some("production") {
            return Err(DbError::ResetRefused);
        }
        #[derive(serde::Deserialize)]
        struct Table {
            name: String,
        }
        let tables: Vec<Table> = self
            .all(
                r"SELECT name FROM sqlite_master WHERE type = ? AND name NOT LIKE 'sqlite\_%' ESCAPE '\'",
                ["table"],
            )
            .await?;
        for table in tables {
            let quoted = table.name.replace('"', "\"\"");
            self.run(&format!("DELETE FROM \"{quoted}\""), ()).await?;
        }
        Ok(())
    }
}

/// A write transaction. Commit it explicitly; dropping it rolls back.
pub struct Transaction {
    inner: libsql::Transaction,
}

impl fmt::Debug for Transaction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Transaction")
            .finish_non_exhaustive()
    }
}

impl Transaction {
    pub async fn all<T>(&self, sql: &str, params: impl IntoParams) -> Result<Vec<T>, DbError>
    where
        T: DeserializeOwned,
    {
        query_all(&self.inner, sql, params).await
    }

    pub async fn run(&self, sql: &str, params: impl IntoParams) -> Result<u64, DbError> {
        execute(&self.inner, sql, params).await
    }

    pub async fn commit(self) -> Result<(), DbError> {
        self.inner
            .commit()
            .await
            .map_err(|source| query_error("COMMIT", source))
    }

    pub async fn rollback(self) -> Result<(), DbError> {
        self.inner
            .rollback()
            .await
            .map_err(|source| query_error("ROLLBACK", source))
    }
}

/// The process-wide handle, opened and bootstrapped on first use. A failed
/// open is not remembered, so one unreachable moment at cold start does not
/// reject every query for the life of the process.
pub async fn shared<F>(env: F) -> Result<&'static Db, DbError>
where
    F: Fn(&str) -> Option<String>,
{
    static SHARED: OnceCell<Db> = OnceCell::const_new();
    SHARED.get_or_try_init(|| Db::open_from_env(env)).await
}

/// The path of a `file:` URL, accepting both `file:relative/path` and
/// `file:///absolute/path`.
fn local_path(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("file:")?;
    Some(match rest.strip_prefix("//") {
        Some(absolute) => absolute,
        None => rest,
    })
}

fn check_remote_scheme(url: &str) -> Result<(), DbError> {
    let scheme = url.split_once(':').map_or("", |(scheme, _)| scheme);
    match scheme {
        "libsql" | "https" | "http" => Ok(()),
        _ => Err(DbError::UnsupportedUrl {
            scheme: scheme.to_owned(),
        }),
    }
}

fn new_connection(database: &Database, location: Location) -> Result<Connection, DbError> {
    let connection = database.connect().map_err(DbError::Open)?;
    if location == Location::LocalFile {
        connection
            .busy_timeout(LOCAL_BUSY_TIMEOUT)
            .map_err(DbError::Open)?;
    }
    Ok(connection)
}

fn query_error(sql: &str, source: libsql::Error) -> DbError {
    DbError::Query {
        sql: sql.to_owned(),
        source,
    }
}

async fn query_all<T>(
    connection: &Connection,
    sql: &str,
    params: impl IntoParams,
) -> Result<Vec<T>, DbError>
where
    T: DeserializeOwned,
{
    let mut rows = connection
        .query(sql, params)
        .await
        .map_err(|source| query_error(sql, source))?;
    let mut decoded = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|source| query_error(sql, source))?
    {
        let value = libsql::de::from_row(&row).map_err(|source| DbError::Decode {
            sql: sql.to_owned(),
            source,
        })?;
        decoded.push(value);
    }
    Ok(decoded)
}

async fn execute(
    connection: &Connection,
    sql: &str,
    params: impl IntoParams,
) -> Result<u64, DbError> {
    connection
        .execute(sql, params)
        .await
        .map_err(|source| query_error(sql, source))
}

#[cfg(test)]
mod tests {
    use super::testing::{TestDb, columns_of, index_exists, table_names};
    use super::*;

    fn node_env(value: Option<&'static str>) -> impl Fn(&str) -> Option<String> {
        move |name| {
            (name == "NODE_ENV")
                .then(|| value.map(str::to_owned))
                .flatten()
        }
    }

    #[derive(Debug, serde::Deserialize, PartialEq)]
    struct Id {
        id: i64,
    }

    async fn probe_rows(db: &Db) -> Vec<Id> {
        db.all("SELECT id FROM reset_guard_probe", ())
            .await
            .unwrap()
    }

    async fn with_probe_row() -> TestDb {
        let test_db = TestDb::fresh().await;
        test_db
            .run(
                "CREATE TABLE IF NOT EXISTS reset_guard_probe (id INTEGER)",
                (),
            )
            .await
            .unwrap();
        test_db
            .run("INSERT INTO reset_guard_probe (id) VALUES (1)", ())
            .await
            .unwrap();
        test_db
    }

    #[tokio::test]
    async fn reset_is_refused_when_node_env_is_production_and_leaves_data_untouched() {
        let test_db = with_probe_row().await;

        let outcome = test_db.reset(node_env(Some("production"))).await;

        assert!(
            outcome
                .as_ref()
                .is_err_and(|error| error.to_string().contains("refused in production")),
            "{outcome:?}"
        );
        assert_eq!(
            probe_rows(&test_db).await.len(),
            1,
            "the guard must throw before any table is touched"
        );
    }

    #[tokio::test]
    async fn reset_empties_every_table_outside_production() {
        let test_db = with_probe_row().await;

        test_db.reset(node_env(None)).await.unwrap();

        assert_eq!(probe_rows(&test_db).await.len(), 0);
    }

    /// The case that stops the obvious fix: migrating before the schema runs
    /// would heal a legacy database and break this one, where `ALTER TABLE`
    /// names a table nothing has created yet.
    #[tokio::test]
    async fn bootstrap_creates_every_table_the_schema_declares_on_an_empty_database() {
        let test_db = TestDb::fresh().await;

        assert_eq!(
            table_names(&test_db).await.len(),
            schema::table_statements().count(),
            "one table per CREATE TABLE in the schema, or a phase of the bootstrap did not run"
        );
    }

    /// The third phase is the one that only runs after the migration, so a
    /// fresh database is where it would be quietly skipped.
    #[tokio::test]
    async fn bootstrap_creates_the_schemas_indexes_on_an_empty_database_too() {
        let test_db = TestDb::fresh().await;

        assert!(index_exists(&test_db, "inboxes_by_wallet").await);
    }

    #[tokio::test]
    async fn bootstrapping_a_database_that_was_just_created_does_not_error() {
        let test_db = TestDb::fresh().await;

        let again = test_db.bootstrap().await;

        assert!(
            again.is_ok(),
            "the second cold start runs this again over everything the first one made: {again:?}"
        );
    }

    #[tokio::test]
    async fn bootstrap_creates_exactly_the_tables_the_live_database_has() {
        let test_db = TestDb::fresh().await;

        assert_eq!(
            table_names(&test_db).await,
            [
                "challenges",
                "claim_sends",
                "classifications",
                "inbox_claims",
                "inboxes",
                "issued_rp_contexts",
                "nullifier_rebinds",
                "nullifiers",
                "passes",
                "sender_wallets",
                "spent_wallet_nonces",
            ]
        );
    }

    /// The live Turso database has an `allowlist` table no SQL in the repo
    /// describes. Bootstrap must neither drop, alter, nor empty it.
    #[tokio::test]
    async fn bootstrap_leaves_a_table_it_does_not_know_about_alone() {
        let test_db = TestDb::empty().await;
        let allowlist = "CREATE TABLE allowlist (email TEXT PRIMARY KEY, note TEXT)";
        test_db.run(allowlist, ()).await.unwrap();
        test_db
            .run(
                "INSERT INTO allowlist (email, note) VALUES (?, ?)",
                ["someone@example.com", "kept"],
            )
            .await
            .unwrap();
        let columns_before = columns_of(&test_db, "allowlist").await;

        test_db.bootstrap().await.unwrap();
        test_db.bootstrap().await.unwrap();

        #[derive(Debug, serde::Deserialize, PartialEq)]
        struct Sql {
            sql: String,
        }
        let definition: Vec<Sql> = test_db
            .all(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'allowlist'",
                (),
            )
            .await
            .unwrap();
        assert_eq!(
            definition,
            [Sql {
                sql: allowlist.to_owned()
            }]
        );
        assert_eq!(columns_of(&test_db, "allowlist").await, columns_before);
        #[derive(Debug, serde::Deserialize, PartialEq)]
        struct Entry {
            email: String,
            note: String,
        }
        let entries: Vec<Entry> = test_db
            .all("SELECT email, note FROM allowlist", ())
            .await
            .unwrap();
        assert_eq!(
            entries,
            [Entry {
                email: "someone@example.com".to_owned(),
                note: "kept".to_owned()
            }]
        );
    }

    /// Every other table at its current shape and `inboxes` as it was before
    /// `wallet` existed - skipping each statement that names `inboxes` leaves
    /// out its index as well, since a deployment predating `wallet` could not
    /// have had an index over that column either.
    async fn legacy_inboxes_database() -> TestDb {
        let test_db = TestDb::empty().await;
        for statement in schema::SCHEMA {
            if statement.sql().contains("inboxes") {
                continue;
            }
            test_db.run(statement.sql(), ()).await.unwrap();
        }
        test_db
            .run(
                "CREATE TABLE inboxes (
  handle TEXT PRIMARY KEY,
  destination TEXT NOT NULL,
  created_at INTEGER NOT NULL
)",
                (),
            )
            .await
            .unwrap();
        test_db.bootstrap().await.unwrap();
        test_db
    }

    /// The same shape as the outage, on `inboxes` rather than the
    /// `claim_sends` table that actually failed: applying the schema before
    /// migrating puts `CREATE INDEX ... ON inboxes (wallet)` in front of the
    /// `ALTER TABLE` that adds the column.
    #[tokio::test]
    async fn bootstrap_brings_up_a_database_whose_inboxes_table_predates_the_wallet_column() {
        let test_db = legacy_inboxes_database().await;

        let columns = columns_of(&test_db, "inboxes").await;
        assert!(
            columns.iter().any(|column| column.name == "wallet"),
            "the migration must have run against the legacy table"
        );
    }

    /// Healing the table cannot come at the cost of the index the schema
    /// declares over the column it just added.
    #[tokio::test]
    async fn the_wallet_index_is_created_once_the_legacy_table_has_been_healed() {
        let test_db = legacy_inboxes_database().await;

        assert!(
            index_exists(&test_db, "inboxes_by_wallet").await,
            "the index has to be created, just later than it was"
        );
    }

    #[tokio::test]
    async fn inboxes_queries_naming_wallet_answer_against_a_healed_legacy_database() {
        let test_db = legacy_inboxes_database().await;

        test_db
            .create_inbox("demo", "owner@example.com", "0xABC")
            .await;

        assert_eq!(
            test_db.inbox_wallet_by_handle("demo").await.as_deref(),
            Some("0xabc"),
            "creating an inbox and reading it by handle must round-trip the wallet"
        );
        assert_eq!(
            test_db.inbox_handle_by_wallet("0xabc").await.as_deref(),
            Some("demo"),
            "reading by wallet must find the row it was just written to"
        );
    }

    #[tokio::test]
    async fn bootstrapping_an_already_healed_legacy_database_again_does_not_error() {
        let test_db = legacy_inboxes_database().await;

        let again = test_db.bootstrap().await;

        assert!(
            again.is_ok(),
            "every cold start after the first one runs this against a database it already repaired: {again:?}"
        );
    }

    #[tokio::test]
    async fn a_batch_applies_every_statement_or_none() {
        let test_db = TestDb::fresh().await;
        let insert = |nonce: &'static str| {
            Statement::new(
                "INSERT INTO spent_wallet_nonces (nonce, expires_at) VALUES (?, ?)",
                vec![Value::from(nonce), Value::from(1_i64)],
            )
        };

        let failed = test_db.batch(vec![insert("a"), insert("a")]).await;
        test_db.batch(vec![insert("b"), insert("c")]).await.unwrap();

        assert!(failed.is_err(), "the duplicate primary key must fail");
        #[derive(Debug, serde::Deserialize)]
        struct Nonce {
            nonce: String,
        }
        let nonces: Vec<Nonce> = test_db
            .all("SELECT nonce FROM spent_wallet_nonces ORDER BY nonce", ())
            .await
            .unwrap();
        let nonces: Vec<String> = nonces.into_iter().map(|row| row.nonce).collect();
        assert_eq!(
            nonces,
            ["b", "c"],
            "the failed batch must leave nothing behind"
        );
    }

    #[tokio::test]
    async fn a_dropped_transaction_rolls_back() {
        let test_db = TestDb::fresh().await;

        {
            let transaction = test_db.transaction().await.unwrap();
            transaction
                .run(
                    "INSERT INTO spent_wallet_nonces (nonce, expires_at) VALUES (?, ?)",
                    ("dropped", 1_i64),
                )
                .await
                .unwrap();
        }

        let rows: Vec<Id> = test_db
            .all("SELECT rowid AS id FROM spent_wallet_nonces", ())
            .await
            .unwrap();
        assert!(rows.is_empty(), "{rows:?}");
    }

    #[tokio::test]
    async fn a_row_of_the_wrong_shape_is_an_error_naming_the_query() {
        let test_db = TestDb::fresh().await;
        test_db
            .run(
                "INSERT INTO spent_wallet_nonces (nonce, expires_at) VALUES (?, ?)",
                ("n", 1_i64),
            )
            .await
            .unwrap();

        let decoded = test_db
            .all::<Id>("SELECT nonce AS id FROM spent_wallet_nonces", ())
            .await;

        assert!(
            matches!(&decoded, Err(DbError::Decode { sql, .. }) if sql.contains("spent_wallet_nonces")),
            "{decoded:?}"
        );
    }

    #[test]
    fn file_urls_resolve_to_paths() {
        assert_eq!(
            local_path("file:.data/postage.db"),
            Some(".data/postage.db")
        );
        assert_eq!(local_path("file:///tmp/x.db"), Some("/tmp/x.db"));
        assert_eq!(local_path("libsql://db.turso.io"), None);
    }

    #[test]
    fn only_turso_schemes_are_accepted_as_remote() {
        assert!(check_remote_scheme("libsql://db.turso.io").is_ok());
        assert!(check_remote_scheme("https://db.turso.io").is_ok());
        assert!(matches!(
            check_remote_scheme(":memory:"),
            Err(DbError::UnsupportedUrl { scheme }) if scheme.is_empty()
        ));
        assert!(matches!(
            check_remote_scheme("postgres://user:secret@host"),
            Err(DbError::UnsupportedUrl { scheme }) if scheme == "postgres"
        ));
    }

    #[tokio::test]
    async fn open_from_env_bootstraps_the_database_named_by_database_url() {
        let directory = tempfile::tempdir().unwrap();
        let url = format!("file:{}", directory.path().join("env.db").display());
        let env = move |name: &str| (name == "DATABASE_URL").then(|| url.clone());

        let db = Db::open_from_env(env).await.unwrap();

        assert_eq!(
            table_names(&db).await.len(),
            schema::table_statements().count()
        );
    }
}

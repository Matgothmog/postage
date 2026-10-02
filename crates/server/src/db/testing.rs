//! Test support: a database of its own per test, in a temporary directory that
//! is removed when the test is done with it.
//!
//! One file per test rather than one shared path plus [`Db::reset`]: tests run
//! in parallel, and a shared file means two of them truncate each other's
//! tables mid-test.

use std::ops::Deref;

use serde::Deserialize;
use tempfile::TempDir;

use super::Db;

#[derive(Debug)]
pub struct TestDb {
    // Declared before `directory` so the connection closes before the file
    // underneath it is deleted.
    db: Db,
    directory: TempDir,
}

impl TestDb {
    /// A database with every table the schema declares, the way a cold start
    /// leaves it.
    pub async fn fresh() -> Self {
        let test_db = Self::empty().await;
        test_db.bootstrap().await.unwrap();
        test_db
    }

    /// A database with no tables at all, for building a legacy shape by hand.
    pub async fn empty() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let db = Db::connect(&file_url(&directory), "").await.unwrap();
        Self { db, directory }
    }

    /// Another connection to the same file, standing in for a second instance
    /// cold-starting against the same database.
    pub async fn second_handle(&self) -> Db {
        Db::connect(&file_url(&self.directory), "").await.unwrap()
    }
}

impl Deref for TestDb {
    type Target = Db;

    fn deref(&self) -> &Db {
        &self.db
    }
}

fn file_url(directory: &TempDir) -> String {
    format!("file:{}", directory.path().join("test.db").display())
}

/// One row of `PRAGMA table_info`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub notnull: i64,
    pub dflt_value: Option<String>,
}

pub async fn columns_of(db: &Db, table: &str) -> Vec<ColumnInfo> {
    db.all(&format!("PRAGMA table_info({table})"), ())
        .await
        .unwrap()
}

#[derive(Debug, Deserialize)]
struct Name {
    name: String,
}

/// Every table in the database, the bookkeeping ones SQLite makes for itself
/// (`sqlite_sequence`) left out.
pub async fn table_names(db: &Db) -> Vec<String> {
    let rows: Vec<Name> = db
        .all(
            r"SELECT name FROM sqlite_master WHERE type = ? AND name NOT LIKE 'sqlite\_%' ESCAPE '\' ORDER BY name",
            ["table"],
        )
        .await
        .unwrap();
    rows.into_iter().map(|row| row.name).collect()
}

pub async fn index_exists(db: &Db, name: &str) -> bool {
    let rows: Vec<Name> = db
        .all(
            "SELECT name FROM sqlite_master WHERE type = ? AND name = ?",
            ["index", name],
        )
        .await
        .unwrap();
    rows.len() == 1
}

//! Inboxes: a handle that forwards to a verified destination, owned by the
//! wallet that claimed it.
//!
//! Handles, destinations and wallets are stored lower-cased, so every lookup
//! lower-cases its key the same way. `now` comes from the caller; nothing here
//! reads a clock.

use libsql::params;
use serde::{Deserialize, Serialize};

use super::{Db, DbError};

/// Serialized in the column order `SELECT *` returns on a database built from
/// the current schema, which is the order `GET /api/inbox` answers in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Inbox {
    pub handle: String,
    /// Where mail is forwarded. Verified with Cloudflare before anything is sent.
    pub destination: String,
    pub wallet: Option<String>,
    pub created_at: i64,
}

/// Creates the inbox, or repoints an existing handle at a new destination and
/// wallet. `created_at` keeps the value from the first creation.
pub async fn create_inbox(
    db: &Db,
    handle: &str,
    destination: &str,
    wallet: Option<&str>,
    now: i64,
) -> Result<(), DbError> {
    db.run(
        "INSERT INTO inboxes (handle, destination, wallet, created_at) VALUES (?, ?, ?, ?)
     ON CONFLICT (handle) DO UPDATE SET destination = excluded.destination, wallet = excluded.wallet",
        params![
            handle.to_lowercase(),
            destination.to_lowercase(),
            wallet.map(str::to_lowercase),
            now
        ],
    )
    .await?;
    Ok(())
}

pub async fn inbox_by_handle(db: &Db, handle: &str) -> Result<Option<Inbox>, DbError> {
    let rows: Vec<Inbox> = db
        .all(
            "SELECT * FROM inboxes WHERE handle = ?",
            params![handle.to_lowercase()],
        )
        .await?;
    Ok(rows.into_iter().next())
}

pub async fn inbox_by_wallet(db: &Db, wallet: &str) -> Result<Option<Inbox>, DbError> {
    let rows: Vec<Inbox> = db
        .all(
            "SELECT * FROM inboxes WHERE wallet = ?",
            params![wallet.to_lowercase()],
        )
        .await?;
    Ok(rows.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::TestDb;

    const NOW: i64 = 1_760_000_000;

    #[tokio::test]
    async fn a_created_inbox_is_found_by_handle_and_by_wallet() {
        let db = TestDb::fresh().await;
        create_inbox(&db, "demo", "owner@example.com", Some("0xabc"), NOW)
            .await
            .unwrap();

        let expected = Inbox {
            handle: "demo".to_owned(),
            destination: "owner@example.com".to_owned(),
            wallet: Some("0xabc".to_owned()),
            created_at: NOW,
        };
        assert_eq!(
            inbox_by_handle(&db, "demo").await.unwrap(),
            Some(expected.clone())
        );
        assert_eq!(inbox_by_wallet(&db, "0xabc").await.unwrap(), Some(expected));
    }

    #[tokio::test]
    async fn keys_are_stored_and_looked_up_lower_cased() {
        let db = TestDb::fresh().await;
        create_inbox(&db, "Demo", "Owner@Example.COM", Some("0xABC"), NOW)
            .await
            .unwrap();

        let found = inbox_by_handle(&db, "DEMO").await.unwrap().unwrap();

        assert_eq!(found.handle, "demo");
        assert_eq!(found.destination, "owner@example.com");
        assert_eq!(found.wallet.as_deref(), Some("0xabc"));
        assert!(inbox_by_wallet(&db, "0xAbC").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn an_unknown_handle_or_wallet_is_none() {
        let db = TestDb::fresh().await;

        assert_eq!(inbox_by_handle(&db, "nobody").await.unwrap(), None);
        assert_eq!(inbox_by_wallet(&db, "0xnone").await.unwrap(), None);
    }

    #[tokio::test]
    async fn creating_again_repoints_the_inbox_and_keeps_its_creation_time() {
        let db = TestDb::fresh().await;
        create_inbox(&db, "demo", "old@example.com", Some("0x1"), NOW)
            .await
            .unwrap();

        create_inbox(&db, "demo", "new@example.com", Some("0x2"), NOW + 500)
            .await
            .unwrap();

        let found = inbox_by_handle(&db, "demo").await.unwrap().unwrap();
        assert_eq!(found.destination, "new@example.com");
        assert_eq!(found.wallet.as_deref(), Some("0x2"));
        assert_eq!(found.created_at, NOW);
        assert_eq!(inbox_by_wallet(&db, "0x1").await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_inbox_without_a_wallet_has_none_and_is_not_found_by_wallet() {
        let db = TestDb::fresh().await;
        create_inbox(&db, "demo", "owner@example.com", None, NOW)
            .await
            .unwrap();

        assert_eq!(
            inbox_by_handle(&db, "demo").await.unwrap().unwrap().wallet,
            None
        );
        assert_eq!(inbox_by_wallet(&db, "").await.unwrap(), None);
    }
}

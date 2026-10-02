//! The wallet a sender last paid from, so the next message they write can be
//! priced on what is known about that wallet rather than as a stranger.
//!
//! Senders and wallets are stored lower-cased. `now` comes from the caller.

use libsql::params;
use serde::Deserialize;

use super::{Db, DbError};

#[derive(Debug, Deserialize)]
struct LinkedWallet {
    wallet: String,
}

/// Links the sender to a wallet, replacing whichever one it was linked to.
pub async fn link_sender_wallet(
    db: &Db,
    sender: &str,
    wallet: &str,
    now: i64,
) -> Result<(), DbError> {
    db.run(
        "INSERT INTO sender_wallets (sender, wallet, linked_at) VALUES (?, ?, ?)
     ON CONFLICT (sender) DO UPDATE SET wallet = excluded.wallet, linked_at = excluded.linked_at",
        params![sender.to_lowercase(), wallet.to_lowercase(), now],
    )
    .await?;
    Ok(())
}

pub async fn wallet_for_sender(db: &Db, sender: &str) -> Result<Option<String>, DbError> {
    let rows: Vec<LinkedWallet> = db
        .all(
            "SELECT wallet FROM sender_wallets WHERE sender = ?",
            params![sender.to_lowercase()],
        )
        .await?;
    Ok(rows.into_iter().next().map(|row| row.wallet))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::TestDb;

    const NOW: i64 = 1_760_000_000;

    #[tokio::test]
    async fn a_linked_wallet_is_returned_for_its_sender_case_insensitively() {
        let db = TestDb::fresh().await;
        link_sender_wallet(&db, "Alice@Example.com", "0xABC", NOW)
            .await
            .unwrap();

        assert_eq!(
            wallet_for_sender(&db, "alice@example.com")
                .await
                .unwrap()
                .as_deref(),
            Some("0xabc")
        );
    }

    #[tokio::test]
    async fn a_sender_with_no_link_has_no_wallet() {
        let db = TestDb::fresh().await;

        assert_eq!(
            wallet_for_sender(&db, "stranger@example.com")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn linking_again_replaces_the_wallet_and_refreshes_the_time() {
        let db = TestDb::fresh().await;
        link_sender_wallet(&db, "a@example.com", "0x1", NOW)
            .await
            .unwrap();

        link_sender_wallet(&db, "a@example.com", "0x2", NOW + 60)
            .await
            .unwrap();

        assert_eq!(
            wallet_for_sender(&db, "a@example.com")
                .await
                .unwrap()
                .as_deref(),
            Some("0x2")
        );
        #[derive(Debug, Deserialize)]
        struct LinkedAt {
            linked_at: i64,
        }
        let rows: Vec<LinkedAt> = db
            .all("SELECT linked_at FROM sender_wallets", ())
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].linked_at, NOW + 60);
    }
}

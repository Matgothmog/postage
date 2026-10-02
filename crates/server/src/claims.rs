//! Bringing an inbox claim up to date (`web/src/lib/claims.ts`): asking
//! Cloudflare whether its owner has clicked the confirmation link, and
//! promoting the claim to a real inbox once both confirmations are in. Until
//! then the handle resolves to nothing and mail sent to it is refused as an
//! unknown address.

use crate::cloudflare::{Cloudflare, CloudflareError};
use crate::db::claims::{
    InboxClaim, claim_by_handle, clear_claim, cloudflare_checks_exhausted,
    mark_cloudflare_verified, take_cloudflare_check,
};
use crate::db::inboxes::create_inbox;
use crate::db::{Db, DbError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimState {
    pub claim: InboxClaim,
    pub code_verified: bool,
    pub cloudflare_verified: bool,
    pub live: bool,
    /// The claim has asked Cloudflare as often as it may and never been
    /// confirmed. Nothing will ask again, so a page polling this should stop
    /// and say so rather than spin on a state that can no longer change.
    pub stalled: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Cloudflare(#[from] CloudflareError),
}

/// The claim on `handle` as it stands now, promoted to an inbox if it has
/// just become live; `None` when nobody is claiming the handle.
///
/// `expires_at` is not consulted, and that is the point rather than an
/// omission: it bounds the emailed code, which was already checked against it
/// when it was entered. Cloudflare's link carries no deadline we set, so a
/// claim whose code went in on time goes live whenever its owner clicks.
pub async fn settle_claim(
    db: &Db,
    cloudflare: &Cloudflare,
    handle: &str,
    now: i64,
) -> Result<Option<ClaimState>, ClaimError> {
    let Some(claim) = claim_by_handle(db, handle).await? else {
        return Ok(None);
    };

    let mut cloudflare_verified = claim.cf_verified_at.is_some();
    let mut stalled = false;

    // Asked only when this claim has a slot for it. The call spends an
    // account-wide Cloudflare quota and the route that leads here is polled by
    // an anonymous browser, so the question is rationed per claim rather than
    // per request.
    if let (false, Some(address_id)) = (cloudflare_verified, claim.cf_address_id.as_deref()) {
        if take_cloudflare_check(db, &claim.handle, now).await? {
            let current = cloudflare.destination_status(address_id).await?;
            if let Some(verified_at) = current.and_then(|destination| destination.verified_at) {
                mark_cloudflare_verified(db, &claim.handle, address_id, verified_at).await?;
                cloudflare_verified = true;
            }
        } else {
            // Refused, and only a refusal can mean the budget is gone: being
            // inside the interval is the ordinary case and costs nothing.
            stalled = cloudflare_checks_exhausted(db, &claim.handle).await?;
        }
    }

    let code_verified = claim.code_verified_at.is_some();
    let live = code_verified && cloudflare_verified;
    if live {
        create_inbox(
            db,
            &claim.handle,
            &claim.destination,
            Some(&claim.wallet),
            now,
        )
        .await?;
        clear_claim(db, &claim.handle).await?;
    }

    Ok(Some(ClaimState {
        claim,
        code_verified,
        cloudflare_verified,
        live,
        stalled,
    }))
}

#[cfg(test)]
mod tests {
    use libsql::params;
    use serde_json::json;

    use super::*;
    use crate::db::claims::{
        CF_CHECK_BUDGET, CF_CHECK_INTERVAL_SECONDS, NewClaim, mark_code_verified, start_claim,
    };
    use crate::db::inboxes::inbox_by_handle;
    use crate::db::testing::TestDb;
    use crate::http_stub::{Reply, Stub, serve_with};

    const NOW: i64 = 1_788_868_800;
    const HANDLE: &str = "demo";
    const ADDRESS_ID: &str = "addr-1";
    const WALLET: &str = "0x1111111111111111111111111111111111111111";
    /// 2026-01-01T00:00:00Z.
    const CONFIRMED_AT: i64 = 1_767_225_600;

    async fn claiming(db: &Db, address_id: Option<&str>, code_verified: bool) {
        start_claim(
            db,
            &NewClaim {
                handle: HANDLE.to_owned(),
                destination: "Reader@Example.com".to_owned(),
                wallet: WALLET.to_owned(),
                code_hash: "hash".to_owned(),
                expires_at: NOW + 900,
                cf_address_id: address_id.map(str::to_owned),
                cf_verified_at: None,
            },
            NOW,
        )
        .await
        .unwrap();
        if code_verified {
            mark_code_verified(db, HANDLE, NOW).await.unwrap();
        }
    }

    /// Cloudflare answering that the address was, or was not yet, confirmed.
    async fn cloudflare_saying(verified: Option<&'static str>) -> (Stub, Cloudflare) {
        let stub = serve_with(move |_| {
            Reply::new(
                200,
                json!({
                    "success": true,
                    "errors": [],
                    "result": { "id": ADDRESS_ID, "email": "reader@example.com", "verified": verified },
                })
                .to_string(),
            )
        })
        .await;
        let cloudflare = Cloudflare::new(
            reqwest::Client::default(),
            "test-account".to_owned(),
            "test-token".to_owned(),
        )
        .with_api_base(&stub.base);
        (stub, cloudflare)
    }

    async fn confirmed_cloudflare() -> (Stub, Cloudflare) {
        cloudflare_saying(Some("2026-01-01T00:00:00Z")).await
    }

    #[tokio::test]
    async fn nobody_claiming_the_handle_is_none() {
        let db = TestDb::fresh().await;
        let (stub, cloudflare) = confirmed_cloudflare().await;

        assert_eq!(
            settle_claim(&db, &cloudflare, HANDLE, NOW).await.unwrap(),
            None
        );
        assert!(stub.sent().is_empty());
    }

    #[tokio::test]
    async fn both_confirmations_in_promote_the_claim_to_an_inbox() {
        let db = TestDb::fresh().await;
        claiming(&db, Some(ADDRESS_ID), true).await;
        let (stub, cloudflare) = confirmed_cloudflare().await;

        let state = settle_claim(&db, &cloudflare, HANDLE, NOW)
            .await
            .unwrap()
            .unwrap();

        assert!(state.code_verified && state.cloudflare_verified && state.live);
        assert!(!state.stalled);
        let inbox = inbox_by_handle(&db, HANDLE).await.unwrap().unwrap();
        assert_eq!(inbox.destination, "reader@example.com");
        assert_eq!(inbox.wallet.as_deref(), Some(WALLET));
        assert_eq!(
            claim_by_handle(&db, HANDLE).await.unwrap(),
            None,
            "a used code hash is not kept"
        );
        assert_eq!(
            stub.sent()[0].path,
            "/accounts/test-account/email/routing/addresses/addr-1"
        );
    }

    /// Cloudflare's link has no deadline of ours, so a claim whose code went
    /// in on time goes live whenever its owner clicks, expired code or not.
    #[tokio::test]
    async fn a_claim_goes_live_after_its_code_window_has_closed() {
        let db = TestDb::fresh().await;
        claiming(&db, Some(ADDRESS_ID), true).await;
        let (_stub, cloudflare) = confirmed_cloudflare().await;

        let much_later = NOW + 7 * 24 * 60 * 60;
        let state = settle_claim(&db, &cloudflare, HANDLE, much_later)
            .await
            .unwrap()
            .unwrap();

        assert!(state.live);
    }

    /// Cloudflare's confirmation is kept even while the code is outstanding,
    /// so it is not asked again.
    #[tokio::test]
    async fn a_confirmed_address_is_recorded_while_the_code_is_still_outstanding() {
        let db = TestDb::fresh().await;
        claiming(&db, Some(ADDRESS_ID), false).await;
        let (stub, cloudflare) = confirmed_cloudflare().await;

        let state = settle_claim(&db, &cloudflare, HANDLE, NOW)
            .await
            .unwrap()
            .unwrap();
        assert!(state.cloudflare_verified && !state.code_verified && !state.live);
        let claim = claim_by_handle(&db, HANDLE).await.unwrap().unwrap();
        assert_eq!(claim.cf_verified_at, Some(CONFIRMED_AT));

        let later = NOW + CF_CHECK_INTERVAL_SECONDS;
        let again = settle_claim(&db, &cloudflare, HANDLE, later)
            .await
            .unwrap()
            .unwrap();
        assert!(again.cloudflare_verified);
        assert_eq!(
            stub.sent().len(),
            1,
            "a verified address is never asked about again"
        );
        assert_eq!(inbox_by_handle(&db, HANDLE).await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_address_not_yet_confirmed_leaves_the_claim_waiting() {
        let db = TestDb::fresh().await;
        claiming(&db, Some(ADDRESS_ID), true).await;
        let (_stub, cloudflare) = cloudflare_saying(Some("0001-01-01T00:00:00Z")).await;

        let state = settle_claim(&db, &cloudflare, HANDLE, NOW)
            .await
            .unwrap()
            .unwrap();

        assert!(!state.cloudflare_verified && !state.live && !state.stalled);
        assert_eq!(inbox_by_handle(&db, HANDLE).await.unwrap(), None);
        assert!(claim_by_handle(&db, HANDLE).await.unwrap().is_some());
    }

    /// A burst of pollers inside the interval costs one question, not one each.
    #[tokio::test]
    async fn polling_inside_the_interval_asks_cloudflare_once_and_is_not_stalled() {
        let db = TestDb::fresh().await;
        claiming(&db, Some(ADDRESS_ID), true).await;
        let (stub, cloudflare) = cloudflare_saying(None).await;

        settle_claim(&db, &cloudflare, HANDLE, NOW).await.unwrap();
        let second = settle_claim(&db, &cloudflare, HANDLE, NOW + 1)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(stub.sent().len(), 1);
        assert!(!second.stalled);
    }

    #[tokio::test]
    async fn a_claim_that_has_spent_its_budget_is_stalled_and_asks_nothing() {
        let db = TestDb::fresh().await;
        claiming(&db, Some(ADDRESS_ID), true).await;
        db.run(
            "UPDATE inbox_claims SET cf_checks = ? WHERE handle = ?",
            params![CF_CHECK_BUDGET, HANDLE],
        )
        .await
        .unwrap();
        let (stub, cloudflare) = confirmed_cloudflare().await;

        let state = settle_claim(&db, &cloudflare, HANDLE, NOW)
            .await
            .unwrap()
            .unwrap();

        assert!(state.stalled && !state.live);
        assert!(stub.sent().is_empty());
    }

    #[tokio::test]
    async fn a_claim_with_no_registered_address_asks_nothing() {
        let db = TestDb::fresh().await;
        claiming(&db, None, true).await;
        let (stub, cloudflare) = confirmed_cloudflare().await;

        let state = settle_claim(&db, &cloudflare, HANDLE, NOW)
            .await
            .unwrap()
            .unwrap();

        assert!(!state.cloudflare_verified && !state.live && !state.stalled);
        assert!(stub.sent().is_empty());
    }

    /// An outage is a failure, never "not verified yet"; the check it spent
    /// stays spent, as in the TypeScript.
    #[tokio::test]
    async fn a_cloudflare_failure_is_an_error_not_a_waiting_claim() {
        let db = TestDb::fresh().await;
        claiming(&db, Some(ADDRESS_ID), true).await;
        let stub = serve_with(|_| Reply::new(503, "<html>down</html>")).await;
        let cloudflare = Cloudflare::new(
            reqwest::Client::default(),
            "test-account".to_owned(),
            "test-token".to_owned(),
        )
        .with_api_base(&stub.base);

        let error = settle_claim(&db, &cloudflare, HANDLE, NOW)
            .await
            .unwrap_err();

        assert!(
            matches!(
                error,
                ClaimError::Cloudflare(CloudflareError::Unreadable(503))
            ),
            "{error}"
        );
        let claim = claim_by_handle(&db, HANDLE).await.unwrap().unwrap();
        assert_eq!(claim.cf_checks, 1);
    }

    /// The handle is looked up as stored, lowercase, whatever the caller typed.
    #[tokio::test]
    async fn the_handle_is_matched_without_regard_to_case() {
        let db = TestDb::fresh().await;
        claiming(&db, Some(ADDRESS_ID), true).await;
        let (_stub, cloudflare) = confirmed_cloudflare().await;

        let state = settle_claim(&db, &cloudflare, "DEMO", NOW)
            .await
            .unwrap()
            .unwrap();

        assert!(state.live);
    }
}

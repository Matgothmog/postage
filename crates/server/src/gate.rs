//! Settling a challenge (`web/src/lib/gate.ts`): the single path both lanes
//! take through the gate.
//!
//! The invariants it exists to hold, all of which were once re-derived in each
//! caller:
//!
//!   1. A challenge settles once. The claim is a single conditional update, so
//!      two tabs cannot both believe they are the one settling it.
//!   2. Clearing yields exactly one entitlement. Either the held message goes,
//!      or the sender can send one - never both, never neither.
//!   3. Dangerous mail is delivered by no route and no lane.
//!   4. A failure part way through leaves nothing granted and the challenge
//!      openable again, because a payment cannot be made twice.
//!   5. Answering a challenge that is already settled reports what actually
//!      happened, and repairs a sender who holds nothing.

use crate::db::challenges::{
    Challenge, challenge_by_token, claim_challenge, mark_entitled, release_challenge_claim,
};
use crate::db::passes::{add_paid_use, grant_pass, has_live_pass};
use crate::db::{Db, DbError};
use crate::hold::{MailWorker, Release};
use crate::log;

const DANGEROUS: &str = "dangerous";

/// How a sender got through. The two are not variations on one thing:
///
/// - `Human`: a proof of personhood buys a window. It says someone was there a
///   moment ago, so it opens the gate for a short while and however many
///   messages fit in it.
/// - `Paid`: a payment buys one delivery. It is a price for a specific
///   message, so it must yield exactly one, and a second payment a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Human,
    Paid,
}

impl Lane {
    /// The spelling stored in `settled_by` and `passes.reason`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Paid => "paid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateResult {
    /// No challenge carries this token.
    Unknown,
    /// Dangerous mail: refused on every lane, `reason: "dangerous"` on the wire.
    Charged,
    /// Through the gate. `reason` is the lane that settled the challenge,
    /// which may be another request's; `delivered` says whether the held
    /// message went out.
    Cleared { reason: String, delivered: bool },
}

/// The one place a challenge is settled.
///
/// Errors are the database's. A failure after the claim gives the claim back
/// before it is returned, so the challenge can be answered again.
pub async fn open_gate(
    db: &Db,
    worker: &MailWorker,
    token: &str,
    lane: Lane,
    now: i64,
) -> Result<GateResult, DbError> {
    let Some(challenge) = challenge_by_token(db, token).await? else {
        return Ok(GateResult::Unknown);
    };
    if challenge.tier == DANGEROUS {
        // Closed as well as refused. Left open, the page goes on offering to
        // pay for something no route will deliver, and a second payToSend
        // reverts.
        claim_challenge(db, token, lane.as_str(), now).await?;
        return Ok(GateResult::Charged);
    }

    if !claim_challenge(db, token, lane.as_str(), now).await? {
        return recover(db, token, challenge, lane, now).await;
    }

    match settle(db, worker, &challenge, token, lane, now).await {
        Ok(result) => Ok(result),
        Err(error) => {
            // Nothing was granted before this point on either lane, so giving
            // the claim back leaves the sender exactly as they were rather
            // than holding a settled challenge and nothing to show for it.
            let _ = release_challenge_claim(db, token, lane.as_str()).await;
            Err(error)
        }
    }
}

async fn settle(
    db: &Db,
    worker: &MailWorker,
    challenge: &Challenge,
    token: &str,
    lane: Lane,
    now: i64,
) -> Result<GateResult, DbError> {
    // The message goes out before anything is granted, on both lanes.
    // Granting first leaves a pass standing if this fails, and the rollback
    // would then reopen a challenge whose sender already holds what it owed.
    let released = if challenge.delivered_at.is_some() {
        Release::Delivered
    } else {
        worker
            .release_held_message(db, token, &challenge.handle, now)
            .await?
    };

    // The sender is never told why, but "expired", "no_inbox" and
    // "send_failed" are three different bugs to chase. Not the token: it is
    // the capability that releases the message.
    if let Release::Undelivered(reason) = released {
        log::error(
            "held message not released",
            &[("handle", &challenge.handle), ("reason", &reason)],
        );
    }

    // Granted last, and the paid entitlement was already taken by the claim,
    // so nothing after this can fail and leave the sender holding something
    // the rollback would not undo.
    let delivered = released.delivered();
    grant(db, challenge, lane, delivered, now).await?;
    Ok(GateResult::Cleared {
        reason: lane.as_str().to_owned(),
        delivered,
    })
}

/// What each lane is owed once the message has been dealt with.
///
/// A proof buys a window, so it is opened whether or not the held message
/// went: being able to write is the thing that was earned. A payment buys one
/// delivery, so it owes nothing further if that delivery already happened.
async fn grant(
    db: &Db,
    challenge: &Challenge,
    lane: Lane,
    delivered: bool,
    now: i64,
) -> Result<(), DbError> {
    match lane {
        Lane::Human => {
            grant_pass(
                db,
                &challenge.handle,
                &challenge.sender,
                Lane::Human.as_str(),
                None,
                now,
            )
            .await
        }
        Lane::Paid if !delivered => {
            add_paid_use(db, &challenge.handle, &challenge.sender, now).await
        }
        Lane::Paid => Ok(()),
    }
}

/// Someone else settled this, or we did and the answer was lost. Report what
/// actually happened rather than a refusal, and if they hold nothing while
/// nothing was delivered, give back what their lane earned. The caller has
/// already proved the lane to reach this point.
async fn recover(
    db: &Db,
    token: &str,
    before: Challenge,
    lane: Lane,
    now: i64,
) -> Result<GateResult, DbError> {
    // Re-read, because `before` was loaded ahead of the claim and will not
    // carry whatever the winner wrote while we were asking.
    let settled = challenge_by_token(db, token).await?.unwrap_or(before);
    if settled.tier == DANGEROUS {
        return Ok(GateResult::Charged);
    }

    let delivered = settled.delivered_at.is_some();
    let holds_nothing = !has_live_pass(db, &settled.handle, &settled.sender, now).await?;

    if !delivered && holds_nothing {
        match lane {
            // A fresh proof was presented to reach here, and that is the price
            // of a window. Re-earning one is the design, not a leak.
            Lane::Human => {
                grant_pass(
                    db,
                    &settled.handle,
                    &settled.sender,
                    Lane::Human.as_str(),
                    None,
                    now,
                )
                .await?;
            }
            // A payment settles onchain forever, so "have they paid" cannot
            // decide this - only whether this challenge has already handed
            // over what that payment bought. Taking the entitlement is the
            // same statement as checking for it, because the winner of the
            // claim may still be inside the delivery it is about to make.
            Lane::Paid => {
                if mark_entitled(db, token, now).await? {
                    add_paid_use(db, &settled.handle, &settled.sender, now).await?;
                }
            }
        }
    }

    Ok(GateResult::Cleared {
        reason: settled
            .settled_by
            .unwrap_or_else(|| lane.as_str().to_owned()),
        delivered,
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use libsql::params;
    use serde::Deserialize;

    use super::*;
    use crate::db::challenges::{NewChallenge, create_challenge, mark_delivered};
    use crate::db::inboxes::create_inbox;
    use crate::db::passes::spend_pass;
    use crate::db::testing::TestDb;
    use crate::http_stub::{Reply, closed_port, serve_with};
    use crate::log::captured;

    const NOW: i64 = 1_788_868_800;
    const HANDLE: &str = "demo";

    async fn seed(db: &Db, token: &str, tier: &str, sender: &str) {
        create_challenge(
            db,
            &NewChallenge {
                token: token.to_owned(),
                handle: HANDLE.to_owned(),
                sender: sender.to_owned(),
                message_id: format!("0x{}", "ab".repeat(32)),
                tier: tier.to_owned(),
                amount: "1".to_owned(),
                held_until: Some(NOW + 900),
                quote_json: "{}".to_owned(),
                created_at: NOW,
            },
        )
        .await
        .unwrap();
    }

    /// Delivery always fails against this one: nothing listens on the port,
    /// which is the interesting half. A message that goes out needs no
    /// entitlement; one that does not is where every double grant came from.
    fn unreachable_worker() -> MailWorker {
        MailWorker::new(reqwest::Client::default(), &closed_port(), "test")
    }

    async fn live(db: &Db, sender: &str) -> bool {
        has_live_pass(db, HANDLE, sender, NOW).await.unwrap()
    }

    /// Both reach past the module deliberately: nothing in the app winds a
    /// pass down or reads an expiry back, and sitting out a real window would
    /// take a quarter of an hour.
    async fn window_ends_at(db: &Db, sender: &str, at: i64) {
        db.run(
            "UPDATE passes SET expires_at = ? WHERE handle = ? AND sender = ?",
            params![at, HANDLE, sender],
        )
        .await
        .unwrap();
    }

    async fn pass_expiry(db: &Db, sender: &str) -> i64 {
        #[derive(Deserialize)]
        struct Expiry {
            expires_at: i64,
        }
        let rows: Vec<Expiry> = db
            .all(
                "SELECT expires_at FROM passes WHERE handle = ? AND sender = ?",
                params![HANDLE, sender],
            )
            .await
            .unwrap();
        rows.first().map_or(0, |row| row.expires_at)
    }

    #[tokio::test]
    async fn a_payment_yields_exactly_one_delivery_however_often_it_is_asked_for() {
        let db = TestDb::fresh().await;
        let worker = unreachable_worker();
        seed(&db, "paid1", "commercial", "payer@x.com").await;

        let first = open_gate(&db, &worker, "paid1", Lane::Paid, NOW)
            .await
            .unwrap();
        assert!(matches!(first, GateResult::Cleared { .. }));
        assert!(live(&db, "payer@x.com").await);

        spend_pass(&db, HANDLE, "payer@x.com", false, NOW)
            .await
            .unwrap();
        assert!(!live(&db, "payer@x.com").await);

        open_gate(&db, &worker, "paid1", Lane::Paid, NOW)
            .await
            .unwrap();
        assert!(
            !live(&db, "payer@x.com").await,
            "one payment must not be redeemable again once its delivery was spent"
        );
    }

    /// The second proof answers a fresh challenge, and the window is wound
    /// down first so "pushed back out" and "left where it was" differ.
    #[tokio::test]
    async fn a_proof_re_earns_a_window_because_presenting_it_again_is_the_cost() {
        let db = TestDb::fresh().await;
        let worker = unreachable_worker();
        seed(&db, "hum1", "human", "person@x.com").await;
        open_gate(&db, &worker, "hum1", Lane::Human, NOW)
            .await
            .unwrap();

        let stale_at = NOW + 60;
        window_ends_at(&db, "person@x.com", stale_at).await;

        seed(&db, "hum2", "human", "person@x.com").await;
        open_gate(&db, &worker, "hum2", Lane::Human, NOW)
            .await
            .unwrap();

        assert!(
            pass_expiry(&db, "person@x.com").await > stale_at,
            "a second proof was presented, so the window has to move back out past where it was left"
        );
    }

    #[tokio::test]
    async fn dangerous_mail_is_delivered_by_no_lane() {
        let db = TestDb::fresh().await;
        let worker = unreachable_worker();
        seed(&db, "bad1", "dangerous", "phish@x.com").await;

        for lane in [Lane::Human, Lane::Paid] {
            let result = open_gate(&db, &worker, "bad1", lane, NOW).await.unwrap();
            assert_eq!(result, GateResult::Charged);
        }
        assert!(!live(&db, "phish@x.com").await);
    }

    #[tokio::test]
    async fn a_delivered_message_grants_nothing_so_it_cannot_be_sent_twice() {
        let db = TestDb::fresh().await;
        let worker = unreachable_worker();
        seed(&db, "del1", "commercial", "done@x.com").await;
        mark_delivered(&db, "del1", NOW).await.unwrap();

        open_gate(&db, &worker, "del1", Lane::Paid, NOW)
            .await
            .unwrap();
        assert!(!live(&db, "done@x.com").await);
    }

    /// What a winner leaves behind when it settles and then dies before
    /// granting anything: a settled challenge and nothing to show for it.
    /// Claiming without granting is that state exactly.
    #[tokio::test]
    async fn a_sender_holding_nothing_after_a_failed_delivery_is_repaired() {
        let db = TestDb::fresh().await;
        let worker = unreachable_worker();
        seed(&db, "stuck", "commercial", "stuck@x.com").await;
        claim_challenge(&db, "stuck", "human", NOW).await.unwrap();

        open_gate(&db, &worker, "stuck", Lane::Paid, NOW)
            .await
            .unwrap();

        assert!(
            live(&db, "stuck@x.com").await,
            "the payment bought a delivery nobody made, so answering again has to hand it over"
        );
    }

    /// The winner is inside the release for a long time while the loser
    /// decides what to do, so the worker holds its answer open until both
    /// have made their decision.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn clearing_twice_at_once_settles_once_even_while_the_release_is_slow() {
        let db = TestDb::fresh().await;
        create_inbox(
            &db,
            HANDLE,
            "demo@example.com",
            Some(&format!("0x{}", "11".repeat(20))),
            NOW,
        )
        .await
        .unwrap();
        seed(&db, "race1", "commercial", "race@x.com").await;
        let stub = serve_with(|_| Reply::new(200, "{}").after(Duration::from_millis(300))).await;
        let worker = MailWorker::new(reqwest::Client::default(), &stub.base, "test");

        let (first, second) = tokio::join!(
            open_gate(&db, &worker, "race1", Lane::Paid, NOW),
            open_gate(&db, &worker, "race1", Lane::Paid, NOW)
        );
        first.unwrap();
        second.unwrap();

        assert!(
            !live(&db, "race@x.com").await,
            "the message was delivered, so one payment must leave no spare use behind"
        );
    }

    #[tokio::test]
    async fn an_unknown_token_clears_nothing() {
        let db = TestDb::fresh().await;
        let result = open_gate(&db, &unreachable_worker(), "nope", Lane::Paid, NOW)
            .await
            .unwrap();
        assert_eq!(result, GateResult::Unknown);
    }

    /// "The inbox vanished", "the hold had expired" and "the worker could not
    /// be reached" are three different bugs; the reason reaches the log.
    #[tokio::test]
    async fn a_release_that_did_not_deliver_logs_which_of_the_three_reasons_it_was() {
        let db = TestDb::fresh().await;
        // No inbox for this handle, so the release fails at its first check.
        seed(&db, "unlogged", "commercial", "missing@x.com").await;

        let (result, lines) = captured::during(open_gate(
            &db,
            &unreachable_worker(),
            "unlogged",
            Lane::Paid,
            NOW,
        ))
        .await;

        result.unwrap();
        assert_eq!(
            lines,
            ["held message not released handle=demo reason=no_inbox"],
            "the reason a release did not happen must reach the log"
        );
        assert!(
            !lines[0].contains("unlogged"),
            "the token is a capability and stays out of the log"
        );
    }

    /// An unlimited window already outranks a single use, so paying during
    /// one must push the window out instead, or the money buys nothing.
    #[tokio::test]
    async fn paying_while_a_free_window_is_live_does_not_lose_the_payment() {
        let db = TestDb::fresh().await;
        seed(&db, "both", "commercial", "both@x.com").await;
        grant_pass(&db, HANDLE, "both@x.com", "human", None, NOW)
            .await
            .unwrap();
        let window_ends = NOW + 60;
        window_ends_at(&db, "both@x.com", window_ends).await;

        open_gate(&db, &unreachable_worker(), "both", Lane::Paid, NOW)
            .await
            .unwrap();

        assert!(
            pass_expiry(&db, "both@x.com").await > window_ends,
            "the delivery was paid for, so it has to outlast the free window it was paid during"
        );
    }

    #[tokio::test]
    async fn a_cleared_challenge_reports_its_lane_and_whether_the_message_went() {
        let db = TestDb::fresh().await;
        create_inbox(&db, HANDLE, "demo@example.com", None, NOW)
            .await
            .unwrap();
        seed(&db, "went", "commercial", "went@x.com").await;
        let stub = serve_with(|_| Reply::new(200, "{}")).await;
        let worker = MailWorker::new(reqwest::Client::default(), &stub.base, "test");

        let result = open_gate(&db, &worker, "went", Lane::Human, NOW)
            .await
            .unwrap();

        assert_eq!(
            result,
            GateResult::Cleared {
                reason: "human".to_owned(),
                delivered: true
            }
        );
    }

    /// The loser reports the winner's lane, not its own.
    #[tokio::test]
    async fn answering_a_settled_challenge_reports_the_lane_that_settled_it() {
        let db = TestDb::fresh().await;
        seed(&db, "settled", "commercial", "s@x.com").await;
        claim_challenge(&db, "settled", "human", NOW).await.unwrap();

        let result = open_gate(&db, &unreachable_worker(), "settled", Lane::Paid, NOW)
            .await
            .unwrap();

        assert_eq!(
            result,
            GateResult::Cleared {
                reason: "human".to_owned(),
                delivered: false
            }
        );
    }

    /// A failure after the claim hands the claim back, so the challenge can be
    /// answered again: a payment cannot be made twice. Dropping the passes
    /// table makes the grant, the last step, fail.
    #[tokio::test]
    async fn a_failure_after_the_claim_gives_the_claim_back() {
        let db = TestDb::fresh().await;
        seed(&db, "fails", "commercial", "f@x.com").await;
        db.run("DROP TABLE passes", ()).await.unwrap();

        let outcome = open_gate(&db, &unreachable_worker(), "fails", Lane::Paid, NOW).await;

        assert!(outcome.is_err());
        let challenge = challenge_by_token(&db, "fails").await.unwrap().unwrap();
        assert_eq!(challenge.resolved_at, None);
        assert_eq!(challenge.settled_by, None);
        assert_eq!(challenge.entitled_at, None);
    }
}

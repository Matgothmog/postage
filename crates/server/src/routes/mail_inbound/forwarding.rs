//! The two ways an inbound message reaches its destination without being held
//! (`web/src/app/api/mail/inbound/forwarding.ts`): the tier nobody pays for,
//! and a pass the sender already holds.

use postage_core::classify::Verdict;
use postage_shared::Tier;

use crate::db::classifications::BudgetState;
use crate::db::passes::spend_pass;
use crate::db::{Db, DbError};

/// How a message reached the destination without being held: the tier that
/// is never charged, or the pass that carried it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Forwarded {
    pub reason: String,
}

/// One message as the forwarding rules see it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Candidate<'a> {
    pub handle: &'a str,
    pub sender: &'a str,
    pub verdict: &'a Verdict,
    pub authenticated: bool,
    /// Which ceiling refused the classifier, when one did.
    pub budget_refusal: Option<BudgetState>,
}

/// Held to a higher bar when the verdict is degraded, and shut entirely when
/// the model was never asked.
///
/// The header fallback calls anything transactional-sounding important as
/// long as authentication did not outright fail, so a degraded verdict is a
/// free delivery waiting to be arranged. That is tolerable for exactly one
/// reason to be degraded: the model was asked and failed anyway. Real login
/// codes have to keep arriving through an outage of ours, and a sender cannot
/// cause that outage.
///
/// Every other degraded verdict on this path is one where a budget had run
/// out, and a budget is a ceiling we chose. All three can be reached
/// deliberately (an address's own slice, a domain's, a handle's pool), so none
/// of them may unlock the tier nobody pays for. The cost falls on the
/// recipient: while a pool is drained, transactional mail is held for a
/// challenge instead of delivered, rather than a flood being the way through
/// the gate.
pub(super) fn delivered_free(
    verdict: &Verdict,
    authenticated: bool,
    budget_refusal: Option<BudgetState>,
) -> bool {
    if verdict.tier != Tier::Important {
        return false;
    }
    if !verdict.degraded {
        return true;
    }
    authenticated && budget_refusal.is_none()
}

/// The two ways a message goes straight to the destination, in the order they
/// have to be tried. `None` means it is priced and challenged instead.
pub(super) async fn forward_without_challenge(
    db: &Db,
    mail: &Candidate<'_>,
    now: i64,
) -> Result<Option<Forwarded>, DbError> {
    // Checked before any pass is spent. This tier grants nothing, so taking a
    // paid use for it would charge someone twice for one delivery.
    if delivered_free(mail.verdict, mail.authenticated, mail.budget_refusal) {
        return Ok(Some(Forwarded {
            reason: mail.verdict.tier.as_str().to_owned(),
        }));
    }

    if !mail.authenticated || mail.verdict.tier == Tier::Dangerous {
        return Ok(None);
    }

    // Narrowed to counted passes only when the sender spent their own slice:
    // an unlimited window plus a budget they exhausted themselves is a licence
    // to deliver anything unread, while a single paid use cannot flood. A
    // domain's slice and a handle's pool are shared with strangers, so
    // narrowing on either would let a flood take away what someone else paid
    // or proved for.
    let counted_only = mail.budget_refusal == Some(BudgetState::SpentBySender);
    let pass = spend_pass(db, mail.handle, mail.sender, counted_only, now).await?;
    Ok(pass.map(|pass| Forwarded {
        reason: pass.reason,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::passes::{grant_pass, has_live_pass};
    use crate::db::testing::TestDb;
    use crate::routes::testing::NOW;

    const HANDLE: &str = "demo";
    const SENDER: &str = "sender@x.com";

    fn verdict_of(tier: Tier, degraded: bool) -> Verdict {
        Verdict {
            tier,
            confidence: 0.9,
            reasons: Vec::new(),
            degraded,
        }
    }

    async fn forward(
        db: &Db,
        tier: Tier,
        authenticated: bool,
        budget_refusal: Option<BudgetState>,
    ) -> Option<Forwarded> {
        let verdict = verdict_of(tier, false);
        let mail = Candidate {
            handle: HANDLE,
            sender: SENDER,
            verdict: &verdict,
            authenticated,
            budget_refusal,
        };
        forward_without_challenge(db, &mail, NOW).await.unwrap()
    }

    fn forwarded(reason: &str) -> Option<Forwarded> {
        Some(Forwarded {
            reason: reason.to_owned(),
        })
    }

    #[test]
    fn mail_the_model_itself_called_important_is_delivered_free() {
        assert!(delivered_free(
            &verdict_of(Tier::Important, false),
            true,
            None
        ));
    }

    /// The tier exists so that login codes keep arriving while the model is
    /// down.
    #[test]
    fn a_degraded_important_verdict_still_goes_free_while_there_was_budget_to_read_it() {
        assert!(delivered_free(
            &verdict_of(Tier::Important, true),
            true,
            None
        ));
    }

    /// The header fallback calls anything transactional-sounding important,
    /// so a sender who empties their own slice on purpose would otherwise
    /// have bought their way into the tier nobody pays for.
    #[test]
    fn a_sender_who_spent_their_own_slice_cannot_be_delivered_free_on_a_degraded_verdict() {
        assert!(!delivered_free(
            &verdict_of(Tier::Important, true),
            true,
            Some(BudgetState::SpentBySender)
        ));
    }

    /// A domain is bought once and its local parts are free after that, so a
    /// spent domain must buy no more than a spent address does.
    #[test]
    fn a_domain_that_spent_its_share_cannot_be_delivered_free_on_a_degraded_verdict() {
        assert!(!delivered_free(
            &verdict_of(Tier::Important, true),
            true,
            Some(BudgetState::SpentByDomain)
        ));
    }

    /// Emptying a handle's pool takes nothing but authenticated mail from a
    /// few domains, so it must not buy free delivery for whoever comes next.
    #[test]
    fn a_drained_handle_pool_does_not_open_the_free_tier_to_the_next_sender() {
        assert!(!delivered_free(
            &verdict_of(Tier::Important, true),
            true,
            Some(BudgetState::SpentByHandle)
        ));
    }

    #[test]
    fn unauthenticated_mail_is_never_delivered_free_on_a_degraded_verdict() {
        assert!(!delivered_free(
            &verdict_of(Tier::Important, true),
            false,
            Some(BudgetState::SpentBySender)
        ));
    }

    #[test]
    fn no_other_tier_is_free_however_the_verdict_was_reached() {
        for tier in [Tier::Human, Tier::Commercial, Tier::Dangerous] {
            assert!(
                !delivered_free(&verdict_of(tier, false), true, None),
                "{tier:?}"
            );
            assert!(
                !delivered_free(&verdict_of(tier, true), true, None),
                "{tier:?}"
            );
        }
    }

    /// This tier grants nothing, so taking a paid use for it would charge
    /// someone twice for one delivery.
    #[tokio::test]
    async fn the_free_tier_is_taken_before_any_pass_is_spent() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "paid", Some(1), NOW)
            .await
            .unwrap();

        let result = forward(&db, Tier::Important, true, None).await;

        assert_eq!(result, forwarded("important"));
        assert!(
            has_live_pass(&db, HANDLE, SENDER, NOW).await.unwrap(),
            "a delivery nobody was charged for must not spend what the sender paid"
        );
    }

    #[tokio::test]
    async fn a_live_pass_carries_a_message_that_was_not_called_dangerous() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "human", None, NOW)
            .await
            .unwrap();

        let result = forward(&db, Tier::Commercial, true, None).await;

        assert_eq!(result, forwarded("human"));
    }

    /// Proving personhood does not clear this tier: a real person can still
    /// be phishing.
    #[tokio::test]
    async fn dangerous_mail_is_not_forwarded_by_a_pass() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "human", None, NOW)
            .await
            .unwrap();

        assert_eq!(forward(&db, Tier::Dangerous, true, None).await, None);
    }

    /// The allowlist is keyed on the envelope sender, so honouring a pass for
    /// mail nobody could authenticate would let anyone through by writing
    /// someone else's name on it.
    #[tokio::test]
    async fn an_unauthenticated_sender_cannot_spend_the_pass_their_address_earned() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "human", None, NOW)
            .await
            .unwrap();

        let result = forward(
            &db,
            Tier::Commercial,
            false,
            Some(BudgetState::SpentBySender),
        )
        .await;

        assert_eq!(result, None);
    }

    /// An unlimited window plus a budget the sender exhausted themselves is a
    /// licence to deliver anything unread, so that one shuts.
    #[tokio::test]
    async fn a_sender_who_spent_their_own_slice_cannot_ride_an_unlimited_window() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "human", None, NOW)
            .await
            .unwrap();

        let result = forward(
            &db,
            Tier::Commercial,
            true,
            Some(BudgetState::SpentBySender),
        )
        .await;

        assert_eq!(result, None);
    }

    /// A single paid use cannot flood by construction, and refusing it would
    /// take the money and demand it again.
    #[tokio::test]
    async fn a_paid_delivery_still_goes_through_when_the_sender_spent_their_own_slice() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "paid", Some(1), NOW)
            .await
            .unwrap();

        let result = forward(
            &db,
            Tier::Commercial,
            true,
            Some(BudgetState::SpentBySender),
        )
        .await;

        assert_eq!(result, forwarded("paid"));
    }

    #[tokio::test]
    async fn a_pool_drained_by_other_people_does_not_re_challenge_a_sender_who_proved_themselves() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "human", None, NOW)
            .await
            .unwrap();

        let result = forward(
            &db,
            Tier::Commercial,
            true,
            Some(BudgetState::SpentByHandle),
        )
        .await;

        assert_eq!(result, forwarded("human"));
    }

    /// A domain rations a shared cost; it does not name a culprit.
    #[tokio::test]
    async fn a_domain_drained_by_others_does_not_re_challenge_a_sender_who_proved_themselves() {
        let db = TestDb::fresh().await;
        grant_pass(&db, HANDLE, SENDER, "human", None, NOW)
            .await
            .unwrap();

        let result = forward(
            &db,
            Tier::Commercial,
            true,
            Some(BudgetState::SpentByDomain),
        )
        .await;

        assert_eq!(result, forwarded("human"));
    }

    #[tokio::test]
    async fn a_stranger_holding_no_pass_is_left_to_the_challenge() {
        let db = TestDb::fresh().await;

        assert_eq!(forward(&db, Tier::Commercial, true, None).await, None);
    }
}

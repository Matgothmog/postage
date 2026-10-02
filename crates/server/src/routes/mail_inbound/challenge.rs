//! Pricing one held message and recording the challenge that stands between
//! it and the inbox (`web/src/app/api/mail/inbound/challenge.ts`).

use std::error::Error;

use alloy_primitives::{U256, hex};
use postage_core::classify::Verdict;
use postage_core::pricing::{Quote, SenderSignals, quote};
use postage_core::quote::{message_id_for, parse_address, sign_quote};
use postage_core::quote_types::{QuoteFields, StoredQuote};
use postage_shared::Tier;
use rand_core::{OsRng, TryRngCore};

use crate::app::AppState;
use crate::config::{classifier_signer, message_id_secret};
use crate::db::Db;
use crate::db::challenges::{
    HOLD_SECONDS, NewChallenge, QuoteRecord, create_challenge, purge_expired_holds,
};
use crate::db::sender_wallets::wallet_for_sender;
use crate::faults::{BoxError, FaultStage, during, require_configured};
use crate::reputation::gather_signals;

/// The message a challenge is issued for.
#[derive(Debug, Clone, Copy)]
pub(super) struct HeldMail<'a> {
    pub handle: &'a str,
    pub sender: &'a str,
    pub subject: &'a str,
    pub verdict: &'a Verdict,
    /// The inbox's wallet: the escrow pays it, and its floor prices the mail.
    pub wallet: &'a str,
    pub app_url: &'a str,
    /// Whether the receiving server confirmed the envelope sender. Recorded
    /// so a message pasted back in later is not relayed as if from someone we
    /// never checked.
    pub authenticated: bool,
}

/// What a sender is asked for, and where they are sent to answer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IssuedChallenge {
    pub token: String,
    /// 18-decimal USDC base units.
    pub price: u128,
    pub reasons: Vec<String>,
    pub quote: QuoteFields,
    pub challenge_url: String,
    /// `None` when nothing is being held. Dangerous mail is never delivered
    /// by any route, so there is nothing to hold.
    pub held_until: Option<i64>,
}

/// Prices one message and records the challenge for it.
///
/// Nothing about the message is stored. The worker keeps the original bytes
/// so that releasing a hold puts the message that was sent on the wire; this
/// records who wrote to whom and the price. Faults carry the stage that
/// failed; anything else (an unusable key, say) reaches the caller as an
/// unexpected failure, as it did in the TypeScript.
pub(super) async fn issue_challenge(
    state: &AppState,
    db: &Db,
    mail: &HeldMail<'_>,
) -> Result<IssuedChallenge, BoxError> {
    let priced = price(state, db, mail).await?;

    // Named before either is reached for, so an unset one reads as the
    // unfinished deployment it is rather than as a malformed key.
    require_configured(
        state.env(),
        &["MESSAGE_ID_SECRET", "CLASSIFIER_PRIVATE_KEY"],
    )?;

    let token = new_token()?;
    let received_at = state.now();
    let quote = signed_quote(state, mail, priced.amount, received_at)?;
    let tier = mail.verdict.tier;
    let held_until = (tier != Tier::Dangerous).then_some(received_at + HOLD_SECONDS);
    during(FaultStage::Database, purge_expired_holds(db, received_at)).await?;

    let stored = StoredQuote {
        quote: quote.clone(),
        reasons: priced.reasons.clone(),
    };
    let challenge = NewChallenge {
        token: token.clone(),
        handle: mail.handle.to_owned(),
        sender: mail.sender.to_owned(),
        message_id: quote.message_id.clone(),
        tier: tier.as_str().to_owned(),
        amount: priced.amount.to_string(),
        held_until,
        quote_json: serde_json::to_string(&QuoteRecord {
            quote: &stored,
            sender_verified: mail.authenticated,
        })?,
        created_at: received_at,
    };
    during(FaultStage::Database, create_challenge(db, &challenge)).await?;

    Ok(IssuedChallenge {
        challenge_url: format!("{}/c/{token}", mail.app_url),
        token,
        price: priced.amount,
        reasons: priced.reasons,
        quote,
        held_until,
    })
}

/// The price for this message: the inbox's floor, moved by the verdict and
/// by what is known about the sender.
async fn price(state: &AppState, db: &Db, mail: &HeldMail<'_>) -> Result<Quote, BoxError> {
    // A sender who has paid before is priced on that history rather than as a
    // stranger, which is the whole point of indexing payments.
    let sender_wallet = during(FaultStage::Database, wallet_for_sender(db, mail.sender)).await?;
    let signals = match sender_wallet {
        Some(wallet) => Some(sender_signals(state, &wallet).await?),
        None => None,
    };

    // The floor comes from the chain, never from our own database: the escrow
    // reverts on anything below it, so a cached copy that drifts produces
    // quotes nobody can pay. `effectiveFloor` rather than `floorPrice`, so an
    // inbox whose owner never picked a price is still charged for.
    let floor = during(FaultStage::Chain, effective_floor(state, mail.wallet)).await?;
    Ok(quote(
        floor,
        mail.verdict.tier,
        signals.as_ref(),
        mail.verdict.degraded,
        state.now(),
    ))
}

/// The quote the escrow will honour: this message's id, the inbox, the tier
/// and the amount, signed with the classifier's key.
fn signed_quote(
    state: &AppState,
    mail: &HeldMail<'_>,
    amount: u128,
    received_at: i64,
) -> Result<QuoteFields, BoxError> {
    let secret = message_id_secret(state.env().lookup())?;
    let message_id = message_id_for(&secret, mail.sender, mail.handle, mail.subject, received_at)?;
    let signer = classifier_signer(state.env().lookup())?;
    let signed = sign_quote(
        &signer,
        message_id,
        mail.wallet,
        mail.verdict.tier,
        U256::from(amount),
        received_at,
    )?;
    Ok(signed.fields())
}

/// What the network knows about a sender who has a wallet.
///
/// `gather_signals` softens both of its queries rather than failing, so the
/// settings saying where to send them are checked here, where a missing one
/// arrives as the misconfiguration it is.
async fn sender_signals(state: &AppState, wallet: &str) -> Result<SenderSignals, BoxError> {
    require_configured(state.env(), &["GRAPH_QUERY_URL", "GRAPH_API_KEY"])?;
    Ok(gather_signals(state.graph(), wallet).await)
}

/// `PostageEscrow.effectiveFloor(wallet)`. Everything that can go wrong in
/// reaching it, an unusable RPC URL or an inbox wallet that is not an
/// address included, is the chain stage's, as it was inside viem's
/// `readContract`.
async fn effective_floor(state: &AppState, wallet: &str) -> Result<u128, BoxError> {
    let chain = state.chain()?;
    let inbox = parse_address(wallet)?;
    let floor = chain.effective_floor(inbox).await?;
    u128::try_from(floor).map_err(|_| Box::<dyn Error + Send + Sync>::from(OversizeFloor(floor)))
}

/// A floor too large for the price arithmetic, which works in 128 bits. No
/// USDC amount comes near it; refused rather than truncated.
#[derive(Debug, thiserror::Error)]
#[error("the escrow reported a floor of {0}, too large to price with")]
struct OversizeFloor(U256);

/// `randomUUID().replaceAll("-", "")`: 32 lowercase hex digits of a version 4
/// UUID, the shape every stored token already has.
fn new_token() -> Result<String, BoxError> {
    let mut bytes = [0u8; 16];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|error| format!("no randomness for a challenge token: {error}"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reqwest::Url;

    use super::*;
    use crate::chain::Chain;
    use crate::config::Env;
    use crate::db::challenges::challenge_by_token;
    use crate::db::testing::TestDb;
    use crate::http_stub::Stub;
    use crate::routes::mail_inbound::tests_support::{FLOOR, WALLET, chain_node, test_env};
    use crate::routes::testing::{NOW, clock_at};

    const HANDLE: &str = "demo";
    /// Deliberately linked to no wallet: one that is would be priced on what
    /// The Graph knows about it.
    const SENDER: &str = "sender@x.com";
    const APP_URL: &str = "http://localhost";

    struct Fixture {
        db: Arc<TestDb>,
        state: AppState,
        _chain: Stub,
    }

    async fn fixture() -> Fixture {
        let db = Arc::new(TestDb::fresh().await);
        let chain = chain_node(false).await;
        let state = AppState::builder(test_env())
            .clock(clock_at(NOW))
            .db(db.clone())
            .chain(Chain::new(Url::parse(&chain.base).unwrap()))
            .build();
        Fixture {
            db,
            state,
            _chain: chain,
        }
    }

    impl Fixture {
        async fn issue(&self, tier: Tier) -> IssuedChallenge {
            self.issue_for(tier, true).await
        }

        async fn issue_for(&self, tier: Tier, authenticated: bool) -> IssuedChallenge {
            let verdict = Verdict {
                tier,
                confidence: 0.9,
                reasons: Vec::new(),
                degraded: false,
            };
            let mail = HeldMail {
                handle: HANDLE,
                sender: SENDER,
                subject: "hello",
                verdict: &verdict,
                wallet: WALLET,
                app_url: APP_URL,
                authenticated,
            };
            issue_challenge(&self.state, &self.db, &mail).await.unwrap()
        }
    }

    /// The escrow reverts on anything below the floor it holds, so a price
    /// that came from anywhere but the chain is a quote nobody can pay.
    #[tokio::test]
    async fn a_strangers_message_is_priced_from_the_floor_the_chain_reports() {
        let fixture = fixture().await;

        let issued = fixture.issue(Tier::Commercial).await;

        assert_eq!(issued.price, FLOOR);
    }

    #[tokio::test]
    async fn the_challenge_a_sender_is_sent_to_answer_is_recorded_against_its_token() {
        let fixture = fixture().await;

        let issued = fixture.issue(Tier::Commercial).await;

        let stored = challenge_by_token(&fixture.db, &issued.token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.handle, HANDLE);
        assert_eq!(stored.sender, SENDER);
        assert_eq!(stored.tier, "commercial");
        assert_eq!(stored.amount, issued.price.to_string());
    }

    /// Dangerous mail is never delivered by any route, so there is nothing to
    /// hold and no reason to keep what it said.
    #[tokio::test]
    async fn dangerous_mail_is_recorded_without_holding_anything() {
        let fixture = fixture().await;

        let issued = fixture.issue(Tier::Dangerous).await;

        assert_eq!(issued.held_until, None);
        let stored = challenge_by_token(&fixture.db, &issued.token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.held_until, None);
    }

    #[tokio::test]
    async fn a_held_message_is_kept_for_as_long_as_a_hold_lasts() {
        let fixture = fixture().await;

        let issued = fixture.issue(Tier::Commercial).await;

        let stored = challenge_by_token(&fixture.db, &issued.token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(issued.held_until, Some(stored.created_at + HOLD_SECONDS));
        assert_eq!(stored.held_until, issued.held_until);
    }

    /// The quote is what the escrow checks, so it has to name the inbox being
    /// paid and the price it was signed for.
    #[tokio::test]
    async fn the_quote_is_signed_for_the_inbox_that_gets_paid_at_the_price_that_was_quoted() {
        let fixture = fixture().await;

        let issued = fixture.issue(Tier::Commercial).await;

        assert_eq!(issued.quote.inbox, WALLET);
        assert_eq!(issued.quote.tier, "commercial");
        assert_eq!(issued.quote.amount, issued.price.to_string());
    }

    #[tokio::test]
    async fn the_sender_is_sent_to_the_page_their_own_token_names() {
        let fixture = fixture().await;

        let issued = fixture.issue(Tier::Commercial).await;

        assert_eq!(
            issued.challenge_url,
            format!("{APP_URL}/c/{}", issued.token)
        );
    }

    /// Kept with the quote because the challenge page explains the price from
    /// the stored row: the message it was quoted for is gone by then.
    #[tokio::test]
    async fn the_reasons_behind_a_price_are_stored_with_the_quote() {
        let fixture = fixture().await;

        let issued = fixture.issue(Tier::Commercial).await;

        let stored = challenge_by_token(&fixture.db, &issued.token)
            .await
            .unwrap()
            .unwrap();
        let quoted: StoredQuote = serde_json::from_str(&stored.quote_json).unwrap();
        assert_eq!(quoted.reasons, issued.reasons);
        assert_eq!(quoted.quote.signature, issued.quote.signature);
    }

    /// The stored quote keeps the TypeScript's key order, `{...signed,
    /// reasons}`, so rows written by either implementation read the same.
    #[tokio::test]
    async fn the_stored_quote_is_written_in_the_shape_the_typescript_wrote() {
        let fixture = fixture().await;

        let issued = fixture.issue(Tier::Commercial).await;

        let stored = challenge_by_token(&fixture.db, &issued.token)
            .await
            .unwrap()
            .unwrap();
        let keys: Vec<&str> = stored
            .quote_json
            .split('"')
            .skip(1)
            .step_by(2)
            .filter(|key| {
                [
                    "messageId",
                    "inbox",
                    "tier",
                    "amount",
                    "expiresAt",
                    "signature",
                    "reasons",
                ]
                .contains(key)
            })
            .collect();
        assert_eq!(
            keys,
            [
                "messageId",
                "inbox",
                "tier",
                "amount",
                "expiresAt",
                "signature",
                "reasons"
            ]
        );
    }

    #[tokio::test]
    async fn the_challenge_records_whether_its_sender_was_authenticated() {
        let fixture = fixture().await;

        let verified = fixture.issue_for(Tier::Commercial, true).await;
        let unverified = fixture.issue_for(Tier::Commercial, false).await;

        let stored = |token: String| {
            let db = &fixture.db;
            async move { challenge_by_token(db, &token).await.unwrap().unwrap() }
        };
        assert!(stored(verified.token).await.sender_verified());
        assert!(!stored(unverified.token).await.sender_verified());
    }

    #[test]
    fn a_token_is_a_dashless_version_four_uuid() {
        let token = new_token().unwrap();

        assert_eq!(token.len(), 32);
        assert!(
            token
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_eq!(&token[12..13], "4");
        assert!(matches!(&token[16..17], "8" | "9" | "a" | "b"));
        assert_ne!(token, new_token().unwrap());
    }

    /// A sender with a wallet is priced on the subgraphs, and the settings
    /// that reach them are named when missing rather than read as no history.
    #[tokio::test]
    async fn a_sender_with_a_wallet_and_no_subgraph_settings_is_a_named_config_fault() {
        let fixture = fixture().await;
        crate::db::sender_wallets::link_sender_wallet(&fixture.db, SENDER, WALLET, NOW)
            .await
            .unwrap();
        let state = AppState::builder(Env::empty()).build();
        let verdict = Verdict {
            tier: Tier::Commercial,
            confidence: 0.9,
            reasons: Vec::new(),
            degraded: false,
        };
        let mail = HeldMail {
            handle: HANDLE,
            sender: SENDER,
            subject: "hello",
            verdict: &verdict,
            wallet: WALLET,
            app_url: APP_URL,
            authenticated: true,
        };

        let error = issue_challenge(&state, &fixture.db, &mail)
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "GRAPH_QUERY_URL is not set on the gateway"
        );
    }
}

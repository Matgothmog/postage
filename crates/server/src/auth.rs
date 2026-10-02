//! Proving a request comes from someone holding a wallet (`web/src/lib/auth.ts`).
//!
//! Two things count as proof and the app issues both: Privy's identity token,
//! which lists the wallets Privy minted for whoever is signed in, and a
//! signature over a statement naming the wallet, carried with a single-use
//! nonce. Neither is the address itself: a wallet address is public, indexed
//! onchain, and handed to every sender who was ever gated.

use axum::http::HeaderMap;
use postage_core::quote::parse_address;
use postage_core::secret::WalletNonceKey;
use postage_core::wallet_nonce::verify_wallet_nonce;
use postage_core::wallet_proof::{OfferedProof, read_proof, with_nonce};
use postage_core::wallet_signature::verify_message;

use crate::db::Db;
use crate::db::spent_nonces::{purge_spent_wallet_nonces, spend_wallet_nonce};
use crate::log;
use crate::privy::PrivyVerifier;

pub use postage_core::wallet_proof::{claim_statement, confirm_statement, read_statement};

/// How far a signed statement's timestamp may lag or lead the server's clock
/// and still be trusted, in each direction, so the window is ten minutes wide.
/// The forward half exists because the timestamp comes from the signer's
/// device, whose clock may run fast.
///
/// Left wide on purpose now that a nonce is what actually stops a replay. This
/// absorbs a device clock we do not control, while `WALLET_NONCE_TTL_SECONDS`
/// (two minutes) is judged entirely on our own; narrowing this to match would
/// lock out every signer whose phone is a few minutes out and buy nothing.
pub const CLOCK_SKEW_TOLERANCE_SECONDS: i64 = 5 * 60;

/// What a wallet proof is checked against: the key nonces are minted under,
/// and the ledger of nonces already answered.
#[derive(Debug, Clone, Copy)]
pub struct WalletAuth<'a> {
    pub db: &'a Db,
    pub nonce_key: &'a WalletNonceKey,
}

impl WalletAuth<'_> {
    /// Whether the signature half of `offered` proves its wallet, now.
    ///
    /// The statement is rebuilt from the offered timestamp, the nonce line
    /// appended, and the signer recovered locally; nothing is asked of a
    /// chain, so no RPC can vouch for a forged signature. The nonce is spent
    /// last, and only for a signature that already verified, so nobody can
    /// burn a stranger's nonce by posting rubbish under it.
    ///
    /// Every refusal is `false`: a malformed signature, a stale timestamp and
    /// an unreachable ledger all end the same way for the caller.
    pub async fn proves_wallet<S>(&self, offered: &OfferedProof, statement: S, now: i64) -> bool
    where
        S: Fn(f64) -> String,
    {
        let (Some(wallet), Some(signature), Some(nonce)) = (
            present(offered.wallet.as_deref()),
            present(offered.signature.as_deref()),
            present(offered.nonce.as_deref()),
        ) else {
            return false;
        };
        if parse_address(wallet).is_err() || !is_fresh(offered.issued_at, now) {
            return false;
        }

        // Checked before the signature, because it is the cheap half and
        // because a nonce minted for a different wallet must not be spent by
        // this one.
        let Some(expires_at) = verify_wallet_nonce(self.nonce_key, nonce, wallet, now) else {
            return false;
        };

        let signed = with_nonce(&statement(offered.issued_at), nonce);
        if !verify_message(wallet, &signed, signature) {
            return false;
        }

        self.spend_nonce(nonce, expires_at, now).await
    }

    /// Whether this request was made by someone holding `wallet`: an identity
    /// token Privy issued for it, or else a signed proof naming it.
    ///
    /// The identity-token branch is the one thing a nonce does not cover. The
    /// token is Privy's bearer credential: anyone holding a copy is that
    /// session until it expires.
    pub async fn holds_wallet<S>(
        &self,
        privy: &PrivyVerifier,
        headers: &HeaderMap,
        wallet: &str,
        statement: S,
        now: i64,
    ) -> bool
    where
        S: Fn(f64) -> String,
    {
        let wanted = wallet.to_lowercase();
        let offered = read_proof(|name| header_text(headers, name));

        let identity = privy.read_identity(offered.identity_token.as_deref()).await;
        if identity.is_some_and(|identity| identity.wallets.contains(&wanted)) {
            return true;
        }

        // Checked against the wallet we are asking about before the signature
        // is verified, so a valid signature from some other wallet cannot
        // stand in.
        if offered.wallet.as_deref().map(str::to_lowercase) != Some(wanted) {
            return false;
        }

        self.proves_wallet(&offered, statement, now).await
    }

    /// Records the nonce as answered, and refuses the proof if it cannot.
    ///
    /// Not knowing whether this nonce has already been used is not a reason to
    /// let the signature through. The caller turns this into "sign in again",
    /// the wrong message for a database outage, so the log line is what tells
    /// an operator which it was. Nothing about the nonce is logged: it is a
    /// live credential until the row lands, and this is the path where it did
    /// not.
    async fn spend_nonce(&self, nonce: &str, expires_at: i64, now: i64) -> bool {
        // Dropped on the way past rather than left to grow a row per sign-in
        // forever; here rather than where nonces are minted, which touches no
        // database and is open to anyone.
        let spent = match purge_spent_wallet_nonces(self.db, now).await {
            Ok(()) => spend_wallet_nonce(self.db, nonce, expires_at).await,
            Err(error) => Err(error),
        };
        spent.unwrap_or_else(|error| {
            log::error(
                "spent wallet nonce ledger unavailable",
                &[("reason", &error)],
            );
            false
        })
    }
}

/// JavaScript's `!value` for a header: absent and empty are both nothing.
fn present(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

/// Inside the skew window either side of `now`. A timestamp that is not finite
/// is refused outright: NaN sits outside neither edge, so without this it
/// would be timeless rather than stale.
fn is_fresh(issued_at: f64, now: i64) -> bool {
    let tolerance = CLOCK_SKEW_TOLERANCE_SECONDS as f64;
    let age = now as f64 - issued_at;
    issued_at.is_finite() && (-tolerance..=tolerance).contains(&age)
}

/// A header as the Fetch API's `Headers.get` reads it: every value under the
/// name joined with ", ", each byte taken as one Latin-1 character.
fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    let values: Vec<String> = headers
        .get_all(name)
        .iter()
        .map(|value| value.as_bytes().iter().copied().map(char::from).collect())
        .collect();
    (!values.is_empty()).then(|| values.join(", "))
}

#[cfg(test)]
mod tests {
    //! Signatures here are real: `proves_wallet` recovers the signer, so a
    //! stubbed verdict would only show that the stub was consulted. There is
    //! no hostile RPC to stand behind these tests as there was in the
    //! TypeScript, because nothing here can reach one: the verifier takes no
    //! chain client, so "a signature was settled by the chain" cannot happen.

    use std::sync::Arc;

    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;
    use axum::http::HeaderValue;
    use futures_util::FutureExt;
    use futures_util::future::BoxFuture;
    use postage_core::handle::MAIL_DOMAIN;
    use postage_core::secret::MessageIdSecret;
    use postage_core::wallet_nonce::{WALLET_NONCE_TTL_SECONDS, mint_wallet_nonce};
    use postage_core::wallet_proof::{NONCE_HEADER, signed_proof};
    use rand_core::{OsRng, TryRngCore};

    use super::*;
    use crate::db::testing::TestDb;
    use crate::http_stub::{Reply, Stub, serve_with};
    use crate::log::captured;
    use crate::privy::{JwksError, JwksFetch, JwksResponse};

    const TOLERANCE: f64 = CLOCK_SKEW_TOLERANCE_SECONDS as f64;

    /// 2026-09-08T12:00:00Z. Both edges of the window are one exact second, so
    /// the clock stands still for every test.
    const FROZEN: i64 = 1_788_868_800;
    const NOW: f64 = FROZEN as f64;

    const SECRET: &str = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";

    fn holder() -> PrivateKeySigner {
        PrivateKeySigner::from_slice(&[0x11; 32]).unwrap()
    }

    fn impostor() -> PrivateKeySigner {
        PrivateKeySigner::from_slice(&[0x22; 32]).unwrap()
    }

    fn wallet() -> String {
        holder().address().to_checksum(None)
    }

    fn nonce_key() -> WalletNonceKey {
        WalletNonceKey::derive(&MessageIdSecret::new(SECRET).unwrap()).unwrap()
    }

    fn mint(wallet: &str) -> String {
        mint_wallet_nonce(&nonce_key(), wallet, FROZEN, &mut OsRng.unwrap_err())
    }

    fn statement(issued_at: f64) -> String {
        read_statement(&wallet(), issued_at)
    }

    /// `personal_sign`, as a wallet does it.
    fn sign_text(by: &PrivateKeySigner, text: &str) -> String {
        let signature = by.sign_message_sync(text.as_bytes()).unwrap();
        format!("0x{}", alloy_primitives::hex::encode(signature.as_bytes()))
    }

    fn sign(by: &PrivateKeySigner, issued_at: f64, nonce: &str) -> String {
        sign_text(by, &with_nonce(&statement(issued_at), nonce))
    }

    fn offer(
        wallet: Option<&str>,
        issued_at: f64,
        signature: Option<&str>,
        nonce: Option<&str>,
    ) -> OfferedProof {
        OfferedProof {
            identity_token: None,
            wallet: wallet.map(str::to_owned),
            issued_at,
            signature: signature.map(str::to_owned),
            nonce: nonce.map(str::to_owned),
        }
    }

    /// One database and one fresh nonce per test, so a signature and the
    /// reader checking it are a matching pair.
    struct Harness {
        db: TestDb,
        key: WalletNonceKey,
        nonce: String,
    }

    impl Harness {
        async fn new() -> Self {
            Self {
                db: TestDb::fresh().await,
                key: nonce_key(),
                nonce: mint(&wallet()),
            }
        }

        fn auth(&self) -> WalletAuth<'_> {
            WalletAuth {
                db: &self.db,
                nonce_key: &self.key,
            }
        }

        /// The holder's proof for `issued_at` under this test's nonce,
        /// checked against the read statement at the frozen clock.
        async fn proves_signed_at(&self, issued_at: f64) -> bool {
            let signature = sign(&holder(), issued_at, &self.nonce);
            self.proves(&offer(
                Some(&wallet()),
                issued_at,
                Some(&signature),
                Some(&self.nonce),
            ))
            .await
        }

        async fn proves(&self, offered: &OfferedProof) -> bool {
            self.auth().proves_wallet(offered, statement, FROZEN).await
        }
    }

    #[tokio::test]
    async fn a_statement_signed_this_second_proves_the_wallet() {
        assert!(Harness::new().await.proves_signed_at(NOW).await);
    }

    #[tokio::test]
    async fn a_statement_signed_exactly_five_minutes_ago_still_proves_the_wallet() {
        assert!(Harness::new().await.proves_signed_at(NOW - TOLERANCE).await);
    }

    #[tokio::test]
    async fn a_statement_signed_five_minutes_and_one_second_ago_proves_nothing() {
        assert!(
            !Harness::new()
                .await
                .proves_signed_at(NOW - TOLERANCE - 1.0)
                .await
        );
    }

    /// The window opens forwards as well as back, because the timestamp is
    /// the signer's and their clock is not ours.
    #[tokio::test]
    async fn a_statement_dated_exactly_five_minutes_ahead_is_allowed_for_a_clock_that_runs_fast() {
        assert!(Harness::new().await.proves_signed_at(NOW + TOLERANCE).await);
    }

    #[tokio::test]
    async fn a_statement_dated_five_minutes_and_one_second_ahead_proves_nothing() {
        assert!(
            !Harness::new()
                .await
                .proves_signed_at(NOW + TOLERANCE + 1.0)
                .await
        );
    }

    /// NaN fails both halves of the window comparison, so only the finite
    /// check stands between it and a signature that is replayable forever.
    #[tokio::test]
    async fn a_timestamp_of_nan_is_stale_rather_than_timeless() {
        let age = NOW - f64::NAN;
        assert!(
            age.partial_cmp(&-TOLERANCE).is_none() && age.partial_cmp(&TOLERANCE).is_none(),
            "precondition: NaN sits outside neither edge, so only the finite check can refuse it"
        );

        assert!(!Harness::new().await.proves_signed_at(f64::NAN).await);
    }

    /// The timestamp is whatever a header a caller controls converts to.
    #[tokio::test]
    async fn a_header_that_is_not_a_number_at_all_proves_nothing() {
        let issued_at = read_proof(|name| {
            (name == postage_core::wallet_proof::ISSUED_AT_HEADER).then(|| "whenever".to_owned())
        })
        .issued_at;
        assert!(issued_at.is_nan());

        assert!(!Harness::new().await.proves_signed_at(issued_at).await);
    }

    #[tokio::test]
    async fn a_header_that_was_never_sent_proves_nothing() {
        let issued_at = read_proof(|_| None).issued_at;

        assert!(!Harness::new().await.proves_signed_at(issued_at).await);
    }

    #[tokio::test]
    async fn an_infinite_timestamp_proves_nothing_in_either_direction() {
        let harness = Harness::new().await;

        assert!(!harness.proves_signed_at(f64::INFINITY).await);
        assert!(!harness.proves_signed_at(f64::NEG_INFINITY).await);
    }

    #[tokio::test]
    async fn a_request_naming_no_wallet_proves_nothing() {
        let harness = Harness::new().await;
        let signature = sign(&holder(), NOW, &harness.nonce);

        assert!(
            !harness
                .proves(&offer(None, NOW, Some(&signature), Some(&harness.nonce)))
                .await
        );
    }

    #[tokio::test]
    async fn a_wallet_that_is_not_an_address_proves_nothing() {
        let harness = Harness::new().await;
        let signature = sign(&holder(), NOW, &harness.nonce);

        assert!(
            !harness
                .proves(&offer(
                    Some("0xnot-an-address"),
                    NOW,
                    Some(&signature),
                    Some(&harness.nonce)
                ))
                .await
        );
    }

    #[tokio::test]
    async fn a_request_carrying_no_signature_proves_nothing() {
        let harness = Harness::new().await;

        assert!(
            !harness
                .proves(&offer(Some(&wallet()), NOW, None, Some(&harness.nonce)))
                .await
        );
    }

    /// The forgery an attacker can actually mount: they hold a key, just not
    /// this one.
    #[tokio::test]
    async fn a_signature_from_another_wallet_proves_nothing() {
        let harness = Harness::new().await;
        let forged = sign(&impostor(), NOW, &harness.nonce);

        assert!(
            !harness
                .proves(&offer(
                    Some(&wallet()),
                    NOW,
                    Some(&forged),
                    Some(&harness.nonce)
                ))
                .await
        );
    }

    /// A `v` that names no recovery id. A refusal, not a panic.
    #[tokio::test]
    async fn a_malformed_signature_proves_nothing_rather_than_escaping() {
        let harness = Harness::new().await;
        let nonsense = format!("0x{}", "ab".repeat(65));

        assert!(
            !harness
                .proves(&offer(
                    Some(&wallet()),
                    NOW,
                    Some(&nonsense),
                    Some(&harness.nonce)
                ))
                .await
        );
    }

    /// The statement carries the timestamp, which is what stops a signature
    /// collected once from proving anything at a second moment in the window.
    #[tokio::test]
    async fn a_signature_over_one_timestamp_proves_nothing_about_another() {
        let harness = Harness::new().await;
        let signed_at = NOW - 120.0;
        let replayed_at = NOW - 60.0;

        assert!(harness.proves_signed_at(signed_at).await);

        // A second nonce and a second signature at the same `signed_at`, so
        // the refusal below is the timestamp binding's and not the ledger's.
        let fresh = mint(&wallet());
        let collected = sign(&holder(), signed_at, &fresh);

        assert!(
            !harness
                .proves(&offer(
                    Some(&wallet()),
                    replayed_at,
                    Some(&collected),
                    Some(&fresh)
                ))
                .await
        );
    }

    #[tokio::test]
    async fn a_wallet_named_in_lowercase_proves_the_same_wallet() {
        let harness = Harness::new().await;
        let signature = sign(&holder(), NOW, &harness.nonce);

        assert!(
            harness
                .proves(&offer(
                    Some(&wallet().to_lowercase()),
                    NOW,
                    Some(&signature),
                    Some(&harness.nonce)
                ))
                .await
        );
    }

    /// The replay a timestamp could never stop on its own: the same bytes, at
    /// the same instant, over the same text, by the same wallet.
    #[tokio::test]
    async fn a_proof_that_verified_once_proves_nothing_the_second_time() {
        let harness = Harness::new().await;
        let collected = sign(&holder(), NOW, &harness.nonce);
        let offered = offer(Some(&wallet()), NOW, Some(&collected), Some(&harness.nonce));

        assert!(harness.proves(&offered).await);
        assert!(!harness.proves(&offered).await);
    }

    #[tokio::test]
    async fn a_request_carrying_no_nonce_at_all_proves_nothing() {
        let harness = Harness::new().await;
        let signature = sign(&holder(), NOW, &harness.nonce);

        assert!(
            !harness
                .proves(&offer(Some(&wallet()), NOW, Some(&signature), None))
                .await
        );
    }

    /// A nonce nobody minted, under a genuine signature.
    #[tokio::test]
    async fn a_forged_nonce_proves_nothing_however_real_the_signature_over_it_is() {
        let harness = Harness::new().await;
        let forged = format!("{}.{}.{}", FROZEN + 120, "ab".repeat(16), "cd".repeat(32));
        let collected = sign(&holder(), NOW, &forged);

        assert!(
            !harness
                .proves(&offer(
                    Some(&wallet()),
                    NOW,
                    Some(&collected),
                    Some(&forged)
                ))
                .await
        );
    }

    /// Inside the skew tolerance, so only the nonce's own window refuses it.
    #[tokio::test]
    async fn a_nonce_past_its_own_window_proves_nothing_inside_the_skew_tolerance_or_not() {
        let harness = Harness::new().await;
        let collected = sign(&holder(), NOW, &harness.nonce);
        // Precondition: there is a gap between the two windows.
        const { assert!(WALLET_NONCE_TTL_SECONDS < CLOCK_SKEW_TOLERANCE_SECONDS) };
        let later = FROZEN + WALLET_NONCE_TTL_SECONDS + 1;

        let offered = offer(Some(&wallet()), NOW, Some(&collected), Some(&harness.nonce));
        assert!(
            !harness
                .auth()
                .proves_wallet(&offered, statement, later)
                .await
        );
    }

    #[tokio::test]
    async fn a_nonce_minted_for_another_wallet_proves_nothing_here() {
        let harness = Harness::new().await;
        let elsewhere = mint(&impostor().address().to_checksum(None));
        let collected = sign(&holder(), NOW, &elsewhere);

        assert!(
            !harness
                .proves(&offer(
                    Some(&wallet()),
                    NOW,
                    Some(&collected),
                    Some(&elsewhere)
                ))
                .await
        );
    }

    /// What a client half-migrated to the nonce contract would send.
    #[tokio::test]
    async fn a_signature_that_does_not_cover_the_nonce_proves_nothing() {
        let harness = Harness::new().await;
        let without_nonce = sign_text(&holder(), &statement(NOW));

        assert!(
            !harness
                .proves(&offer(
                    Some(&wallet()),
                    NOW,
                    Some(&without_nonce),
                    Some(&harness.nonce)
                ))
                .await
        );
    }

    /// Refusing a bad signature must not cost the nonce it was offered under.
    #[tokio::test]
    async fn a_nonce_survives_a_signature_that_fails_to_verify() {
        let harness = Harness::new().await;
        let forged = sign(&impostor(), NOW, &harness.nonce);

        assert!(
            !harness
                .proves(&offer(
                    Some(&wallet()),
                    NOW,
                    Some(&forged),
                    Some(&harness.nonce)
                ))
                .await
        );
        assert!(harness.proves_signed_at(NOW).await);
    }

    /// The TypeScript reader signed over the number `Number()` made of the
    /// header, written back by a template literal. A fractional timestamp, or
    /// one sent as hex, has to verify against exactly that text here too, so
    /// the signed text below is spelled out by hand rather than rebuilt.
    #[tokio::test]
    async fn a_timestamp_header_is_signed_over_as_javascript_would_write_its_number() {
        let harness = Harness::new().await;
        let wallet = wallet();
        let text = |issued: &str, nonce: &str| {
            format!(
                "Postage: read my inbox\nDomain: {MAIL_DOMAIN}\nWallet: {}\nIssued: {issued}\nNonce: {nonce}",
                wallet.to_lowercase()
            )
        };
        let header = |value: String| {
            move |name: &str| {
                (name == postage_core::wallet_proof::ISSUED_AT_HEADER).then(|| value.clone())
            }
        };

        let fractional = read_proof(header(format!("{FROZEN}.5"))).issued_at;
        let signature = sign_text(&holder(), &text(&format!("{FROZEN}.5"), &harness.nonce));
        assert!(
            harness
                .proves(&offer(
                    Some(&wallet),
                    fractional,
                    Some(&signature),
                    Some(&harness.nonce)
                ))
                .await
        );

        let fresh = mint(&wallet);
        let hexadecimal = read_proof(header(format!("0x{FROZEN:x}"))).issued_at;
        let signature = sign_text(&holder(), &text(&FROZEN.to_string(), &fresh));
        assert!(
            harness
                .proves(&offer(
                    Some(&wallet),
                    hexadecimal,
                    Some(&signature),
                    Some(&fresh)
                ))
                .await
        );
    }

    // statements.test.ts: the statements bind the deployment, end to end.

    const OTHER_DOMAIN: &str = "usepostage.example";

    /// Another deployment asking the same wallet to sign: the domain is the
    /// only difference between the two texts.
    fn as_other_deployment(statement: &str) -> String {
        statement.replace(MAIL_DOMAIN, OTHER_DOMAIN)
    }

    #[tokio::test]
    async fn a_read_signature_collected_on_another_deployment_proves_nothing_here() {
        let harness = Harness::new().await;
        let elsewhere = as_other_deployment(&statement(NOW));
        let signature = sign_text(&holder(), &with_nonce(&elsewhere, &harness.nonce));

        assert!(
            !harness
                .proves(&offer(
                    Some(&wallet()),
                    NOW,
                    Some(&signature),
                    Some(&harness.nonce)
                ))
                .await
        );
    }

    #[tokio::test]
    async fn a_read_signature_collected_on_this_deployment_still_proves_the_wallet() {
        let harness = Harness::new().await;
        let signature = sign_text(&holder(), &with_nonce(&statement(NOW), &harness.nonce));

        assert!(
            harness
                .proves(&offer(
                    Some(&wallet()),
                    NOW,
                    Some(&signature),
                    Some(&harness.nonce)
                ))
                .await
        );
    }

    #[tokio::test]
    async fn a_confirm_signature_collected_on_another_deployment_proves_nothing_here() {
        let harness = Harness::new().await;
        let confirm = |at: f64| confirm_statement("demo", &wallet(), at);
        let elsewhere = as_other_deployment(&confirm(NOW));
        let signature = sign_text(&holder(), &with_nonce(&elsewhere, &harness.nonce));
        let offered = offer(Some(&wallet()), NOW, Some(&signature), Some(&harness.nonce));

        assert!(
            !harness
                .auth()
                .proves_wallet(&offered, confirm, FROZEN)
                .await
        );
    }

    // wallet-proof.test.ts: what the browser writes, the reader accepts.

    /// A Privy JWKS that never answers, so no identity token is ever read and
    /// every request below is decided by its signature.
    struct NoJwks;

    impl JwksFetch for NoJwks {
        fn fetch(&self, _url: String) -> BoxFuture<'static, Result<JwksResponse, JwksError>> {
            async { Err(JwksError::Transport("no JWKS in tests".to_owned())) }.boxed()
        }
    }

    fn privy() -> PrivyVerifier {
        PrivyVerifier::new("test-app".to_owned(), Arc::new(NoJwks), Arc::new(|| FROZEN))
    }

    /// The browser's half of the contract, built from the core functions it
    /// runs: a nonce minted for the wallet (what `/api/wallet-nonce` hands
    /// back), the statement at the browser's own clock with the nonce line
    /// appended, the wallet's `personal_sign`, and the headers `signed_proof`
    /// writes.
    fn browser_proof(
        by: &PrivateKeySigner,
        wallet: &str,
        statement: impl Fn(f64) -> String,
    ) -> HeaderMap {
        let nonce = mint(wallet);
        let signature = sign_text(by, &with_nonce(&statement(NOW), &nonce));
        let mut headers = HeaderMap::new();
        for (name, value) in signed_proof(wallet, NOW, &signature, &nonce) {
            headers.insert(name, HeaderValue::from_str(&value).unwrap());
        }
        headers
    }

    #[tokio::test]
    async fn a_proof_the_browser_writes_is_accepted_by_the_reader_on_the_other_side() {
        let harness = Harness::new().await;
        let headers = browser_proof(&holder(), &wallet(), statement);

        assert!(
            harness
                .auth()
                .holds_wallet(&privy(), &headers, &wallet(), statement, FROZEN)
                .await
        );
    }

    /// The statement is load-bearing in that trip, not decoration.
    #[tokio::test]
    async fn a_proof_written_for_one_statement_is_refused_by_a_reader_checking_another() {
        let harness = Harness::new().await;
        let headers = browser_proof(&holder(), &wallet(), statement);
        let confirm = |at: f64| confirm_statement("demo", &wallet(), at);

        assert!(
            !harness
                .auth()
                .holds_wallet(&privy(), &headers, &wallet(), confirm, FROZEN)
                .await
        );
    }

    #[tokio::test]
    async fn a_proof_naming_one_wallet_but_signed_by_another_is_refused() {
        let harness = Harness::new().await;
        let headers = browser_proof(&impostor(), &wallet(), statement);

        assert!(
            !harness
                .auth()
                .holds_wallet(&privy(), &headers, &wallet(), statement, FROZEN)
                .await
        );
    }

    /// The replay, end to end and byte for byte.
    #[tokio::test]
    async fn a_proof_the_browser_writes_is_accepted_once_and_refused_on_its_second_use() {
        let harness = Harness::new().await;
        let headers = browser_proof(&holder(), &wallet(), statement);
        let auth = harness.auth();

        assert!(
            auth.holds_wallet(&privy(), &headers, &wallet(), statement, FROZEN)
                .await
        );
        assert!(
            !auth
                .holds_wallet(&privy(), &headers, &wallet(), statement, FROZEN)
                .await
        );
    }

    /// What the ledger refuses is the replay, not the wallet.
    #[tokio::test]
    async fn a_wallet_can_prove_itself_again_with_a_proof_of_its_own() {
        let harness = Harness::new().await;
        let auth = harness.auth();
        let first = browser_proof(&holder(), &wallet(), statement);
        auth.holds_wallet(&privy(), &first, &wallet(), statement, FROZEN)
            .await;

        let second = browser_proof(&holder(), &wallet(), statement);

        assert!(
            auth.holds_wallet(&privy(), &second, &wallet(), statement, FROZEN)
                .await
        );
    }

    #[tokio::test]
    async fn a_proof_arriving_without_its_nonce_header_is_refused() {
        let harness = Harness::new().await;
        let mut headers = browser_proof(&holder(), &wallet(), statement);
        headers.remove(NONCE_HEADER);

        assert!(
            !harness
                .auth()
                .holds_wallet(&privy(), &headers, &wallet(), statement, FROZEN)
                .await
        );
    }

    /// A valid proof for one wallet does not answer for another.
    #[tokio::test]
    async fn a_proof_for_one_wallet_is_refused_when_another_is_asked_about() {
        let harness = Harness::new().await;
        let headers = browser_proof(&holder(), &wallet(), statement);
        let other = impostor().address().to_checksum(None);

        assert!(
            !harness
                .auth()
                .holds_wallet(&privy(), &headers, &other, statement, FROZEN)
                .await
        );
    }

    #[test]
    fn a_header_sent_twice_reads_as_the_fetch_api_joins_it() {
        let mut headers = HeaderMap::new();
        headers.append("x-postage-wallet", HeaderValue::from_static("0xa"));
        headers.append("x-postage-wallet", HeaderValue::from_static("0xb"));
        headers.append("x-other", HeaderValue::from_bytes(b"caf\xe9").unwrap());

        assert_eq!(
            header_text(&headers, "x-postage-wallet").as_deref(),
            Some("0xa, 0xb")
        );
        assert_eq!(
            header_text(&headers, "x-other").as_deref(),
            Some("caf\u{e9}")
        );
        assert_eq!(header_text(&headers, "x-missing"), None);
    }

    // auth-database-fault.test.ts: a ledger that cannot be reached.

    /// Long enough to be mistaken for a real one, so asserting it never
    /// reaches the log asserts something redaction would have to catch.
    const AUTH_TOKEN: &str = "not-a-real-turso-token-but-long-enough-to-look-like-one";

    const REFUSAL: &str = "SQLITE_UNKNOWN: the server is not accepting queries";

    /// A database that accepts the connection and refuses every request, the
    /// shape the live outage had: Turso reachable and not serving. A loopback
    /// HTTP server answering 500 to every Hrana request is that, one layer
    /// down from the TypeScript's stub of the same thing; a closed port would
    /// be a different failure (a transport error the client words itself).
    async fn broken_database() -> (Stub, Db) {
        let stub = serve_with(|_| Reply::new(500, REFUSAL)).await;
        let db = Db::connect(&stub.base, AUTH_TOKEN).await.unwrap();
        (stub, db)
    }

    /// Runs a proof that is good in every other way against the broken
    /// ledger, returning the verdict, the lines it logged and its nonce.
    async fn prove_against_broken_ledger() -> (bool, Vec<String>, String) {
        let (_stub, db) = broken_database().await;
        let key = nonce_key();
        let auth = WalletAuth {
            db: &db,
            nonce_key: &key,
        };
        let nonce = mint(&wallet());
        let signature = sign(&holder(), NOW, &nonce);
        let offered = offer(Some(&wallet()), NOW, Some(&signature), Some(&nonce));

        let (proven, lines) =
            captured::during(auth.proves_wallet(&offered, statement, FROZEN)).await;
        (proven, lines, nonce)
    }

    /// Not knowing whether this nonce was answered is no reason to let it
    /// through; an outage must not turn signatures back into bearer
    /// credentials. The function's contract is a bool, so the database error
    /// cannot escape as anything else.
    #[tokio::test]
    async fn a_proof_good_in_every_other_way_is_refused_when_the_nonce_ledger_is_unreachable() {
        let (proven, _, _) = prove_against_broken_ledger().await;

        assert!(!proven);
    }

    /// An operator has to tell a database outage from a wrong signature.
    #[tokio::test]
    async fn the_refusal_is_explained_in_the_log_rather_than_passed_off_as_a_bad_signature() {
        let (_, lines, _) = prove_against_broken_ledger().await;

        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("ledger unavailable"), "{lines:?}");
    }

    /// The nonce is still live here, and the token and secret are the same
    /// class of thing.
    #[tokio::test]
    async fn nothing_worth_stealing_reaches_the_log() {
        let (_, lines, nonce) = prove_against_broken_ledger().await;

        let logged = lines.join("\n");
        assert!(!logged.is_empty());
        assert!(!logged.contains(&nonce), "the log quotes the nonce back");
        assert!(
            !logged.contains(AUTH_TOKEN),
            "the log quotes the database credential back"
        );
        assert!(!logged.contains(SECRET), "the log quotes the secret back");
    }
}

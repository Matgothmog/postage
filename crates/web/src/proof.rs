//! Proving a wallet to the server. Replaces
//! `web/src/app/claim-inbox-helpers.ts`.
//!
//! The three things a proof needs from outside (a wallet prompt, a nonce from
//! the server, the clock) sit behind `ProofIo`, so the rules below run
//! natively against a fake and the components pass the browser one.

use alloy_primitives::Address;
use postage_core::handle::{handle_of, normalize_handle};
use postage_core::statements::claim_statement;
use postage_core::time::seconds_from_millis;
use postage_core::wallet_proof::{ProofHeaders, identity_proof, signed_proof, with_nonce};

use crate::api::{self, NonceUnavailable, SignedClaim};
use crate::bridge::BridgeError;
use crate::bridge::privy::Privy;

/// What a proof reaches outside for.
#[allow(async_fn_in_trait)] // single-threaded wasm: the futures need not be `Send`
pub trait ProofIo {
    /// Personal-sign `message` with `wallet`, with no wallet UI.
    async fn sign_message(&self, message: &str, wallet: Address) -> Result<String, BridgeError>;
    /// A fresh nonce for `wallet` to sign under.
    async fn wallet_nonce(&self, wallet: Address) -> Result<String, NonceUnavailable>;
    /// Whole seconds since the epoch.
    fn now_seconds(&self) -> i64;
}

/// The real thing: Privy signs, the API mints nonces, the browser has the
/// clock.
#[derive(Debug, Clone, Copy)]
pub struct BrowserProofIo {
    privy: Privy,
}

impl BrowserProofIo {
    pub fn new(privy: Privy) -> Self {
        Self { privy }
    }
}

impl ProofIo for BrowserProofIo {
    async fn sign_message(&self, message: &str, wallet: Address) -> Result<String, BridgeError> {
        self.privy.sign_message(message, wallet).await
    }

    async fn wallet_nonce(&self, wallet: Address) -> Result<String, NonceUnavailable> {
        api::request_wallet_nonce(wallet).await
    }

    fn now_seconds(&self) -> i64 {
        // The browser's clock is milliseconds as an f64; whole seconds are
        // what every timestamp the server compares against holds.
        seconds_from_millis(js_sys::Date::now() as i64)
    }
}

/// Why a proof could not be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProofError {
    #[error(transparent)]
    Nonce(#[from] NonceUnavailable),
    /// The wallet declined or failed to sign; Privy's own message, without the
    /// error code the `BridgeError` display appends.
    #[error("{0}")]
    Sign(String),
}

impl From<BridgeError> for ProofError {
    fn from(error: BridgeError) -> Self {
        Self::Sign(match error {
            BridgeError::Sdk { message, .. } => message,
            other => other.to_string(),
        })
    }
}

/// Headers proving the wallet to the server, by whichever route this session
/// has. Privy's identity token already names the wallets it minted; a session
/// without one signs a statement saying what it is about to do. The address
/// alone proves nothing: it is public, and every gated sender is handed one.
///
/// The nonce is fetched before the wallet is prompted, not after, because it
/// is part of what gets signed: a signature collected without one answers no
/// particular request and is refused by every reader.
pub async fn wallet_proof(
    io: &impl ProofIo,
    identity_token: Option<&str>,
    wallet: Address,
    statement: impl Fn(f64) -> String,
) -> Result<ProofHeaders, ProofError> {
    if let Some(token) = identity_token {
        return Ok(identity_proof(token));
    }

    let nonce = io.wallet_nonce(wallet).await?;
    let issued_at = io.now_seconds() as f64;
    let signature = io
        .sign_message(&with_nonce(&statement(issued_at), &nonce), wallet)
        .await?;
    Ok(signed_proof(
        &wallet.to_string(),
        issued_at,
        &signature,
        &nonce,
    ))
}

/// Best effort. A signature is the only proof for someone whose session cannot
/// be read from an identity token, and redundant for everyone else, so a
/// wallet that refuses to sign is not on its own a reason to stop.
///
/// A nonce that cannot be fetched lands in the same `None` as a refused
/// prompt, and for the same reason: what comes back either proves the wallet
/// or does not, and half a proof is no more use to the caller than none.
pub async fn sign_claim(
    io: &impl ProofIo,
    handle: &str,
    destination: &str,
    wallet: Address,
) -> Option<SignedClaim> {
    let nonce = io.wallet_nonce(wallet).await.ok()?;
    let issued_at = io.now_seconds();
    let statement = claim_statement(handle, destination, &wallet.to_string(), issued_at as f64);
    let signature = io
        .sign_message(&with_nonce(&statement, &nonce), wallet)
        .await
        .ok()?;
    Some(SignedClaim {
        issued_at,
        signature,
        nonce,
    })
}

/// Turns an email address into the handle its owner would probably have
/// picked, so the field arrives filled in rather than empty. The charset,
/// length and no-doubled-dots rules are the shared `normalize_handle`; this
/// only supplies the email-to-local-part step.
pub fn suggest_handle(email: Option<&str>) -> String {
    email.map_or_else(String::new, |email| normalize_handle(&handle_of(email)))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use alloy_primitives::address;
    use futures::executor::block_on;
    use postage_core::handle::MAIL_DOMAIN;
    use postage_core::wallet_proof::{
        IDENTITY_TOKEN_HEADER, ISSUED_AT_HEADER, NONCE_HEADER, SIGNATURE_HEADER, WALLET_HEADER,
    };

    use super::*;

    const WALLET: Address = address!("0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7");
    const NONCE: &str = "nonce-from-the-server";

    /// A scripted `ProofIo`: records what it was asked to sign and how many
    /// nonces it handed out.
    struct FakeIo {
        signature: Result<String, BridgeError>,
        nonce: Result<String, NonceUnavailable>,
        signed: RefCell<Vec<String>>,
        nonces_asked: RefCell<u32>,
    }

    impl FakeIo {
        fn signing(signature: &str) -> Self {
            Self {
                signature: Ok(signature.to_owned()),
                nonce: Ok(NONCE.to_owned()),
                signed: RefCell::default(),
                nonces_asked: RefCell::default(),
            }
        }

        fn refusing_to_sign() -> Self {
            Self {
                signature: Err(BridgeError::Sdk {
                    code: "4001".to_owned(),
                    message: "user closed the wallet prompt".to_owned(),
                }),
                ..Self::signing("unused")
            }
        }

        fn without_a_nonce(mut self) -> Self {
            self.nonce = Err(NonceUnavailable);
            self
        }
    }

    impl ProofIo for FakeIo {
        async fn sign_message(
            &self,
            message: &str,
            _wallet: Address,
        ) -> Result<String, BridgeError> {
            self.signed.borrow_mut().push(message.to_owned());
            self.signature.clone()
        }

        async fn wallet_nonce(&self, _wallet: Address) -> Result<String, NonceUnavailable> {
            *self.nonces_asked.borrow_mut() += 1;
            self.nonce.clone()
        }

        fn now_seconds(&self) -> i64 {
            1_757_332_800
        }
    }

    fn header<'a>(headers: &'a ProofHeaders, name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(header, _)| *header == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn wallet_proof_returns_the_identity_token_header_without_signing_when_one_is_present() {
        let io = FakeIo::signing("unused");
        let headers = block_on(wallet_proof(&io, Some("token-abc"), WALLET, |_| {
            "statement".to_owned()
        }))
        .unwrap();
        assert_eq!(
            headers,
            vec![(IDENTITY_TOKEN_HEADER, "token-abc".to_owned())]
        );
        assert!(io.signed.borrow().is_empty());
    }

    #[test]
    fn wallet_proof_signs_the_statement_and_returns_wallet_headers_when_there_is_no_identity_token()
    {
        let io = FakeIo::signing("0xSignature");
        let headers = block_on(wallet_proof(&io, None, WALLET, |at| {
            format!("statement at {at}")
        }))
        .unwrap();
        assert_eq!(
            header(&headers, WALLET_HEADER),
            Some("0x4469E869433Cf6Cc08DD54AFc6AC7e288b9A38f7")
        );
        assert_eq!(header(&headers, SIGNATURE_HEADER), Some("0xSignature"));
        assert_eq!(header(&headers, ISSUED_AT_HEADER), Some("1757332800"));
        assert_eq!(header(&headers, NONCE_HEADER), Some(NONCE));
    }

    /// The nonce has to be inside what the wallet put its key to, not merely
    /// alongside it.
    #[test]
    fn wallet_proof_signs_the_nonce_along_with_the_statement() {
        let io = FakeIo::signing("0xSignature");
        block_on(wallet_proof(&io, None, WALLET, |_| "statement".to_owned())).unwrap();
        assert_eq!(
            *io.signed.borrow(),
            vec![format!("statement\nNonce: {NONCE}")]
        );
    }

    /// A session with no identity token has no other proof to offer, so this
    /// cannot degrade into an unsigned request: it reaches the caller as a
    /// failure, and nothing was signed.
    #[test]
    fn wallet_proof_fails_rather_than_signing_without_a_nonce_when_the_endpoint_refuses() {
        let io = FakeIo::signing("0xSignature").without_a_nonce();
        let error =
            block_on(wallet_proof(&io, None, WALLET, |_| "statement".to_owned())).unwrap_err();
        assert_eq!(error, ProofError::Nonce(NonceUnavailable));
        assert!(io.signed.borrow().is_empty());
    }

    /// The identity-token branch signs nothing, so it must not be made to wait
    /// on a nonce it will never use.
    #[test]
    fn wallet_proof_asks_for_no_nonce_when_it_has_an_identity_token() {
        let io = FakeIo::signing("unused").without_a_nonce();
        block_on(wallet_proof(&io, Some("token-abc"), WALLET, |_| {
            "statement".to_owned()
        }))
        .unwrap();
        assert_eq!(*io.nonces_asked.borrow(), 0);
    }

    #[test]
    fn sign_claim_returns_the_issued_time_and_signature_on_success() {
        let io = FakeIo::signing("0xSignature");
        let claim = block_on(sign_claim(&io, "demo", "demo@example.com", WALLET)).unwrap();
        assert_eq!(claim.signature, "0xSignature");
        assert_eq!(claim.issued_at, 1_757_332_800);
        assert_eq!(claim.nonce, NONCE);
        let signed = io.signed.borrow();
        assert!(signed[0].starts_with("Postage: claim an address\n"));
        assert!(signed[0].ends_with(&format!("\nNonce: {NONCE}")));
    }

    /// Best effort covers this too: what comes back either proves the wallet
    /// or does not, and a claim signed under no nonce is refused by the route
    /// anyway.
    #[test]
    fn sign_claim_returns_nothing_rather_than_an_unusable_proof_when_no_nonce_can_be_fetched() {
        let io = FakeIo::signing("0xSignature").without_a_nonce();
        assert_eq!(
            block_on(sign_claim(&io, "demo", "demo@example.com", WALLET)),
            None
        );
        assert!(io.signed.borrow().is_empty());
    }

    #[test]
    fn sign_claim_is_best_effort_a_wallet_that_refuses_to_sign_returns_nothing_not_an_error() {
        let io = FakeIo::refusing_to_sign();
        assert_eq!(
            block_on(sign_claim(&io, "demo", "demo@example.com", WALLET)),
            None
        );
    }

    #[test]
    fn suggest_handle_returns_nothing_for_a_signed_in_session_with_no_email() {
        assert_eq!(suggest_handle(None), "");
    }

    #[test]
    fn suggest_handle_drops_the_domain_and_normalises_the_local_part() {
        assert_eq!(
            suggest_handle(Some(&format!("John.Doe@{MAIL_DOMAIN}"))),
            "john.doe"
        );
    }

    #[test]
    fn suggest_handle_strips_characters_the_handle_rules_do_not_allow() {
        assert_eq!(suggest_handle(Some("john+promo@example.com")), "johnpromo");
    }

    #[test]
    fn a_declined_prompt_reads_as_privys_message_without_the_code() {
        let error = ProofError::from(BridgeError::Sdk {
            code: "4001".to_owned(),
            message: "User rejected the request.".to_owned(),
        });
        assert_eq!(error.to_string(), "User rejected the request.");
    }
}

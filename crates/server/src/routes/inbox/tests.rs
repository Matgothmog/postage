//! The route driven through the router. No TypeScript test called this route
//! over HTTP; the closest were `wallet-proof.test.ts`, whose round trips built
//! requests to `/api/inbox` and handed them to the reader, and
//! `ClaimStrip.test.ts`, whose cases mirror `validate()`. Both are ported here
//! against the route itself, followed by a test per branch and the races.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use axum::Router;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use libsql::params;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use postage_core::secret::{MessageIdSecret, WalletNonceKey};
use postage_core::verification::code_matches;
use postage_core::wallet_nonce::mint_wallet_nonce;
use postage_core::wallet_proof::{NONCE_HEADER, WALLET_HEADER, signed_proof, with_nonce};
use serde::Deserialize;
use serde_json::{Value, json};

use super::*;
use crate::auth::confirm_statement;
use crate::cloudflare::Cloudflare;
use crate::config::Env;
use crate::db::claims::{InboxClaim, mark_code_verified, record_claim_send, start_claim};
use crate::db::inboxes::create_inbox;
use crate::db::testing::TestDb;
use crate::http_stub::{Reply, Stub, closed_port, serve_with};
use crate::mail::Mailer;
use crate::privy::{JwksError, JwksFetch, JwksResponse, PrivyVerifier};
use crate::routes::router;
use crate::routes::testing::{Answer, NOW, clock_at, post_with, send};

const PATH: &str = "/api/inbox";
const APP_ID: &str = "test-app";
const DESTINATION: &str = "me@example.com";
const CONFIRMED_AT: &str = "2026-01-01T00:00:00Z";

fn secret() -> String {
    "x".repeat(32)
}

fn holder() -> PrivateKeySigner {
    PrivateKeySigner::from_slice(&[0x11; 32]).unwrap()
}

fn impostor() -> PrivateKeySigner {
    PrivateKeySigner::from_slice(&[0x22; 32]).unwrap()
}

fn signer(n: u8) -> PrivateKeySigner {
    PrivateKeySigner::from_slice(&[n; 32]).unwrap()
}

fn address_of(signer: &PrivateKeySigner) -> String {
    signer.address().to_checksum(None)
}

fn wallet() -> String {
    address_of(&holder())
}

fn nonce_key() -> WalletNonceKey {
    WalletNonceKey::derive(&MessageIdSecret::new(secret()).unwrap()).unwrap()
}

fn verification_key() -> VerificationKey {
    VerificationKey::derive(&MessageIdSecret::new(secret()).unwrap()).unwrap()
}

fn mint(wallet: &str) -> String {
    mint_wallet_nonce(&nonce_key(), wallet, NOW, &mut OsRng.unwrap_err())
}

/// `personal_sign`, as a wallet does it.
fn sign_text(by: &PrivateKeySigner, text: &str) -> String {
    let signature = by.sign_message_sync(text.as_bytes()).unwrap();
    format!("0x{}", alloy_primitives::hex::encode(signature.as_bytes()))
}

// ---- Privy: a key minted in this process, served as the app's JWKS ----

static PRIVY_KEY: LazyLock<SigningKey> = LazyLock::new(|| {
    loop {
        let mut secret = [0_u8; 32];
        OsRng.try_fill_bytes(&mut secret).unwrap();
        if let Ok(key) = SigningKey::from_slice(&secret) {
            return key;
        }
    }
});

struct ServedJwks;

impl JwksFetch for ServedJwks {
    fn fetch(&self, _url: String) -> BoxFuture<'static, Result<JwksResponse, JwksError>> {
        let point = PRIVY_KEY.verifying_key().to_encoded_point(false);
        let keys = json!({ "keys": [{
            "kty": "EC",
            "crv": "P-256",
            "x": URL_SAFE_NO_PAD.encode(point.x().unwrap()),
            "y": URL_SAFE_NO_PAD.encode(point.y().unwrap()),
            "kid": "k",
        }] });
        let body = keys.to_string().into_bytes();
        async move { Ok(JwksResponse { status: 200, body }) }.boxed()
    }
}

/// An identity token Privy would issue for someone signed in with `email`
/// (if any) holding `wallets`.
fn identity_token(email: Option<&str>, wallets: &[&str]) -> String {
    let mut accounts: Vec<Value> = wallets
        .iter()
        .map(|wallet| json!({ "type": "wallet", "address": wallet }))
        .collect();
    if let Some(email) = email {
        accounts.push(json!({ "type": "email", "address": email }));
    }
    let encode = |value: Value| URL_SAFE_NO_PAD.encode(value.to_string());
    let head = encode(json!({ "alg": "ES256", "typ": "JWT", "kid": "k" }));
    let payload = encode(json!({
        "iss": "privy.io",
        "aud": APP_ID,
        "sub": "did:privy:someone",
        "exp": NOW + 600,
        "linked_accounts": Value::Array(accounts).to_string(),
    }));
    let signing_input = format!("{head}.{payload}");
    let signature: Signature = PRIVY_KEY.sign(signing_input.as_bytes());
    format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    )
}

// ---- The app under test ----

/// What Cloudflare says about the address it is asked to register.
#[derive(Debug, Clone, Copy)]
enum CloudflareSays {
    Confirmed,
    Unconfirmed,
    Unreachable,
}

struct Fixture {
    db: Arc<TestDb>,
    resend: Stub,
    cloudflare: Stub,
    cloudflare_base: String,
}

async fn fixture() -> Fixture {
    fixture_with(
        CloudflareSays::Unconfirmed,
        Reply::new(200, r#"{"id":"e1"}"#),
    )
    .await
}

async fn fixture_with(cloudflare: CloudflareSays, resend_reply: Reply) -> Fixture {
    let verified = match cloudflare {
        CloudflareSays::Confirmed => json!(CONFIRMED_AT),
        _ => Value::Null,
    };
    let stub = serve_with(move |sent| {
        let email = sent.body["email"]
            .as_str()
            .unwrap_or(DESTINATION)
            .to_owned();
        let address = json!({ "id": "addr_1", "email": email, "verified": verified });
        Reply::new(
            200,
            json!({ "success": true, "errors": [], "result": address }).to_string(),
        )
    })
    .await;
    let cloudflare_base = match cloudflare {
        CloudflareSays::Unreachable => closed_port(),
        _ => stub.base.clone(),
    };
    Fixture {
        db: Arc::new(TestDb::fresh().await),
        resend: serve_with(move |_| resend_reply.clone()).await,
        cloudflare: stub,
        cloudflare_base,
    }
}

impl Fixture {
    fn app(&self) -> Router {
        router(self.state(Env::fixed([
            ("MESSAGE_ID_SECRET", secret()),
            ("NEXT_PUBLIC_PRIVY_APP_ID", APP_ID.to_owned()),
        ])))
    }

    fn state(&self, env: Env) -> AppState {
        AppState::builder(env)
            .clock(clock_at(NOW))
            .db(self.db.clone())
            .mailer(
                Mailer::new(
                    reqwest::Client::default(),
                    "re_test_key".to_owned(),
                    "Postage <hello@usepostage.com>".to_owned(),
                )
                .with_endpoint(format!("{}/emails", self.resend.base)),
            )
            .cloudflare(
                Cloudflare::new(
                    reqwest::Client::default(),
                    "acct".to_owned(),
                    "t".to_owned(),
                )
                .with_api_base(&self.cloudflare_base),
            )
            .privy(PrivyVerifier::new(
                APP_ID.to_owned(),
                Arc::new(ServedJwks),
                clock_at(NOW),
            ))
            .build()
    }

    async fn claim(&self, body: &Value) -> Answer {
        self.claim_with(body, &[]).await
    }

    async fn claim_with(&self, body: &Value, headers: &[(&str, &str)]) -> Answer {
        send(self.app(), post_with(PATH, &body.to_string(), headers)).await
    }

    async fn read(&self, headers: &[(&str, &str)]) -> Answer {
        read_from(self.app(), headers).await
    }

    async fn stored(&self, handle: &str) -> Option<InboxClaim> {
        claim_by_handle(&self.db, handle).await.unwrap()
    }

    async fn sends(&self) -> i64 {
        #[derive(Deserialize)]
        struct Count {
            n: i64,
        }
        let rows: Vec<Count> = self
            .db
            .all("SELECT COUNT(*) AS n FROM claim_sends", ())
            .await
            .unwrap();
        rows[0].n
    }

    fn mailed(&self) -> Vec<Value> {
        self.resend
            .sent()
            .into_iter()
            .map(|sent| sent.body)
            .collect()
    }

    /// The code in the one message Resend was asked to send.
    fn mailed_code(&self) -> String {
        let mailed = self.mailed();
        assert_eq!(mailed.len(), 1, "exactly one code mailed");
        mailed[0]["subject"].as_str().unwrap()[..6].to_owned()
    }

    async fn own_inbox(&self, handle: &str, wallet: Option<&str>) {
        create_inbox(&self.db, handle, "owner@example.com", wallet, NOW)
            .await
            .unwrap();
    }

    async fn claimed_by(&self, handle: &str, wallet: &str, expires_at: i64) {
        start_claim(
            &self.db,
            &NewClaim {
                handle: handle.to_owned(),
                destination: "earlier@example.com".to_owned(),
                wallet: wallet.to_owned(),
                code_hash: "deadbeef".to_owned(),
                expires_at,
                cf_address_id: None,
                cf_verified_at: None,
            },
            NOW - 60,
        )
        .await
        .unwrap();
    }
}

async fn read_from(app: Router, headers: &[(&str, &str)]) -> Answer {
    let mut request = axum::http::Request::get(PATH);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    send(app, request.body(axum::body::Body::empty()).unwrap()).await
}

/// A claim body signed the way `signClaim` signs one.
fn signed_claim(by: &PrivateKeySigner, handle: &str, destination: &str) -> Value {
    let wallet = address_of(by);
    let nonce = mint(&wallet);
    let issued_at = NOW as f64;
    let statement = claim_statement(
        &handle.to_lowercase(),
        &destination.to_lowercase(),
        &wallet,
        issued_at,
    );
    json!({
        "handle": handle,
        "destination": destination,
        "wallet": wallet,
        "issuedAt": NOW,
        "signature": sign_text(by, &with_nonce(&statement, &nonce)),
        "nonce": nonce,
    })
}

/// The headers a session without an identity token reads its inbox with.
fn read_proof_headers(
    by: &PrivateKeySigner,
    statement_for: impl Fn(&str, f64) -> String,
) -> Vec<(&'static str, String)> {
    let wallet = address_of(by);
    let nonce = mint(&wallet);
    let issued_at = NOW as f64;
    let signature = sign_text(by, &with_nonce(&statement_for(&wallet, issued_at), &nonce));
    signed_proof(&wallet, issued_at, &signature, &nonce)
}

fn reading(by: &PrivateKeySigner) -> Vec<(&'static str, String)> {
    read_proof_headers(by, read_statement)
}

fn as_headers<'a>(owned: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    owned
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect()
}

/// A checksummed address with one letter's case flipped, which viem's
/// `isAddress` refuses: mixed case has to be the checksum.
fn with_one_letter_recased(address: &str) -> String {
    let at = address[2..]
        .find(|c: char| c.is_ascii_alphabetic())
        .map(|index| index + 2)
        .unwrap();
    let flipped = address[at..=at].chars().map(|c| {
        if c.is_ascii_uppercase() {
            c.to_ascii_lowercase()
        } else {
            c.to_ascii_uppercase()
        }
    });
    format!(
        "{}{}{}",
        &address[..at],
        flipped.collect::<String>(),
        &address[at + 1..]
    )
}

fn inbox_json(wallet: &str) -> String {
    format!(
        r#"{{"inbox":{{"handle":"demo","destination":"owner@example.com","wallet":"{}","created_at":{NOW}}}}}"#,
        wallet.to_lowercase()
    )
}

// ---- GET ----

#[tokio::test]
async fn a_signed_in_user_reads_the_inbox_their_wallet_owns() {
    let fixture = fixture().await;
    fixture.own_inbox("demo", Some(&wallet())).await;
    let token = identity_token(None, &[&wallet()]);

    let answer = fixture.read(&[(IDENTITY_TOKEN_HEADER, &token)]).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.text, inbox_json(&wallet()));
}

#[tokio::test]
async fn a_signed_in_user_without_an_inbox_reads_an_explicit_null() {
    let fixture = fixture().await;
    let token = identity_token(None, &[&wallet()]);

    let answer = fixture.read(&[(IDENTITY_TOKEN_HEADER, &token)]).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.text, r#"{"inbox":null}"#);
}

#[tokio::test]
async fn every_wallet_the_session_holds_is_looked_at_in_turn() {
    let fixture = fixture().await;
    fixture
        .own_inbox("demo", Some(&address_of(&impostor())))
        .await;
    let token = identity_token(None, &[&wallet(), &address_of(&impostor())]);

    let answer = fixture.read(&[(IDENTITY_TOKEN_HEADER, &token)]).await;

    assert_eq!(answer.text, inbox_json(&address_of(&impostor())));
}

#[tokio::test]
async fn a_token_that_does_not_verify_falls_back_to_the_signed_proof() {
    let fixture = fixture().await;

    let answer = fixture
        .read(&[(IDENTITY_TOKEN_HEADER, "not.a.token")])
        .await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        answer.body,
        json!({ "error": "A valid wallet is required" })
    );
}

#[tokio::test]
async fn a_read_naming_no_valid_wallet_is_refused() {
    let fixture = fixture().await;
    let wrong_checksum = with_one_letter_recased(&wallet());

    for named in ["", "0xnot-an-address", wrong_checksum.as_str()] {
        let answer = fixture.read(&[(WALLET_HEADER, named)]).await;

        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{named}");
        assert_eq!(
            answer.body,
            json!({ "error": "A valid wallet is required" })
        );
    }
}

/// The wallet is public, so naming it is not holding it.
#[tokio::test]
async fn naming_a_wallet_without_proving_it_reads_nothing() {
    let fixture = fixture().await;
    fixture.own_inbox("demo", Some(&wallet())).await;

    let answer = fixture.read(&[(WALLET_HEADER, &wallet())]).await;

    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        answer.body,
        json!({ "error": "Sign in again to read this inbox" })
    );
}

/// `wallet-proof.test.ts`: a proof the browser writes is accepted by the
/// reader on the other side.
#[tokio::test]
async fn a_proof_the_browser_writes_reads_the_wallets_inbox() {
    let fixture = fixture().await;
    fixture.own_inbox("demo", Some(&wallet())).await;
    let proof = reading(&holder());

    let answer = fixture.read(&as_headers(&proof)).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.text, inbox_json(&wallet()));
}

#[tokio::test]
async fn a_proven_wallet_without_an_inbox_reads_null() {
    let fixture = fixture().await;

    let answer = fixture.read(&as_headers(&reading(&holder()))).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.text, r#"{"inbox":null}"#);
}

/// `wallet-proof.test.ts`: accepted once and refused on its second use.
#[tokio::test]
async fn a_read_proof_is_accepted_once_and_refused_when_replayed() {
    let fixture = fixture().await;
    let proof = reading(&holder());

    let first = fixture.read(&as_headers(&proof)).await;
    let replayed = fixture.read(&as_headers(&proof)).await;

    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(replayed.status, StatusCode::UNAUTHORIZED);
}

/// `wallet-proof.test.ts`: a wallet can prove itself again with a proof of
/// its own.
#[tokio::test]
async fn a_fresh_proof_still_reads_after_an_earlier_one_was_spent() {
    let fixture = fixture().await;
    fixture.read(&as_headers(&reading(&holder()))).await;

    let again = fixture.read(&as_headers(&reading(&holder()))).await;

    assert_eq!(again.status, StatusCode::OK);
}

/// `wallet-proof.test.ts`: a proof written for one statement is refused by
/// a reader checking another.
#[tokio::test]
async fn a_proof_signed_for_another_route_does_not_read_the_inbox() {
    let fixture = fixture().await;
    let proof = read_proof_headers(&holder(), |wallet, at| {
        confirm_statement("demo", wallet, at)
    });

    let answer = fixture.read(&as_headers(&proof)).await;

    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
}

/// `wallet-proof.test.ts`: a proof naming one wallet but signed by another.
#[tokio::test]
async fn a_proof_signed_by_another_key_does_not_read_the_inbox() {
    let fixture = fixture().await;
    let wallet = wallet();
    let nonce = mint(&wallet);
    let forged = sign_text(
        &impostor(),
        &with_nonce(&read_statement(&wallet, NOW as f64), &nonce),
    );
    let proof = signed_proof(&wallet, NOW as f64, &forged, &nonce);

    let answer = fixture.read(&as_headers(&proof)).await;

    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
}

/// `wallet-proof.test.ts`: a proof arriving without its nonce header.
#[tokio::test]
async fn a_proof_without_its_nonce_does_not_read_the_inbox() {
    let fixture = fixture().await;
    let proof: Vec<_> = reading(&holder())
        .into_iter()
        .filter(|(name, _)| *name != NONCE_HEADER)
        .collect();

    let answer = fixture.read(&as_headers(&proof)).await;

    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
}

/// As in the TypeScript, whose nonce ledger caught the failure and read it
/// as an unproven wallet; the outage is logged, not answered.
#[tokio::test]
async fn an_unreachable_database_refuses_a_signed_read_as_unproven() {
    let app = router(
        AppState::builder(Env::fixed([
            ("MESSAGE_ID_SECRET", secret()),
            ("DATABASE_URL", "nonsense://x".to_owned()),
        ]))
        .clock(clock_at(NOW))
        .build(),
    );
    let proof = reading(&holder());

    let (answer, lines) = crate::log::captured::during(read_from(app, &as_headers(&proof))).await;

    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert!(lines[0].starts_with("spent wallet nonce ledger unavailable"));
}

#[tokio::test]
async fn an_unreachable_database_fails_a_signed_in_read() {
    let app = router(
        AppState::builder(Env::fixed([
            ("NEXT_PUBLIC_PRIVY_APP_ID", APP_ID.to_owned()),
            ("DATABASE_URL", "nonsense://x".to_owned()),
        ]))
        .clock(clock_at(NOW))
        .privy(PrivyVerifier::new(
            APP_ID.to_owned(),
            Arc::new(ServedJwks),
            clock_at(NOW),
        ))
        .build(),
    );
    let token = identity_token(None, &[&wallet()]);

    let answer = read_from(app, &[(IDENTITY_TOKEN_HEADER, &token)]).await;

    assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(answer.text, "");
}

// ---- POST: the long way, a signature and a mailed code ----

#[tokio::test]
async fn a_signed_claim_mails_a_code_and_waits_on_it() {
    let fixture = fixture().await;

    let answer = fixture
        .claim(&signed_claim(&holder(), "demo", DESTINATION))
        .await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(
        answer.text,
        r#"{"status":"pending","handle":"demo","destination":"me@example.com","codeVerified":false,"cloudflareVerified":false,"live":false,"expiresIn":900}"#
    );
    let code = fixture.mailed_code();
    assert_eq!(fixture.mailed()[0]["to"], DESTINATION);
    let claim = fixture.stored("demo").await.unwrap();
    assert!(code_matches(
        &verification_key(),
        "demo",
        &code,
        &claim.code_hash
    ));
    assert_eq!(claim.wallet, wallet().to_lowercase());
    assert_eq!(claim.expires_at, NOW + 900);
    assert_eq!(claim.code_verified_at, None);
    assert_eq!(fixture.sends().await, 1);
    assert!(
        fixture.cloudflare.sent().is_empty(),
        "Cloudflare is not told until the code comes back"
    );
}

#[tokio::test]
async fn the_handle_and_destination_are_claimed_lower_cased() {
    let fixture = fixture().await;

    let answer = fixture
        .claim(&signed_claim(&holder(), "Demo", "Me@Example.COM"))
        .await;

    assert_eq!(answer.body["handle"], "demo");
    assert_eq!(answer.body["destination"], DESTINATION);
    assert_eq!(fixture.mailed()[0]["to"], DESTINATION);
    assert_eq!(
        fixture.stored("demo").await.unwrap().destination,
        DESTINATION
    );
}

/// `ClaimStrip.test.ts`: a handle and destination the server would accept.
#[tokio::test]
async fn a_handle_with_inner_dots_and_dashes_is_accepted() {
    let fixture = fixture().await;

    let answer = fixture
        .claim(&signed_claim(&holder(), "demo.user-1", "user@example.com"))
        .await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
}

/// The only characters that lower-case into the handle alphabet from outside
/// it: the Kelvin sign becomes `k`. The handle is claimed as what it became,
/// and nothing non-ASCII survives into it.
#[tokio::test]
async fn a_handle_is_judged_and_claimed_as_it_lower_cases() {
    let fixture = fixture().await;

    let answer = fixture
        .claim(&signed_claim(&holder(), "\u{212A}elvin", DESTINATION))
        .await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(answer.body["handle"], "kelvin");
    assert!(fixture.stored("kelvin").await.is_some());
}

#[tokio::test]
async fn a_claim_without_a_signature_is_refused_before_anything_is_sent() {
    let fixture = fixture().await;
    let mut body = signed_claim(&holder(), "demo", DESTINATION);
    body["signature"] = Value::Null;

    let answer = fixture.claim(&body).await;

    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        answer.body,
        json!({ "error": "Sign the request with the wallet you are claiming for" })
    );
    assert!(fixture.mailed().is_empty());
    assert_eq!(fixture.sends().await, 0);
    assert_eq!(fixture.stored("demo").await, None);
}

/// The signature binds the handle and the destination, so it cannot be
/// lifted onto a claim for something else.
#[tokio::test]
async fn a_signature_over_another_handle_or_address_proves_nothing() {
    let fixture = fixture().await;

    let mut other_handle = signed_claim(&holder(), "demo", DESTINATION);
    other_handle["handle"] = json!("other");
    let mut other_address = signed_claim(&holder(), "demo", DESTINATION);
    other_address["destination"] = json!("victim@example.com");

    assert_eq!(
        fixture.claim(&other_handle).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture.claim(&other_address).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert!(fixture.mailed().is_empty());
}

#[tokio::test]
async fn a_signature_from_another_key_proves_nothing() {
    let fixture = fixture().await;
    let mut body = signed_claim(&impostor(), "demo", DESTINATION);
    body["wallet"] = json!(wallet());

    let answer = fixture.claim(&body).await;

    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_claim_proof_is_good_for_one_request() {
    let fixture = fixture().await;
    let body = signed_claim(&holder(), "demo", DESTINATION);

    let first = fixture.claim(&body).await;
    let replayed = fixture.claim(&body).await;

    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(replayed.status, StatusCode::UNAUTHORIZED);
    assert_eq!(fixture.mailed().len(), 1);
}

#[tokio::test]
async fn a_stale_signature_proves_nothing() {
    let fixture = fixture().await;
    let mut body = signed_claim(&holder(), "demo", DESTINATION);
    body["issuedAt"] = json!(NOW - 301);

    assert_eq!(fixture.claim(&body).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_claim_naming_no_valid_wallet_is_refused_first() {
    let fixture = fixture().await;

    for body in [
        json!({ "handle": "demo", "destination": DESTINATION }),
        json!({ "handle": "demo", "wallet": "" }),
        json!({ "handle": "demo", "wallet": "0xnot-an-address" }),
        json!({ "handle": "demo", "wallet": 12 }),
        json!({ "handle": "demo", "wallet": [wallet().to_lowercase()] }),
        json!({ "handle": 5, "wallet": null }),
    ] {
        let answer = fixture.claim(&body).await;

        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(
            answer.body,
            json!({ "error": "A valid wallet is required" }),
            "{body}"
        );
    }
}

/// Each of these was a bare 500 in the TypeScript (a body that is not JSON,
/// a JSON null, a handle or destination that is not a string, a nonce that
/// is not one) or was coerced past the cast. All are refused with a 400
/// before anything is counted or sent.
#[tokio::test]
async fn a_malformed_claim_is_refused_before_anything_is_spent() {
    let fixture = fixture().await;
    let wallet = wallet();

    for (raw, error) in [
        ("not json".to_owned(), "Invalid request"),
        (String::new(), "Invalid request"),
        ("null".to_owned(), "A valid wallet is required"),
        ("[]".to_owned(), "A valid wallet is required"),
        (
            json!({ "wallet": wallet, "handle": 5 }).to_string(),
            "Invalid request",
        ),
        (
            json!({ "wallet": wallet, "handle": ["demo"] }).to_string(),
            "Invalid request",
        ),
        (
            json!({ "wallet": wallet, "handle": "demo", "destination": { "a": 1 } }).to_string(),
            "Invalid request",
        ),
        (
            json!({ "wallet": wallet, "handle": "demo", "destination": false }).to_string(),
            "Invalid request",
        ),
        (
            json!({ "wallet": wallet, "handle": "demo", "issuedAt": "1788868800" }).to_string(),
            "Invalid request",
        ),
        (
            json!({ "wallet": wallet, "handle": "demo", "signature": 1 }).to_string(),
            "Invalid request",
        ),
        (
            json!({ "wallet": wallet, "handle": "demo", "nonce": true }).to_string(),
            "Invalid request",
        ),
    ] {
        let answer = send(fixture.app(), post_with(PATH, &raw, &[])).await;

        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{raw}");
        assert_eq!(answer.body, json!({ "error": error }), "{raw}");
    }
    assert_eq!(fixture.sends().await, 0);
    assert!(fixture.mailed().is_empty());
}

/// `ClaimStrip.test.ts` mirrors these branches client-side; here they are
/// against the route. Each is refused before the signature is looked at, so
/// none of these bodies needs to be signed.
#[tokio::test]
async fn each_rule_a_claim_breaks_has_its_own_refusal() {
    const SHAPE: &str =
        "Pick 2-31 characters: letters, digits, dot, dash, not starting or ending with punctuation";
    const DOTS: &str = "Two dots in a row is not a valid address";
    const RESERVED_NAME: &str = "That name is reserved";
    const NO_DESTINATION: &str = "A valid destination address is required";
    const OURS: &str = "Forward to an inbox you already read, not back to Postage";
    let fixture = fixture().await;
    let longest = "a".repeat(31);
    let too_long = "a".repeat(32);

    for (handle, destination, error) in [
        (json!("a"), json!(DESTINATION), SHAPE),
        (json!(too_long), json!(DESTINATION), SHAPE),
        (json!(""), json!(DESTINATION), SHAPE),
        (Value::Null, json!(DESTINATION), SHAPE),
        (json!("-demo"), json!(DESTINATION), SHAPE),
        (json!("demo-"), json!(DESTINATION), SHAPE),
        (json!("de mo"), json!(DESTINATION), SHAPE),
        (json!("démo"), json!(DESTINATION), SHAPE),
        (json!("demo..user"), json!(DESTINATION), DOTS),
        (json!("hello"), json!(DESTINATION), RESERVED_NAME),
        (json!("Hello"), json!(DESTINATION), RESERVED_NAME),
        (json!("POSTMASTER"), json!(DESTINATION), RESERVED_NAME),
        (json!("no-reply"), json!(DESTINATION), RESERVED_NAME),
        (json!(longest), Value::Null, NO_DESTINATION),
        (json!("demo"), json!(""), NO_DESTINATION),
        (json!("demo"), json!("not-an-email"), NO_DESTINATION),
        (json!("demo"), json!("a b@example.com"), NO_DESTINATION),
        (
            json!("demo"),
            json!("a\u{feff}@example.com"),
            NO_DESTINATION,
        ),
        (json!("demo"), json!("a@b@example.com"), NO_DESTINATION),
        (json!("demo"), json!("a@example"), NO_DESTINATION),
        (json!("demo"), json!("a@.com"), NO_DESTINATION),
        (json!("demo"), json!("a@example."), NO_DESTINATION),
        (json!("demo"), json!("someone@usepostage.com"), OURS),
        (json!("demo"), json!("Someone@UsePostage.COM"), OURS),
        (json!("demo"), json!("someone@usepostage.com."), OURS),
        (json!("demo"), json!("someone@usepostage.com.."), OURS),
    ] {
        let body = json!({ "handle": handle, "destination": destination, "wallet": wallet() });

        let answer = fixture.claim(&body).await;

        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(answer.body, json!({ "error": error }), "{body}");
    }
    assert_eq!(fixture.sends().await, 0);
}

#[tokio::test]
async fn a_handle_another_wallets_inbox_holds_is_taken() {
    let fixture = fixture().await;
    fixture
        .own_inbox("demo", Some(&address_of(&impostor())))
        .await;

    let answer = fixture
        .claim(&signed_claim(&holder(), "demo", DESTINATION))
        .await;

    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(answer.body, json!({ "error": "That handle is taken" }));
    assert!(fixture.mailed().is_empty());
    assert_eq!(fixture.sends().await, 0);
}

#[tokio::test]
async fn an_inbox_that_names_no_wallet_is_taken() {
    let fixture = fixture().await;
    fixture.own_inbox("demo", None).await;

    let answer = fixture
        .claim(&signed_claim(&holder(), "demo", DESTINATION))
        .await;

    assert_eq!(answer.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn an_owner_may_point_their_own_inbox_somewhere_new() {
    let fixture = fixture().await;
    fixture.own_inbox("demo", Some(&wallet())).await;

    let answer = fixture
        .claim(&signed_claim(&holder(), "demo", DESTINATION))
        .await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
}

#[tokio::test]
async fn another_wallets_answered_claim_is_taken_even_once_expired() {
    let fixture = fixture().await;
    fixture
        .claimed_by("demo", &address_of(&impostor()), NOW - 1)
        .await;
    mark_code_verified(&fixture.db, "demo", NOW - 30)
        .await
        .unwrap();

    let answer = fixture
        .claim(&signed_claim(&holder(), "demo", DESTINATION))
        .await;

    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(answer.body, json!({ "error": "That handle is taken" }));
}

#[tokio::test]
async fn another_wallets_claim_in_progress_holds_the_handle() {
    let fixture = fixture().await;
    fixture
        .claimed_by("demo", &address_of(&impostor()), NOW + 1)
        .await;

    let answer = fixture
        .claim(&signed_claim(&holder(), "demo", DESTINATION))
        .await;

    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(
        answer.body,
        json!({ "error": "Someone is claiming that handle right now. Try again in a few minutes" })
    );
}

#[tokio::test]
async fn another_wallets_abandoned_claim_releases_the_handle() {
    let fixture = fixture().await;
    fixture
        .claimed_by("demo", &address_of(&impostor()), NOW)
        .await;

    let answer = fixture
        .claim(&signed_claim(&holder(), "demo", DESTINATION))
        .await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(
        fixture.stored("demo").await.unwrap().wallet,
        wallet().to_lowercase()
    );
}

#[tokio::test]
async fn a_wallet_starting_its_own_claim_again_gets_a_new_code() {
    let fixture = fixture().await;
    fixture.claimed_by("demo", &wallet(), NOW + 600).await;
    fixture
        .db
        .run("UPDATE inbox_claims SET attempts = 4", ())
        .await
        .unwrap();

    let answer = fixture
        .claim(&signed_claim(&holder(), "demo", DESTINATION))
        .await;

    assert_eq!(answer.status, StatusCode::OK);
    let claim = fixture.stored("demo").await.unwrap();
    assert_ne!(claim.code_hash, "deadbeef");
    assert_eq!(claim.attempts, 0);
    assert_eq!(claim.destination, DESTINATION);
}

async fn sent_before(fixture: &Fixture, destination: &str, wallet: &str, times: usize, at: i64) {
    for _ in 0..times {
        record_claim_send(&fixture.db, destination, wallet, at)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn an_address_asked_three_times_this_hour_is_not_asked_again() {
    let fixture = fixture().await;
    sent_before(&fixture, DESTINATION, &address_of(&impostor()), 3, NOW - 10).await;

    let answer = fixture
        .claim(&signed_claim(&holder(), "demo", DESTINATION))
        .await;

    assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        answer.body,
        json!({ "error": "That address has been asked to confirm too many times. Try again later" })
    );
    assert!(fixture.mailed().is_empty());
}

#[tokio::test]
async fn a_wallet_that_claimed_five_times_this_hour_is_stopped() {
    let fixture = fixture().await;
    for n in 0..5 {
        sent_before(
            &fixture,
            &format!("a{n}@example.com"),
            &wallet(),
            1,
            NOW - 10,
        )
        .await;
    }

    let answer = fixture
        .claim(&signed_claim(&holder(), "demo", DESTINATION))
        .await;

    assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        answer.body,
        json!({ "error": "That wallet has claimed too many addresses this hour. Try again later" })
    );
}

#[tokio::test]
async fn sends_an_hour_old_are_dropped_and_count_for_nothing() {
    let fixture = fixture().await;
    sent_before(
        &fixture,
        DESTINATION,
        &wallet(),
        5,
        NOW - THROTTLE_WINDOW_SECONDS,
    )
    .await;

    let answer = fixture
        .claim(&signed_claim(&holder(), "demo", DESTINATION))
        .await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(fixture.sends().await, 1, "the old rows were purged");
}

/// Counted before anything is sent: a send that fails has still been tried.
#[tokio::test]
async fn resend_refusing_the_code_is_a_bad_gateway_and_starts_no_claim() {
    let fixture = fixture_with(
        CloudflareSays::Unconfirmed,
        Reply::new(422, r#"{"message":"bad address"}"#),
    )
    .await;

    let answer = fixture
        .claim(&signed_claim(&holder(), "demo", DESTINATION))
        .await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        answer.body,
        json!({ "error": r#"Could not send the verification code: {"message":"bad address"}"# })
    );
    assert_eq!(fixture.stored("demo").await, None);
    assert_eq!(fixture.sends().await, 1);
}

#[tokio::test]
async fn a_deployment_without_resend_settings_says_which_one_is_missing() {
    let fixture = fixture().await;
    let app = router(
        AppState::builder(Env::fixed([("MESSAGE_ID_SECRET", secret())]))
            .clock(clock_at(NOW))
            .db(fixture.db.clone())
            .build(),
    );
    let body = signed_claim(&holder(), "demo", DESTINATION).to_string();

    let answer = send(app, post_with(PATH, &body, &[])).await;

    assert_eq!(answer.status, StatusCode::BAD_GATEWAY);
    assert_eq!(answer.body, json!({ "error": "RESEND_API_KEY is not set" }));
}

// ---- POST: the short way, a Privy session claiming its own address ----

fn signed_in_as(email: Option<&str>) -> String {
    identity_token(email, &[&wallet().to_lowercase()])
}

#[tokio::test]
async fn a_signed_in_user_claiming_their_own_address_goes_live_without_a_code() {
    let fixture = fixture_with(CloudflareSays::Confirmed, Reply::new(200, "{}")).await;
    let token = signed_in_as(Some("Me@Example.com"));

    let answer = fixture
        .claim_with(
            &json!({ "handle": "demo", "wallet": wallet() }),
            &[(IDENTITY_TOKEN_HEADER, &token)],
        )
        .await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(
        answer.text,
        r#"{"status":"live","handle":"demo","destination":"me@example.com","codeVerified":true,"cloudflareVerified":true,"live":true}"#
    );
    assert!(fixture.mailed().is_empty(), "no code is mailed");
    let inbox = inbox_by_handle(&fixture.db, "demo").await.unwrap().unwrap();
    assert_eq!(inbox.destination, DESTINATION);
    assert_eq!(inbox.wallet, Some(wallet().to_lowercase()));
    assert_eq!(fixture.stored("demo").await, None);
    assert_eq!(fixture.sends().await, 1);
}

#[tokio::test]
async fn a_signed_in_claim_waits_on_cloudflare_with_its_code_half_done() {
    let fixture = fixture().await;
    let token = signed_in_as(Some(DESTINATION));

    let answer = fixture
        .claim_with(
            &json!({ "handle": "demo", "wallet": wallet() }),
            &[(IDENTITY_TOKEN_HEADER, &token)],
        )
        .await;

    assert_eq!(
        answer.text,
        r#"{"status":"pending","handle":"demo","destination":"me@example.com","codeVerified":true,"cloudflareVerified":false,"live":false}"#
    );
    let claim = fixture.stored("demo").await.unwrap();
    assert_eq!(claim.code_verified_at, Some(NOW));
    assert_eq!(claim.cf_address_id.as_deref(), Some("addr_1"));
    assert_eq!(
        fixture.cloudflare.sent()[0].body,
        json!({ "email": DESTINATION })
    );
}

/// The claim is recorded either way; the poller asks Cloudflare again.
#[tokio::test]
async fn an_unreachable_cloudflare_leaves_the_signed_in_claim_pending() {
    let fixture = fixture_with(CloudflareSays::Unreachable, Reply::new(200, "{}")).await;
    let token = signed_in_as(Some(DESTINATION));

    let answer = fixture
        .claim_with(
            &json!({ "handle": "demo", "wallet": wallet() }),
            &[(IDENTITY_TOKEN_HEADER, &token)],
        )
        .await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.body["codeVerified"], true);
    assert_eq!(answer.body["live"], false);
    let claim = fixture.stored("demo").await.unwrap();
    assert_eq!(claim.code_verified_at, Some(NOW));
    assert_eq!(claim.cf_address_id, None);
}

/// The code a signed-in claim is stored with was never sent, so the confirm
/// route cannot be used to answer it.
#[tokio::test]
async fn a_signed_in_claims_code_hash_matches_no_code_anyone_was_sent() {
    let fixture = fixture().await;
    let token = signed_in_as(Some(DESTINATION));
    fixture
        .claim_with(
            &json!({ "handle": "demo", "wallet": wallet() }),
            &[(IDENTITY_TOKEN_HEADER, &token)],
        )
        .await;

    let claim = fixture.stored("demo").await.unwrap();
    assert_eq!(claim.code_hash.len(), 64);
    assert!(!code_matches(
        &verification_key(),
        "demo",
        "",
        &claim.code_hash
    ));
}

#[tokio::test]
async fn naming_the_sessions_own_address_in_another_case_is_still_the_short_way() {
    let fixture = fixture().await;
    let token = signed_in_as(Some(DESTINATION));

    let answer = fixture
        .claim_with(
            &json!({ "handle": "demo", "wallet": wallet(), "destination": "ME@example.com" }),
            &[(IDENTITY_TOKEN_HEADER, &token)],
        )
        .await;

    assert_eq!(answer.body["codeVerified"], true);
    assert!(fixture.mailed().is_empty());
}

/// Signed in proves the wallet, not an address Privy never confirmed.
#[tokio::test]
async fn a_signed_in_user_forwarding_elsewhere_is_mailed_a_code_without_signing() {
    let fixture = fixture().await;
    let token = signed_in_as(Some(DESTINATION));

    let answer = fixture
        .claim_with(
            &json!({ "handle": "demo", "wallet": wallet(), "destination": "other@example.com" }),
            &[(IDENTITY_TOKEN_HEADER, &token)],
        )
        .await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(answer.body["codeVerified"], false);
    assert_eq!(answer.body["expiresIn"], 900);
    assert_eq!(fixture.mailed()[0]["to"], "other@example.com");
}

#[tokio::test]
async fn a_session_without_an_address_has_to_name_one() {
    let fixture = fixture().await;
    let token = signed_in_as(None);

    let answer = fixture
        .claim_with(
            &json!({ "handle": "demo", "wallet": wallet() }),
            &[(IDENTITY_TOKEN_HEADER, &token)],
        )
        .await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        answer.body,
        json!({ "error": "A valid destination address is required" })
    );
}

/// A session is a session for the wallets it lists. Naming another wallet
/// falls back to the signed proof, which this request does not carry.
#[tokio::test]
async fn a_session_for_another_wallet_proves_nothing_about_this_one() {
    let fixture = fixture().await;
    let token = identity_token(Some(DESTINATION), &[&address_of(&impostor())]);

    let answer = fixture
        .claim_with(
            &json!({ "handle": "demo", "wallet": wallet() }),
            &[(IDENTITY_TOKEN_HEADER, &token)],
        )
        .await;

    assert_eq!(
        answer.status,
        StatusCode::BAD_REQUEST,
        "no session email, so no address"
    );
    let answer = fixture
        .claim_with(
            &json!({ "handle": "demo", "wallet": wallet(), "destination": DESTINATION }),
            &[(IDENTITY_TOKEN_HEADER, &token)],
        )
        .await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_signed_in_claim_on_a_taken_handle_is_refused() {
    let fixture = fixture().await;
    fixture
        .own_inbox("demo", Some(&address_of(&impostor())))
        .await;
    let token = signed_in_as(Some(DESTINATION));

    let answer = fixture
        .claim_with(
            &json!({ "handle": "demo", "wallet": wallet() }),
            &[(IDENTITY_TOKEN_HEADER, &token)],
        )
        .await;

    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(
        inbox_by_handle(&fixture.db, "demo")
            .await
            .unwrap()
            .unwrap()
            .wallet,
        Some(address_of(&impostor()).to_lowercase())
    );
}

// ---- Races ----

/// Both claims are past the availability check before either is written:
/// Resend sits on each send long enough for the other to catch up. In the
/// TypeScript both were answered 200 and the later write took the handle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_wallets_racing_for_one_handle_cannot_both_hold_it() {
    let fixture = Arc::new(
        fixture_with(
            CloudflareSays::Unconfirmed,
            Reply::new(200, "{}").after(Duration::from_millis(300)),
        )
        .await,
    );
    let racers: Vec<_> = [holder(), impostor()]
        .into_iter()
        .map(|by| {
            let fixture = fixture.clone();
            tokio::spawn(async move {
                let wallet = address_of(&by).to_lowercase();
                let answer = fixture
                    .claim(&signed_claim(
                        &by,
                        "demo",
                        &format!("{}@example.com", &wallet[2..8]),
                    ))
                    .await;
                (wallet, answer.status)
            })
        })
        .collect();
    let mut winners = Vec::new();
    for racer in racers {
        let (wallet, status) = racer.await.unwrap();
        match status {
            StatusCode::OK => winners.push(wallet),
            status => assert_eq!(status, StatusCode::CONFLICT),
        }
    }

    assert_eq!(winners.len(), 1, "exactly one claim stands");
    assert_eq!(fixture.stored("demo").await.unwrap().wallet, winners[0]);
}

/// A signed-in claim for one wallet and a mailed claim for another, racing.
/// Whichever stands, a claim is never verified for an address nobody proved:
/// in the TypeScript the short way's `markCodeVerified` and
/// `attachDestination` went by handle alone and could land on the other
/// wallet's claim.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signed_in_claim_racing_a_mailed_one_never_verifies_the_unproven_address() {
    for _ in 0..8 {
        let fixture = Arc::new(
            fixture_with(
                CloudflareSays::Confirmed,
                Reply::new(200, "{}").after(Duration::from_millis(50)),
            )
            .await,
        );
        let short = {
            let fixture = fixture.clone();
            tokio::spawn(async move {
                let token = signed_in_as(Some(DESTINATION));
                fixture
                    .claim_with(
                        &json!({ "handle": "demo", "wallet": wallet() }),
                        &[(IDENTITY_TOKEN_HEADER, &token)],
                    )
                    .await
                    .status
            })
        };
        let long = {
            let fixture = fixture.clone();
            tokio::spawn(async move {
                fixture
                    .claim(&signed_claim(&impostor(), "demo", "victim@example.com"))
                    .await
                    .status
            })
        };
        let statuses = [short.await.unwrap(), long.await.unwrap()];

        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::OK)
                .count(),
            1,
            "{statuses:?}"
        );
        if let Some(inbox) = inbox_by_handle(&fixture.db, "demo").await.unwrap() {
            assert_eq!(inbox.destination, DESTINATION);
        }
        if let Some(claim) = fixture.stored("demo").await
            && claim.destination == "victim@example.com"
        {
            assert_eq!(claim.code_verified_at, None);
            assert_eq!(claim.cf_address_id, None);
        }
    }
}

/// A claim going live while another wallet asks for the handle: the inbox
/// stays with the wallet that proved both halves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_claim_racing_a_promotion_cannot_take_the_new_inbox() {
    for _ in 0..8 {
        let fixture =
            Arc::new(fixture_with(CloudflareSays::Confirmed, Reply::new(200, "{}")).await);
        fixture.claimed_by("demo", &wallet(), NOW + 600).await;
        mark_code_verified(&fixture.db, "demo", NOW).await.unwrap();
        fixture
            .db
            .run(
                "UPDATE inbox_claims SET cf_address_id = 'addr_1', cf_checked_at = ?",
                params![NOW - 60],
            )
            .await
            .unwrap();

        let promote = {
            let fixture = fixture.clone();
            tokio::spawn(async move {
                let cloudflare = Cloudflare::new(
                    reqwest::Client::default(),
                    "acct".to_owned(),
                    "t".to_owned(),
                )
                .with_api_base(&fixture.cloudflare_base);
                settle_claim(&fixture.db, &cloudflare, "demo", NOW)
                    .await
                    .unwrap()
            })
        };
        let rival = {
            let fixture = fixture.clone();
            tokio::spawn(async move {
                fixture
                    .claim(&signed_claim(&impostor(), "demo", "victim@example.com"))
                    .await
                    .status
            })
        };

        assert!(promote.await.unwrap().unwrap().live);
        assert_eq!(rival.await.unwrap(), StatusCode::CONFLICT);
        let inbox = inbox_by_handle(&fixture.db, "demo").await.unwrap().unwrap();
        assert_eq!(inbox.wallet, Some(wallet().to_lowercase()));
        assert_eq!(fixture.stored("demo").await, None);
    }
}

/// Every request in a burst used to read the same counts and pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_of_claims_from_one_wallet_mails_no_more_than_five_addresses() {
    let fixture = Arc::new(
        fixture_with(
            CloudflareSays::Unconfirmed,
            Reply::new(200, "{}").after(Duration::from_millis(100)),
        )
        .await,
    );
    let claims: Vec<_> = (0..40)
        .map(|n| {
            let fixture = fixture.clone();
            tokio::spawn(async move {
                fixture
                    .claim(&signed_claim(
                        &holder(),
                        &format!("demo{n}"),
                        &format!("to{n}@example.com"),
                    ))
                    .await
                    .status
            })
        })
        .collect();
    let mut answered = Vec::new();
    for claim in claims {
        answered.push(claim.await.unwrap());
    }

    let accepted = answered
        .iter()
        .filter(|status| **status == StatusCode::OK)
        .count();
    assert_eq!(accepted, 5, "{answered:?}");
    assert!(
        answered
            .iter()
            .all(|status| [StatusCode::OK, StatusCode::TOO_MANY_REQUESTS].contains(status))
    );
    assert_eq!(fixture.mailed().len(), 5);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_of_claims_on_one_address_mails_it_no_more_than_three_times() {
    let fixture = Arc::new(fixture().await);
    let claims: Vec<_> = (0..24_u8)
        .map(|n| {
            let fixture = fixture.clone();
            tokio::spawn(async move {
                let by = signer(0x30 + n);
                fixture
                    .claim(&signed_claim(&by, &format!("demo{n}"), DESTINATION))
                    .await
                    .status
            })
        })
        .collect();
    let mut accepted = 0;
    for claim in claims {
        if claim.await.unwrap() == StatusCode::OK {
            accepted += 1;
        }
    }

    assert_eq!(accepted, 3);
    assert_eq!(fixture.mailed().len(), 3);
}

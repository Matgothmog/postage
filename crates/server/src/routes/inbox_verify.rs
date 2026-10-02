//! `GET` and `POST /api/inbox/verify` (`web/src/app/api/inbox/verify/route.ts`):
//! the confirmation screen's poll, and the emailed code going in.

use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use postage_core::secret::VerificationKey;
use postage_core::verification::{MAX_ATTEMPTS, code_matches};
use reqwest::Url;
use serde::Serialize;
use serde_json::Value;

use super::js::{field, is_truthy, js_trim, lookup_text, request_json};
use super::{Exit, RouteResult, invalid_request, json, refuse};
use crate::app::AppState;
use crate::auth::{WalletAuth, confirm_statement};
use crate::claims::settle_claim;
use crate::config::message_id_secret;
use crate::db::Db;
use crate::db::claims::{
    AttemptOutcome, InboxClaim, attach_destination_to_claim, claim_by_handle,
    consume_attempt_on_claim, mark_code_verified_on_claim,
};

/// Exactly the fields `FinishClaim.tsx`'s poll reads, in the order the
/// TypeScript wrote them. Handing back nothing else is the mitigation for the
/// poll being open.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaimProgress {
    code_verified: bool,
    cloudflare_verified: bool,
    live: bool,
    stalled: bool,
}

/// Polled while the user is on the confirmation screen, so the Cloudflare half
/// ticks over the moment they click the link in its email.
///
/// Deliberately unauthenticated. This poll is the only thing that ever asks
/// Cloudflare whether the destination was confirmed, so a proof requirement
/// would mean a signature prompt before the visitor has agreed to anything.
/// The residual risk, guessing a handle to learn its status, is bounded
/// account-wide by the Cloudflare check budget, not by who is asking.
pub(crate) async fn get(State(state): State<AppState>, RawQuery(query): RawQuery) -> RouteResult {
    let Some(handle) =
        first_query_value(query.as_deref(), "handle").filter(|handle| !handle.is_empty())
    else {
        return Err(refuse(StatusCode::BAD_REQUEST, "handle is required"));
    };

    let settled = match state.db().await {
        Ok(db) => settle_claim(db, state.cloudflare(), &handle, state.now())
            .await
            .map_err(|_| ()),
        Err(_) => Err(()),
    };
    // Cloudflare being unreachable is a reason to keep waiting, not to report
    // progress the caller has already made as undone. The poller ignores this.
    let Ok(settled) = settled else {
        return Err(refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "Waiting on Cloudflare",
        ));
    };
    let Some(claim) = settled else {
        return Err(refuse(
            StatusCode::NOT_FOUND,
            "Nothing is being claimed here",
        ));
    };

    Ok(json(
        StatusCode::OK,
        &ClaimProgress {
            code_verified: claim.code_verified,
            cloudflare_verified: claim.cloudflare_verified,
            live: claim.live,
            stalled: claim.stalled,
        },
    ))
}

/// `new URL(request.url).searchParams.get(name)`: the first value, decoded as
/// a form would be.
fn first_query_value(query: Option<&str>, name: &str) -> Option<String> {
    let url = Url::parse(&format!(
        "http://query.invalid/?{}",
        query.unwrap_or_default()
    ))
    .ok()?;
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// Confirms the claimer can read the address they pointed the handle at.
///
/// The code proves the address; it does not prove who is claiming. A claim
/// names a wallet and that wallet ends up holding the inbox's earnings, so
/// without this anyone who could get a stranger to type a code they had been
/// sent unasked completed a claim on a wallet the sender of that code chose.
/// So the caller has to hold the wallet the claim was started with.
pub(crate) async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> RouteResult {
    // Unguarded in the TypeScript, where all of this was a bare 500, a code of
    // the wrong type only after an attempt had been counted. Every field is
    // checked before anything is spent.
    let Ok(request) = request_json(&body) else {
        return Err(invalid_request());
    };
    let (handle, code) = read_confirmation(&request)?;

    let db = state.db().await?;
    let now = state.now();
    let Some(claim) = claim_by_handle(db, &handle).await? else {
        return Err(refuse(
            StatusCode::NOT_FOUND,
            "Nothing is being claimed here",
        ));
    };

    // Before the attempt is counted, so failing to prove the wallet cannot
    // burn the real claimer's guesses.
    if !holds_claim_wallet(&state, db, &headers, &claim, now).await? {
        return Err(refuse(
            StatusCode::UNAUTHORIZED,
            "Sign in with the wallet that started this claim to confirm it",
        ));
    }
    check_code(&state, db, &claim, code, now).await?;
    // Credited to the claim the code was checked against. If it has been
    // started over since, the code proved nothing about the claim now there.
    if !mark_code_verified_on_claim(db, &claim, now).await? {
        return Err(claim_expired());
    }

    // Only now does Cloudflare hear about the address, and the claimer does
    // nothing to make that happen. Failing here is handled by the poller,
    // which will try again.
    if let Ok(destination) = state
        .cloudflare()
        .ensure_destination(&claim.destination)
        .await
    {
        let _ = attach_destination_to_claim(
            db,
            &claim.handle,
            &claim.code_hash,
            &destination.id,
            destination.verified_at,
            now,
        )
        .await;
    }

    // The code is accepted and recorded either way. If Cloudflare cannot be
    // reached right now the claim simply waits, rather than telling someone
    // the code they just got right was wrong.
    let settled = settle_claim(db, state.cloudflare(), &claim.handle, now)
        .await
        .ok()
        .flatten();
    Ok(json(
        StatusCode::OK,
        &ClaimProgress {
            code_verified: true,
            cloudflare_verified: settled
                .as_ref()
                .is_some_and(|state| state.cloudflare_verified),
            live: settled.as_ref().is_some_and(|state| state.live),
            stalled: settled.as_ref().is_some_and(|state| state.stalled),
        },
    ))
}

/// The handle to look up, and the code as typed. A code that is not a string
/// is refused here; in the TypeScript it threw at its `trim()` after the
/// attempt had been counted.
fn read_confirmation(request: &Value) -> Result<(String, &str), Exit> {
    let handle = field(request, "handle");
    let code = field(request, "code");
    let (Some(handle), true) = (
        handle.filter(|value| is_truthy(Some(value))),
        is_truthy(code),
    ) else {
        return Err(refuse(
            StatusCode::BAD_REQUEST,
            "handle and code are required",
        ));
    };
    let code = match code {
        Some(Value::String(code)) => js_trim(code),
        _ => return Err(invalid_request()),
    };
    Ok((lookup_text(handle).map_err(|_| invalid_request())?, code))
}

/// Whether this request comes from whoever holds the wallet the claim was
/// started with.
async fn holds_claim_wallet(
    state: &AppState,
    db: &Db,
    headers: &HeaderMap,
    claim: &InboxClaim,
    now: i64,
) -> Result<bool, Exit> {
    let nonce_key = state.wallet_nonce_key()?;
    let auth = WalletAuth {
        db,
        nonce_key: &nonce_key,
    };
    let statement = |issued_at| confirm_statement(&claim.handle, &claim.wallet, issued_at);
    Ok(auth
        .holds_wallet(state.privy(), headers, &claim.wallet, statement, now)
        .await)
}

/// The refusal for a claim whose code is no longer accepted: expired, or
/// started over since this request read it.
fn claim_expired() -> Exit {
    refuse(
        StatusCode::GONE,
        "That code has expired. Start again to get a new one",
    )
}

/// Spends one attempt on `code`, refusing an expired claim, an exhausted
/// one, and a wrong code.
async fn check_code(
    state: &AppState,
    db: &Db,
    claim: &InboxClaim,
    code: &str,
    now: i64,
) -> Result<(), Exit> {
    if claim.expires_at <= now {
        return Err(claim_expired());
    }
    match consume_attempt_on_claim(db, claim, i64::from(MAX_ATTEMPTS), now).await? {
        AttemptOutcome::Taken => {}
        AttemptOutcome::ClaimMoved => return Err(claim_expired()),
        AttemptOutcome::Exhausted => {
            return Err(refuse(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many wrong codes. Start again to get a new one",
            ));
        }
    }

    let key = VerificationKey::derive(&message_id_secret(state.env().lookup())?)?;
    if code_matches(&key, &claim.handle, code, &claim.code_hash) {
        return Ok(());
    }
    let left = i64::from(MAX_ATTEMPTS) - claim.attempts - 1;
    let message = if left > 0 {
        format!("That code is wrong. {left} tries left")
    } else {
        "That was the last try. Start again".to_owned()
    };
    Err(refuse(StatusCode::BAD_REQUEST, &message))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;
    use postage_core::secret::{MessageIdSecret, WalletNonceKey};
    use postage_core::verification::hash_code;
    use postage_core::wallet_nonce::mint_wallet_nonce;
    use postage_core::wallet_proof::{
        IDENTITY_TOKEN_HEADER, WALLET_HEADER, signed_proof, with_nonce,
    };
    use rand_core::{OsRng, TryRngCore};
    use serde_json::json;

    use super::*;
    use crate::cloudflare::Cloudflare;
    use crate::config::Env;
    use crate::db::claims::{NewClaim, start_claim};
    use crate::db::testing::TestDb;
    use crate::http_stub::{Reply, Stub, serve, serve_with};
    use crate::routes::router;
    use crate::routes::testing::{Answer, NOW, clock_at, get as get_request, post_with, send};

    const PATH: &str = "/api/inbox/verify";
    const HANDLE: &str = "demo";
    const CODE: &str = "123456";

    fn secret() -> String {
        "x".repeat(32)
    }

    fn signer() -> PrivateKeySigner {
        PrivateKeySigner::from_slice(&[0x11; 32]).unwrap()
    }

    fn wallet() -> String {
        signer().address().to_checksum(None)
    }

    struct Fixture {
        db: Arc<TestDb>,
        cloudflare: Stub,
    }

    /// A claim on `demo` started by `wallet()`, with `CODE` emailed, and a
    /// Cloudflare that registers the destination unverified.
    async fn fixture() -> Fixture {
        let db = Arc::new(TestDb::fresh().await);
        let key = VerificationKey::derive(&MessageIdSecret::new(secret()).unwrap()).unwrap();
        start_claim(
            &db,
            &NewClaim {
                handle: HANDLE.to_owned(),
                destination: "victim@example.com".to_owned(),
                wallet: wallet(),
                code_hash: hash_code(&key, HANDLE, CODE),
                expires_at: NOW + 900,
                cf_address_id: None,
                cf_verified_at: None,
            },
            NOW,
        )
        .await
        .unwrap();
        let address = r#"{"success":true,"errors":[],"result":{"id":"addr_1","email":"victim@example.com","verified":null}}"#;
        let cloudflare = serve(&[
            ("/accounts/acct/email/routing/addresses", 200, address),
            (
                "/accounts/acct/email/routing/addresses/addr_1",
                200,
                address,
            ),
        ])
        .await;
        Fixture { db, cloudflare }
    }

    impl Fixture {
        fn app(&self) -> axum::Router {
            router(
                AppState::builder(Env::fixed([
                    ("MESSAGE_ID_SECRET", secret()),
                    ("NEXT_PUBLIC_PRIVY_APP_ID", "test-app".to_owned()),
                ]))
                .clock(clock_at(NOW))
                .db(self.db.clone())
                .cloudflare(
                    Cloudflare::new(
                        reqwest::Client::default(),
                        "acct".to_owned(),
                        "t".to_owned(),
                    )
                    .with_api_base(&self.cloudflare.base),
                )
                .build(),
            )
        }

        async fn confirm(&self, headers: &[(&str, &str)]) -> Answer {
            confirm_code(self, CODE, headers).await
        }

        async fn poll(&self) -> Answer {
            send(self.app(), get_request(&format!("{PATH}?handle={HANDLE}"))).await
        }

        async fn attempts(&self) -> i64 {
            claim_by_handle(&self.db, HANDLE)
                .await
                .unwrap()
                .unwrap()
                .attempts
        }
    }

    async fn confirm_code(fixture: &Fixture, code: &str, headers: &[(&str, &str)]) -> Answer {
        let body = json!({ "handle": HANDLE, "code": code }).to_string();
        send(fixture.app(), post_with(PATH, &body, headers)).await
    }

    /// The headers a browser holding the claim's wallet sends.
    fn wallet_proof() -> Vec<(&'static str, String)> {
        let key = WalletNonceKey::derive(&MessageIdSecret::new(secret()).unwrap()).unwrap();
        let nonce = mint_wallet_nonce(&key, &wallet(), NOW, &mut OsRng.unwrap_err());
        let issued_at = NOW as f64;
        let statement = with_nonce(&confirm_statement(HANDLE, &wallet(), issued_at), &nonce);
        let signature = signer().sign_message_sync(statement.as_bytes()).unwrap();
        signed_proof(
            &wallet(),
            issued_at,
            &format!("0x{}", alloy_primitives::hex::encode(signature.as_bytes())),
            &nonce,
        )
    }

    async fn confirm_as_wallet(fixture: &Fixture, code: &str) -> Answer {
        let proof = wallet_proof();
        let headers: Vec<(&str, &str)> = proof
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        confirm_code(fixture, code, &headers).await
    }

    /// The attack this route used to allow: send someone a code they did not
    /// ask for, get them to type it, and the claim completes on a wallet they
    /// have never seen.
    #[tokio::test]
    async fn a_correct_code_alone_does_not_complete_a_claim() {
        let fixture = fixture().await;

        let answer = fixture.confirm(&[]).await;

        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
        let claim = claim_by_handle(&fixture.db, HANDLE).await.unwrap().unwrap();
        assert_eq!(
            claim.code_verified_at, None,
            "the right code from the wrong person must leave the claim exactly as it was"
        );
    }

    /// The wallet is public, so naming it is not holding it.
    #[tokio::test]
    async fn naming_the_claims_wallet_is_not_proof_of_holding_it() {
        let fixture = fixture().await;

        let answer = fixture.confirm(&[(WALLET_HEADER, &wallet())]).await;

        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn an_unverifiable_identity_token_does_not_stand_in_for_the_wallet() {
        let fixture = fixture().await;

        let answer = fixture
            .confirm(&[(IDENTITY_TOKEN_HEADER, "not.a.token")])
            .await;

        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    }

    /// A refusal to prove the wallet must not spend the real claimer's
    /// guesses, or anyone who knows a handle could lock them out.
    #[tokio::test]
    async fn failing_to_prove_the_wallet_costs_the_claimer_no_attempts() {
        let fixture = fixture().await;

        for _ in 0..10 {
            fixture.confirm(&[]).await;
        }

        assert_eq!(fixture.attempts().await, 0);
    }

    /// The GET is deliberately unauthenticated; its only defence against a
    /// stranger who knows the handle is disclosing nothing beyond what the
    /// poll needs.
    #[tokio::test]
    async fn polling_a_fresh_claim_discloses_only_the_fields_the_poller_depends_on() {
        let fixture = fixture().await;

        let answer = fixture.poll().await;

        assert_eq!(answer.status, StatusCode::OK);
        assert_eq!(
            answer.text,
            r#"{"codeVerified":false,"cloudflareVerified":false,"live":false,"stalled":false}"#
        );
    }

    /// The poller compares `codeVerified` against the value it holds; a
    /// missing field would force a state update on every tick.
    #[tokio::test]
    async fn code_verified_comes_back_as_an_explicit_boolean_not_omitted() {
        let fixture = fixture().await;

        let answer = fixture.poll().await;

        assert!(answer.body["codeVerified"].is_boolean());
    }

    #[tokio::test]
    async fn the_claims_wallet_with_the_right_code_completes_the_code_half() {
        let fixture = fixture().await;

        let answer = confirm_as_wallet(&fixture, CODE).await;

        assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
        assert_eq!(
            answer.text,
            r#"{"codeVerified":true,"cloudflareVerified":false,"live":false,"stalled":false}"#
        );
        let claim = claim_by_handle(&fixture.db, HANDLE).await.unwrap().unwrap();
        assert_eq!(claim.code_verified_at, Some(NOW));
        assert_eq!(claim.cf_address_id.as_deref(), Some("addr_1"));
    }

    #[tokio::test]
    async fn a_wrong_code_from_the_claims_wallet_counts_down_the_tries() {
        let fixture = fixture().await;

        let answer = confirm_as_wallet(&fixture, "000000").await;

        assert_eq!(answer.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            answer.body,
            json!({ "error": "That code is wrong. 4 tries left" })
        );
        assert_eq!(fixture.attempts().await, 1);
    }

    #[tokio::test]
    async fn a_poll_or_confirmation_naming_nobody_is_refused() {
        let fixture = fixture().await;

        let no_handle = send(fixture.app(), get_request(PATH)).await;
        let empty_handle = send(fixture.app(), get_request(&format!("{PATH}?handle="))).await;
        let unclaimed = send(fixture.app(), get_request(&format!("{PATH}?handle=other"))).await;
        let no_code = send(
            fixture.app(),
            post_with(PATH, &json!({ "handle": HANDLE }).to_string(), &[]),
        )
        .await;

        assert_eq!(no_handle.status, StatusCode::BAD_REQUEST);
        assert_eq!(no_handle.body, json!({ "error": "handle is required" }));
        assert_eq!(empty_handle.status, StatusCode::BAD_REQUEST);
        assert_eq!(unclaimed.status, StatusCode::NOT_FOUND);
        assert_eq!(no_code.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            no_code.body,
            json!({ "error": "handle and code are required" })
        );
    }

    /// Each of these was a bare 500 in the TypeScript, a code of the wrong
    /// type only after an attempt had been counted. A deliberate change: all
    /// are refused with a 400 first, and the claimer keeps every guess.
    #[tokio::test]
    async fn a_malformed_confirmation_is_refused_before_an_attempt_is_spent() {
        let fixture = fixture().await;

        for (raw, error) in [
            ("not json", "Invalid request"),
            ("", "Invalid request"),
            ("null", "handle and code are required"),
            ("[]", "handle and code are required"),
            (r#"{"handle":"demo","code":123456}"#, "Invalid request"),
            (r#"{"handle":"demo","code":true}"#, "Invalid request"),
            (r#"{"handle":"demo","code":["000000"]}"#, "Invalid request"),
            (r#"{"handle":"demo","code":{"a":1}}"#, "Invalid request"),
            (r#"{"handle":[],"code":"000000"}"#, "Invalid request"),
            (r#"{"handle":{"a":1},"code":"000000"}"#, "Invalid request"),
        ] {
            let proof = wallet_proof();
            let headers: Vec<(&str, &str)> = proof
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();

            let answer = send(fixture.app(), post_with(PATH, raw, &headers)).await;

            assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{raw}");
            assert_eq!(answer.body, json!({ "error": error }), "{raw}");
        }
        assert_eq!(fixture.attempts().await, 0, "no attempt spent");
        let claim = claim_by_handle(&fixture.db, HANDLE).await.unwrap().unwrap();
        assert_eq!(claim.code_verified_at, None);
    }

    /// The claim is started over while Cloudflare is being asked to register
    /// the old address. That address belongs to the claim that was checked, not
    /// to the one that replaced it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_address_registered_for_one_claim_is_not_attached_to_its_replacement() {
        let mut fixture = fixture().await;
        let db = fixture.db.clone();
        let address = r#"{"success":true,"errors":[],"result":{"id":"addr_1","email":"victim@example.com","verified":null}}"#;
        fixture.cloudflare = serve_with(move |_| {
            let restarted = NewClaim {
                handle: HANDLE.to_owned(),
                destination: "other@example.com".to_owned(),
                wallet: wallet(),
                code_hash: "restarted".to_owned(),
                expires_at: NOW + 1800,
                cf_address_id: None,
                cf_verified_at: None,
            };
            let db = db.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(start_claim(&db, &restarted, NOW + 1))
                    .unwrap();
            });
            Reply::new(200, address)
        })
        .await;

        let answer = confirm_as_wallet(&fixture, CODE).await;

        assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
        let claim = claim_by_handle(&fixture.db, HANDLE).await.unwrap().unwrap();
        assert_eq!(claim.code_hash, "restarted");
        assert_eq!(claim.cf_address_id, None);
    }

    #[test]
    fn the_first_value_of_a_repeated_parameter_is_the_one_read_and_it_is_decoded() {
        assert_eq!(
            first_query_value(Some("handle=a%20b&handle=c"), "handle").as_deref(),
            Some("a b")
        );
        assert_eq!(first_query_value(Some("x=1"), "handle"), None);
        assert_eq!(first_query_value(None, "handle"), None);
    }
}

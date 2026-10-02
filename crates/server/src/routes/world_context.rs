//! `POST /api/world/context` (`web/src/app/api/world/context/route.ts`): signs
//! the rp_context every World ID 4.0 proof request has to carry.
//!
//! The signature says that *we* asked for this proof, for this action, and
//! nothing about who asked us. Signing for anyone would hand a stranger our
//! name: they could put the context in front of their own visitors and
//! present the proofs here as their own. The challenge token closes that: it
//! is issued by us to one address for one message, so a context can only be
//! minted by someone we already invited to prove something, a bounded number
//! of times, against a challenge that is still open.
//!
//! The token is checked before the signing key is even read, so an anonymous
//! caller learns nothing about how this deployment is configured.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use postage_core::rp_context::RP_CONTEXT_TTL_SECONDS;
use postage_core::rp_signature::{RpSignature, sign_request};
use rand_core::{OsRng, TryRngCore};
use serde::Serialize;
use serde_json::Value;

use super::js::{TypeError, property, request_json};
use super::{Exit, RouteResult, json, refuse};
use crate::app::AppState;
use crate::config::required;
use crate::db::Db;
use crate::db::challenges::challenge_by_token;
use crate::db::issued_contexts::{purge_expired_contexts, record_issued_context};

const DANGEROUS: &str = "dangerous";

#[derive(Serialize)]
struct SignedContext {
    rp_id: String,
    nonce: String,
    created_at: u64,
    expires_at: u64,
    signature: String,
    /// Returned so the browser reads the action IDKit is told to use from the
    /// same signed payload rather than a separately set variable of its own;
    /// two copies of one value is what let them drift apart before.
    action: String,
}

pub(crate) async fn post(State(state): State<AppState>, body: Bytes) -> RouteResult {
    let Ok(body) = request_json(&body) else {
        return Err(refuse(StatusCode::BAD_REQUEST, "Body must be JSON"));
    };
    let token = match property(&body, "token")? {
        Some(Value::String(token)) if !token.is_empty() => token,
        _ => {
            return Err(refuse(
                StatusCode::BAD_REQUEST,
                "A challenge token is required",
            ));
        }
    };

    let db = state.db().await?;
    check_open(db, token).await?;
    let (signature, action, rp_id) = sign(&state)?;

    let now = state.now();
    purge_expired_contexts(db, now).await?;
    let issued = record_issued_context(
        db,
        token,
        &signature.nonce,
        clamp(signature.created_at),
        clamp(signature.expires_at),
        now,
    )
    .await?;
    // The signature is discarded rather than returned: it was cheap to make
    // and is worth nothing to us, but it is a bearer credential to whoever
    // holds it.
    if !issued {
        return Err(refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many verification attempts. Wait a moment and try again.",
        ));
    }

    Ok(json(
        StatusCode::OK,
        &SignedContext {
            rp_id,
            nonce: signature.nonce,
            created_at: signature.created_at,
            expires_at: signature.expires_at,
            signature: signature.sig,
            action,
        },
    ))
}

/// Refuses an unknown token, a challenge no lane will deliver, and one that
/// has already been answered.
async fn check_open(db: &Db, token: &str) -> Result<(), Exit> {
    let Some(challenge) = challenge_by_token(db, token).await? else {
        return Err(refuse(StatusCode::NOT_FOUND, "Unknown challenge"));
    };

    // Refused here as well as at /api/world/verify, which is where the sender
    // is told why. Signing for mail no lane will deliver spends the key on a
    // proof request whose only possible use is somewhere else.
    if challenge.tier == DANGEROUS {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "This challenge cannot be verified",
        ));
    }
    if challenge.resolved_at.is_some() {
        return Err(refuse(
            StatusCode::CONFLICT,
            "This challenge has already been answered",
        ));
    }
    Ok(())
}

/// A fresh context for the configured action, with the action and the
/// relying party id it was signed for. Read only now, after the token, so a
/// stranger learns nothing about the configuration.
fn sign(state: &AppState) -> Result<(RpSignature, String, String), Exit> {
    let env = state.env().lookup();
    let signing_key = required(&env, "WORLD_RP_SIGNING_KEY")?;
    let action = required(&env, "WORLD_ACTION")?;
    let rp_id = required(&env, "WORLD_RP_ID")?;

    let now = state.now();
    let signed_at = u64::try_from(now)
        .map_err(|_| TypeError(format!("the clock reads {now}, before the epoch")))?;
    let mut random = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut random)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let signature = sign_request(
        &signing_key,
        Some(&action),
        RP_CONTEXT_TTL_SECONDS,
        signed_at,
        &random,
    )?;
    Ok((signature, action, rp_id))
}

/// The window as the table stores it. Both ends come from a clock that read
/// as an `i64` a moment ago, so this never actually saturates.
fn clamp(seconds: u64) -> i64 {
    i64::try_from(seconds).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::config::Env;
    use crate::db::challenges::claim_challenge;
    use crate::db::issued_contexts::{MAX_LIVE_CONTEXTS_PER_TOKEN, consume_issued_context};
    use crate::db::testing::TestDb;
    use crate::log::captured;
    use crate::routes::router;
    use crate::routes::testing::{Answer, NOW, challenge, clock_at, post, seed, send};

    const PATH: &str = "/api/world/context";
    const TOKEN: &str = "tok";

    fn env(signing_key: bool) -> Env {
        let mut vars = vec![
            ("WORLD_ACTION", "send-free".to_owned()),
            ("WORLD_RP_ID", "app_test_rp_id".to_owned()),
        ];
        if signing_key {
            vars.push(("WORLD_RP_SIGNING_KEY", format!("0x{}", "11".repeat(32))));
        }
        Env::fixed(vars)
    }

    /// A database with one open, commercial challenge under `TOKEN`.
    async fn seeded() -> Arc<TestDb> {
        let db = Arc::new(TestDb::fresh().await);
        seed(&db, &challenge(TOKEN, "commercial")).await;
        db
    }

    async fn context(db: &Arc<TestDb>, body: &str) -> Answer {
        context_under(db, env(true), body).await
    }

    async fn context_under(db: &Arc<TestDb>, env: Env, body: &str) -> Answer {
        let app = router(
            AppState::builder(env)
                .clock(clock_at(NOW))
                .db(db.clone())
                .build(),
        );
        send(app, post(PATH, body)).await
    }

    fn token_body(token: &str) -> String {
        json!({ "token": token }).to_string()
    }

    #[tokio::test]
    async fn signs_an_rp_context_for_a_sender_holding_an_open_challenge() {
        let db = seeded().await;

        let answer = context(&db, &token_body(TOKEN)).await;

        assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
        assert_eq!(answer.body["rp_id"], "app_test_rp_id");
        let nonce = answer.body["nonce"].as_str().unwrap();
        let signature = answer.body["signature"].as_str().unwrap();
        assert!(is_lower_hex(nonce, 64), "{nonce}");
        assert!(is_lower_hex(signature, 130), "{signature}");
    }

    fn is_lower_hex(text: &str, digits: usize) -> bool {
        text.strip_prefix("0x").is_some_and(|hex| {
            hex.len() == digits
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    }

    #[tokio::test]
    async fn returns_the_action_it_signed_into_the_context_so_the_browser_reads_it_from_here() {
        let db = seeded().await;

        let answer = context(&db, &token_body(TOKEN)).await;

        assert_eq!(answer.body["action"], "send-free");
    }

    #[tokio::test]
    async fn signs_a_window_the_clients_own_polling_window_can_fit_inside() {
        let db = seeded().await;

        let answer = context(&db, &token_body(TOKEN)).await;

        let created = answer.body["created_at"].as_u64().unwrap();
        let expires = answer.body["expires_at"].as_u64().unwrap();
        assert_eq!(expires - created, RP_CONTEXT_TTL_SECONDS);
        assert_eq!(created, NOW as u64);
    }

    #[tokio::test]
    async fn binds_the_nonce_it_signed_to_the_challenge_it_signed_it_for() {
        let db = seeded().await;

        let answer = context(&db, &token_body(TOKEN)).await;
        let nonce = answer.body["nonce"].as_str().unwrap();

        assert!(
            !consume_issued_context(&db, "another-token", nonce, NOW)
                .await
                .unwrap()
        );
        assert!(
            consume_issued_context(&db, TOKEN, nonce, NOW)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn refuses_a_caller_presenting_no_challenge_token_at_all() {
        let db = seeded().await;

        let answer = context(&db, "{}").await;

        assert_eq!(answer.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            answer.body,
            json!({ "error": "A challenge token is required" })
        );
        assert!(answer.body.get("signature").is_none());
    }

    #[tokio::test]
    async fn refuses_a_body_that_is_not_json() {
        let db = seeded().await;

        let answer = context(&db, "not json").await;

        assert_eq!(answer.status, StatusCode::BAD_REQUEST);
        assert!(answer.body.get("signature").is_none());
    }

    #[tokio::test]
    async fn refuses_a_challenge_token_nobody_was_ever_issued() {
        let db = seeded().await;

        let answer = context(&db, &token_body("bogus")).await;

        assert_eq!(answer.status, StatusCode::NOT_FOUND);
        assert_eq!(answer.body, json!({ "error": "Unknown challenge" }));
    }

    #[tokio::test]
    async fn refuses_a_challenge_that_has_already_been_settled() {
        let db = seeded().await;
        claim_challenge(&db, TOKEN, "human", NOW).await.unwrap();

        let answer = context(&db, &token_body(TOKEN)).await;

        assert_eq!(answer.status, StatusCode::CONFLICT);
        assert!(answer.body.get("signature").is_none());
    }

    #[tokio::test]
    async fn refuses_a_challenge_no_proof_of_personhood_could_ever_release() {
        let db = seeded().await;
        seed(&db, &challenge("danger", "dangerous")).await;

        let answer = context(&db, &token_body("danger")).await;

        assert_eq!(answer.status, StatusCode::FORBIDDEN);
        assert!(answer.body.get("signature").is_none());
    }

    #[tokio::test]
    async fn bounds_how_many_contexts_one_challenge_token_can_mint() {
        let db = seeded().await;
        for issued in 0..MAX_LIVE_CONTEXTS_PER_TOKEN {
            let answer = context(&db, &token_body(TOKEN)).await;
            assert_eq!(
                answer.status,
                StatusCode::OK,
                "context {issued} should have been signed"
            );
        }

        let answer = context(&db, &token_body(TOKEN)).await;

        assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
        assert!(answer.body.get("signature").is_none());
    }

    /// A bare 500, which is what a misconfigured deployment should tell the
    /// world: naming the missing variable in a response would hand an
    /// anonymous caller a map of the server's secrets. The name goes to the
    /// log instead.
    #[tokio::test]
    async fn fails_loudly_rather_than_signing_without_a_configured_key() {
        let db = seeded().await;

        let (answer, lines) =
            captured::during(context_under(&db, env(false), &token_body(TOKEN))).await;

        assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(answer.text, "");
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("WORLD_RP_SIGNING_KEY is not set"),
            "{lines:?}"
        );
    }

    /// The token is checked before any setting is read, so an anonymous
    /// caller cannot tell a configured deployment from an unconfigured one.
    #[tokio::test]
    async fn an_unknown_token_is_refused_before_the_missing_key_is_noticed() {
        let db = seeded().await;

        let answer = context_under(&db, env(false), &token_body("bogus")).await;

        assert_eq!(answer.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_token_that_is_not_a_string_is_refused_as_missing() {
        let db = seeded().await;

        for token in [json!(42), json!(""), json!(null), json!(["tok"])] {
            let answer = context(&db, &json!({ "token": token }).to_string()).await;
            assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{token}");
        }
    }

    #[tokio::test]
    async fn the_fields_go_out_in_the_order_the_typescript_wrote_them() {
        let db = seeded().await;

        let answer = context(&db, &token_body(TOKEN)).await;

        let order: Vec<&str> = [
            "rp_id",
            "nonce",
            "created_at",
            "expires_at",
            "signature",
            "action",
        ]
        .into_iter()
        .filter(|key| answer.text.contains(&format!("\"{key}\"")))
        .collect();
        assert_eq!(order.len(), 6);
        let positions: Vec<usize> = order
            .iter()
            .map(|key| answer.text.find(&format!("\"{key}\"")).unwrap())
            .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "{}",
            answer.text
        );
    }
}

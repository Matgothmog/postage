//! `POST /api/wallet-nonce` (`web/src/app/api/wallet-nonce/route.ts`): hands a
//! browser the one-time value it is about to ask a wallet to sign.
//!
//! Open, and it has to be: a session with no Privy identity token has nothing
//! to prove until it holds a nonce to sign. A nonce is worth nothing to
//! whoever asks for one: it names a wallet the caller already knew, and the
//! only thing it can be turned into is a request that also carries a signature
//! from that wallet's key. Minting touches no database and no shared counter,
//! so there is nothing here for a stranger to exhaust; the single-use record
//! is taken on the way back in, where a valid signature has been presented.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use postage_core::quote::parse_address;
use postage_core::wallet_nonce::{WALLET_NONCE_TTL_SECONDS, mint_wallet_nonce};
use rand_core::{OsRng, TryRngCore};
use serde::Serialize;
use serde_json::Value;

use super::js::{field, request_json};
use super::{RouteResult, json, refuse};
use crate::app::AppState;

/// What a nonce is worth and nothing about how it was made.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Minted {
    nonce: String,
    expires_in: i64,
}

pub(crate) async fn post(State(state): State<AppState>, body: Bytes) -> RouteResult {
    let Ok(body) = request_json(&body) else {
        return Err(refuse(StatusCode::BAD_REQUEST, "Body must be JSON"));
    };

    let wallet = match field(&body, "wallet") {
        Some(Value::String(wallet)) if parse_address(wallet).is_ok() => wallet,
        _ => {
            return Err(refuse(
                StatusCode::BAD_REQUEST,
                "A valid wallet is required",
            ));
        }
    };

    let key = state.wallet_nonce_key()?;
    let nonce = mint_wallet_nonce(&key, wallet, state.now(), &mut OsRng.unwrap_err());
    Ok(json(
        StatusCode::OK,
        &Minted {
            nonce,
            expires_in: WALLET_NONCE_TTL_SECONDS,
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use alloy_signer_local::PrivateKeySigner;
    use postage_core::secret::{MessageIdSecret, WalletNonceKey};
    use postage_core::wallet_nonce::verify_wallet_nonce;
    use serde_json::json;

    use super::*;
    use crate::config::Env;
    use crate::log::captured;
    use crate::routes::router;
    use crate::routes::testing::{Answer, NOW, clock_at, post, send};

    const PATH: &str = "/api/wallet-nonce";

    fn secret() -> String {
        "x".repeat(32)
    }

    fn app() -> axum::Router {
        router(
            AppState::builder(Env::fixed([("MESSAGE_ID_SECRET", secret())]))
                .clock(clock_at(NOW))
                .build(),
        )
    }

    fn wallet(byte: u8) -> String {
        PrivateKeySigner::from_slice(&[byte; 32])
            .unwrap()
            .address()
            .to_checksum(None)
    }

    async fn ask(body: &str) -> Answer {
        send(app(), post(PATH, body)).await
    }

    fn reader_accepts(nonce: &str, wallet: &str) -> bool {
        let key = WalletNonceKey::derive(&MessageIdSecret::new(secret()).unwrap()).unwrap();
        verify_wallet_nonce(&key, nonce, wallet, NOW).is_some()
    }

    #[tokio::test]
    async fn a_nonce_the_route_hands_out_is_one_the_reader_will_accept() {
        let answer = ask(&json!({ "wallet": wallet(0x11) }).to_string()).await;

        assert_eq!(answer.status, StatusCode::OK);
        assert!(reader_accepts(
            answer.body["nonce"].as_str().unwrap(),
            &wallet(0x11)
        ));
    }

    /// The binding is made here, at mint time, not asserted later, so a nonce
    /// collected from this route for one address is worthless to another.
    #[tokio::test]
    async fn a_nonce_is_minted_for_the_wallet_that_asked_and_no_other() {
        let answer = ask(&json!({ "wallet": wallet(0x11) }).to_string()).await;

        assert!(!reader_accepts(
            answer.body["nonce"].as_str().unwrap(),
            &wallet(0x22)
        ));
    }

    #[tokio::test]
    async fn two_callers_asking_at_the_same_moment_get_different_nonces() {
        let body = json!({ "wallet": wallet(0x11) }).to_string();
        let (first, second) = tokio::join!(ask(&body), ask(&body));

        assert_ne!(first.body["nonce"], second.body["nonce"]);
    }

    #[tokio::test]
    async fn a_body_that_is_not_json_is_refused() {
        let answer = ask("not json at all").await;

        assert_eq!(answer.status, StatusCode::BAD_REQUEST);
        assert_eq!(answer.body, json!({ "error": "Body must be JSON" }));
    }

    #[tokio::test]
    async fn a_request_naming_no_wallet_is_refused() {
        let answer = ask("{}").await;

        assert_eq!(answer.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            answer.body,
            json!({ "error": "A valid wallet is required" })
        );
    }

    #[tokio::test]
    async fn a_wallet_that_is_not_an_address_is_refused() {
        let answer = ask(r#"{"wallet":"0xnot-an-address"}"#).await;

        assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    }

    /// The type check is not redundant with the address check: a caller
    /// controls the JSON.
    #[tokio::test]
    async fn a_wallet_that_is_not_a_string_at_all_is_refused_rather_than_crashing() {
        let address = wallet(0x11);
        for wallet in [
            json!(42),
            json!(null),
            json!(true),
            json!({ "address": address }),
            json!([address]),
        ] {
            let answer = ask(&json!({ "wallet": wallet }).to_string()).await;
            assert_eq!(answer.status, StatusCode::BAD_REQUEST, "accepted {wallet}");
        }
    }

    /// The response says what a nonce is worth and nothing about how it was
    /// made. Anything derived from the key would be a key-recovery problem
    /// handed out to anyone who asks.
    #[tokio::test]
    async fn the_response_carries_the_nonce_and_its_window_and_no_key_material() {
        let answer = ask(&json!({ "wallet": wallet(0x11) }).to_string()).await;

        let mut keys: Vec<&String> = answer.body.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["expiresIn", "nonce"]);
        assert_eq!(answer.body["expiresIn"], 120);
        assert!(!answer.text.contains(&secret()));
    }

    /// Written in the order the TypeScript wrote it.
    #[tokio::test]
    async fn the_nonce_comes_before_its_window_on_the_wire() {
        let answer = ask(&json!({ "wallet": wallet(0x11) }).to_string()).await;

        assert!(answer.text.starts_with("{\"nonce\":"), "{}", answer.text);
    }

    /// `const { wallet } = body` on a body of `null` threw in the TypeScript,
    /// which Next.js answered with an empty 500. A deliberate change: it is
    /// refused like any other body without a wallet. The route reads no
    /// database, so there is no nonce or pass row for a refusal to spend.
    #[tokio::test]
    async fn a_body_that_is_not_an_object_is_refused_rather_than_crashing() {
        for body in ["null", "[]", "42", r#""0x""#, "true"] {
            let answer = ask(body).await;

            assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(
                answer.body,
                json!({ "error": "A valid wallet is required" }),
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn without_the_secret_the_route_fails_bare_and_logs_which_setting() {
        let app = router(
            AppState::builder(Env::empty())
                .clock(Arc::new(|| NOW))
                .build(),
        );
        let request = post(PATH, &json!({ "wallet": wallet(0x11) }).to_string());

        let (answer, lines) = captured::during(send(app, request)).await;

        assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(answer.text, "");
        assert_eq!(
            lines,
            ["unhandled route error path=/api/wallet-nonce reason=MESSAGE_ID_SECRET is not set"]
        );
    }
}

//! Privy, stood in for: a throwaway P-256 key whose public half the server
//! reads as Privy's JWKS, and identity tokens minted with it for the two
//! people the journeys play (the inbox owner and a sender).

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use postage_server::privy::{JwksError, JwksFetch, JwksResponse};
use rand_core::{OsRng, TryRngCore};
use serde_json::{Value, json};

use crate::BoxError;

pub const PRIVY_APP_ID: &str = "dummy-privy-app-id";
const KEY_ID: &str = "e2e-key";
/// Long enough to outlive the journey that moves the server clock a day on.
const TOKEN_LIFETIME_SECONDS: i64 = 30 * 24 * 60 * 60;

/// One person as Privy would describe them.
#[derive(Debug, Clone)]
pub struct Persona {
    pub user_id: &'static str,
    /// `None` is a wallet-only sign-in, which has to confirm its address by code.
    pub email: Option<&'static str>,
    pub wallet: &'static str,
}

pub const OWNER: Persona = Persona {
    user_id: "did:privy:e2e-owner",
    email: None,
    wallet: "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
};

pub const SENDER: Persona = Persona {
    user_id: "did:privy:e2e-sender",
    email: Some("payer@example.net"),
    wallet: "0x3c44cdddb6a900fa2b585dd299e03d12fa4293bc",
};

pub struct PrivyKey {
    signing: SigningKey,
}

impl PrivyKey {
    pub fn generate() -> Result<Self, BoxError> {
        loop {
            let mut secret = [0_u8; 32];
            OsRng
                .try_fill_bytes(&mut secret)
                .map_err(|error| format!("no randomness for the Privy key: {error}"))?;
            // A scalar outside the curve order is astronomically rare; draw again.
            if let Ok(signing) = SigningKey::from_slice(&secret) {
                return Ok(Self { signing });
            }
        }
    }

    pub fn jwks(&self) -> Value {
        let point = self.signing.verifying_key().to_encoded_point(false);
        let coordinate = |bytes: Option<&p256::FieldBytes>| {
            bytes
                .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
                .unwrap_or_default()
        };
        json!({ "keys": [{
            "kty": "EC",
            "crv": "P-256",
            "x": coordinate(point.x()),
            "y": coordinate(point.y()),
            "kid": KEY_ID,
        }] })
    }

    /// An identity token for `persona`, shaped like the ones Privy issues.
    pub fn token_for(&self, persona: &Persona, now: i64) -> String {
        let header = json!({ "alg": "ES256", "typ": "JWT", "kid": KEY_ID });
        let claims = json!({
            "iss": "privy.io",
            "aud": PRIVY_APP_ID,
            "sub": persona.user_id,
            "exp": now + TOKEN_LIFETIME_SECONDS,
            "linked_accounts": linked_accounts(persona).to_string(),
        });
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let signature: Signature = self.signing.sign(signing_input.as_bytes());
        format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        )
    }
}

fn linked_accounts(persona: &Persona) -> Value {
    let mut accounts = vec![json!({ "type": "wallet", "address": persona.wallet })];
    if let Some(email) = persona.email {
        accounts.push(json!({ "type": "email", "address": email }));
    }
    Value::Array(accounts)
}

/// Serves the JWKS from memory, so the server never asks auth.privy.io.
pub struct MemoryJwks {
    body: Vec<u8>,
}

impl MemoryJwks {
    pub fn serving(jwks: &Value) -> Arc<Self> {
        Arc::new(Self {
            body: jwks.to_string().into_bytes(),
        })
    }
}

impl JwksFetch for MemoryJwks {
    fn fetch(&self, _url: String) -> BoxFuture<'static, Result<JwksResponse, JwksError>> {
        let response = JwksResponse {
            status: 200,
            body: self.body.clone(),
        };
        async move { Ok(response) }.boxed()
    }
}

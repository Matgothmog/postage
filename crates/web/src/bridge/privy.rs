//! Privy for the Leptos app. Replaces the `@privy-io/react-auth` surface
//! `web/src` uses: `PrivyProvider` (`app/providers.tsx`), `usePrivy`,
//! `useWallets`, `useIdentityToken`, `useSignMessage` and `useSendTransaction`.
//!
//! Privy itself runs as a React island inside `bridge.js`; this module starts
//! it, mirrors its auth state into Leptos signals, and wraps its actions as
//! `async` calls returning `Result`.

use alloy_primitives::{Address, B256, Bytes, U256};
use leptos::prelude::*;
use postage_core::contracts::ARC_TESTNET;
use serde::Deserialize;
use serde_json::json;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

use super::{BridgeError, bridge, settle, settle_string};

/// Arc testnet's explorer, as viem's `arcTestnet` names it. Privy shows it in
/// its transaction screens.
pub const ARC_EXPLORER_URL: &str = "https://testnet.arcscan.app";

const LOGO_SVG: &str = "<svg viewBox='0 0 32 32' width='40' height='40' \
xmlns='http://www.w3.org/2000/svg' fill='none'>\
<circle cx='16' cy='16' r='14' stroke='#3B82F6' stroke-width='1.5'/>\
<text x='16' y='20' font-size='18' font-weight='bold' fill='#3B82F6' \
text-anchor='middle'>P</text></svg>";

/// What `PrivyProvider` is mounted with: the app id plus the client config of
/// `web/src/app/providers.tsx`, with Arc testnet from `postage_core` as the
/// only chain.
#[derive(Debug, Clone, PartialEq)]
pub struct PrivyConfig {
    app_id: String,
}

impl PrivyConfig {
    pub fn new(app_id: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
        }
    }

    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    /// The JSON `bridge.js` hands to `PrivyProvider` as `{appId, client}`.
    pub fn to_json(&self) -> String {
        let chain = arc_chain();
        json!({
            "appId": self.app_id,
            "client": {
                "loginMethods": ["email", "passkey"],
                "embeddedWallets": {
                    "ethereum": { "createOnLogin": "users-without-wallets" },
                    "priceDisplay": { "primary": "native-token", "secondary": null },
                },
                "defaultChain": chain,
                "supportedChains": [chain],
                "appearance": {
                    "theme": "#0A0E1A",
                    "accentColor": "#3B82F6",
                    "logo": svg_data_uri(LOGO_SVG),
                    "landingHeader": "Claim your address",
                    "loginMessage": "One address. Spam pays you.",
                    "emailDomain": "usepostage.com",
                    "showWalletLoginFirst": false,
                },
            },
        })
        .to_string()
    }
}

/// Arc testnet in viem's `Chain` shape, which is what Privy takes. Its
/// `rpcUrls.default.http[0]` is the RPC Privy sends embedded-wallet
/// transactions through.
fn arc_chain() -> serde_json::Value {
    json!({
        "id": ARC_TESTNET.id,
        "name": ARC_TESTNET.name,
        "nativeCurrency": {
            "name": ARC_TESTNET.native_currency.name,
            "symbol": ARC_TESTNET.native_currency.symbol,
            "decimals": ARC_TESTNET.native_currency.decimals,
        },
        "rpcUrls": { "default": { "http": ARC_TESTNET.rpc_urls } },
        "blockExplorers": { "default": { "name": "ArcScan", "url": ARC_EXPLORER_URL } },
        "testnet": true,
    })
}

/// An SVG as an `<img src>`-safe data URI (Privy's `appearance.logo` takes a
/// URL; the React app passed an element).
fn svg_data_uri(svg: &str) -> String {
    let mut uri = String::from("data:image/svg+xml,");
    for character in svg.chars() {
        match character {
            '#' => uri.push_str("%23"),
            '<' => uri.push_str("%3C"),
            '>' => uri.push_str("%3E"),
            '"' => uri.push('\''),
            other => uri.push(other),
        }
    }
    uri
}

/// One snapshot of Privy's auth state, as the island reports it after every
/// change of `ready`, `authenticated`, user, email, wallets or identity token.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivyState {
    /// Privy has restored (or failed to find) a session. Nothing else here
    /// means anything until this is true.
    pub ready: bool,
    pub authenticated: bool,
    pub user_id: Option<String>,
    /// The login email as Privy holds it (not lowercased; see `email`).
    pub email: Option<String>,
    /// Connected wallets in `useWallets` order; the app uses the first.
    pub wallets: Vec<ConnectedWallet>,
    /// `useIdentityToken`'s value. Privy refreshes it on its own; each refresh
    /// arrives as a new snapshot.
    pub identity_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedWallet {
    pub address: Address,
    /// `privy` for the embedded wallet, otherwise the external wallet's kind.
    pub wallet_client_type: Option<String>,
}

impl PrivyState {
    pub fn from_json(json: &str) -> Result<Self, BridgeError> {
        serde_json::from_str(json).map_err(|error| BridgeError::Decode(error.to_string()))
    }

    /// `wallets[0]`, the wallet every flow in the app acts for.
    pub fn wallet(&self) -> Option<Address> {
        self.wallets.first().map(|wallet| wallet.address)
    }

    /// The login email lowercased, as `Account.tsx` read it.
    pub fn email_lowercase(&self) -> Option<String> {
        self.email.as_deref().map(str::to_lowercase)
    }
}

/// A transaction for Privy to send on Arc testnet (`chainId` is always
/// `ARC_TESTNET.id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionRequest {
    pub to: Address,
    pub data: Bytes,
    /// Native USDC in base units (18 decimals). `None` sends no value.
    pub value: Option<U256>,
}

impl TransactionRequest {
    /// `value` travels as a decimal string: a JS number cannot hold it, and
    /// the bridge turns it into a `BigInt`.
    pub fn to_json(&self) -> String {
        json!({
            "to": self.to.to_string(),
            "data": self.data.to_string(),
            "value": self.value.map(|value| value.to_string()),
            "chainId": ARC_TESTNET.id,
        })
        .to_string()
    }
}

/// Handle to the running Privy island. `Copy`, so it can sit in Leptos
/// context and be moved into every closure that needs it.
#[derive(Debug, Clone, Copy)]
pub struct Privy {
    state: RwSignal<PrivyState>,
    ready: Memo<bool>,
    authenticated: Memo<bool>,
    wallet: Memo<Option<Address>>,
    email: Memo<Option<String>>,
    identity_token: Memo<Option<String>>,
}

impl Privy {
    /// Mounts `PrivyProvider` and starts mirroring its state. Call once, from
    /// the root component (the memos belong to the calling owner); a second
    /// call fails with code `already_started`.
    pub fn start(config: &PrivyConfig) -> Result<Self, BridgeError> {
        let privy = Self::detached();
        let state = privy.state;
        let on_state = Closure::<dyn FnMut(String)>::new(move |json: String| {
            match PrivyState::from_json(&json) {
                Ok(next) => state.set(next),
                Err(error) => leptos::logging::error!("ignoring a Privy state update: {error}"),
            }
        });
        // Ownership passes to the JS side, which keeps calling it for the
        // life of the page.
        let on_state = on_state.into_js_value();
        bridge()?
            .privy()
            .start(&config.to_json(), on_state.unchecked_ref())
            .map_err(|rejection| BridgeError::from_rejection(&rejection))?;
        Ok(privy)
    }

    /// Signals over a default (not ready, signed out) state, wired to no SDK.
    /// What `start` builds on, and a stand-in for components rendered without
    /// Privy configured.
    pub fn detached() -> Self {
        let state = RwSignal::new(PrivyState::default());
        Self {
            state,
            ready: Memo::new(move |_| state.with(|state| state.ready)),
            authenticated: Memo::new(move |_| state.with(|state| state.authenticated)),
            wallet: Memo::new(move |_| state.with(PrivyState::wallet)),
            email: Memo::new(move |_| state.with(PrivyState::email_lowercase)),
            identity_token: Memo::new(move |_| state.with(|state| state.identity_token.clone())),
        }
    }

    /// The full latest snapshot.
    pub fn state(&self) -> ReadSignal<PrivyState> {
        self.state.read_only()
    }

    pub fn ready(&self) -> Memo<bool> {
        self.ready
    }

    pub fn authenticated(&self) -> Memo<bool> {
        self.authenticated
    }

    /// The first connected wallet (`useWallets().wallets[0]`).
    pub fn wallet(&self) -> Memo<Option<Address>> {
        self.wallet
    }

    /// The login email, lowercased.
    pub fn email(&self) -> Memo<Option<String>> {
        self.email
    }

    /// `useIdentityToken().identityToken`.
    pub fn identity_token(&self) -> Memo<Option<String>> {
        self.identity_token
    }

    /// Opens the login modal. Resolves once the user is signed in (and the
    /// embedded wallet exists, for a new user) or straight away if already
    /// signed in; fails with `exited_auth_flow` if the modal is closed, and
    /// `not_ready` before `ready`.
    pub async fn login(&self) -> Result<(), BridgeError> {
        settle(bridge()?.privy().login()).await.map(drop)
    }

    pub async fn logout(&self) -> Result<(), BridgeError> {
        settle(bridge()?.privy().logout()).await.map(drop)
    }

    /// Personal-sign `message` with `address`, with Privy's confirmation UI
    /// hidden (`uiOptions.showWalletUIs: false`, as `claim-inbox-helpers.ts`
    /// asked). Returns the signature hex.
    pub async fn sign_message(
        &self,
        message: &str,
        address: Address,
    ) -> Result<String, BridgeError> {
        let promise = bridge()?
            .privy()
            .sign_message(message, &address.to_string());
        settle_string(promise).await
    }

    /// Sends `tx` from the active wallet on Arc testnet, through Privy's
    /// confirmation screen. Resolves to the transaction hash once broadcast,
    /// not mined.
    pub async fn send_transaction(&self, tx: &TransactionRequest) -> Result<B256, BridgeError> {
        let promise = bridge()?.privy().send_transaction(&tx.to_json());
        let hash = settle_string(promise).await?;
        hash.parse()
            .map_err(|_| BridgeError::Decode(format!("not a transaction hash: {hash}")))
    }

    /// The freshest identity token (`getIdentityToken()`), refreshing it if
    /// Privy's copy has expired. `None` when signed out.
    pub async fn fresh_identity_token(&self) -> Result<Option<String>, BridgeError> {
        let value = settle(bridge()?.privy().identity_token()).await?;
        Ok(value.as_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, bytes};

    #[test]
    fn config_carries_the_app_id_and_the_providers_tsx_options() {
        let config: serde_json::Value =
            serde_json::from_str(&PrivyConfig::new("app-123").to_json()).unwrap();
        assert_eq!(config["appId"], "app-123");
        let client = &config["client"];
        assert_eq!(client["loginMethods"], json!(["email", "passkey"]));
        assert_eq!(
            client["embeddedWallets"]["ethereum"]["createOnLogin"],
            "users-without-wallets"
        );
        assert_eq!(client["appearance"]["accentColor"], "#3B82F6");
        assert_eq!(client["appearance"]["landingHeader"], "Claim your address");
    }

    #[test]
    fn config_pins_arc_testnet_as_the_only_chain() {
        let config: serde_json::Value =
            serde_json::from_str(&PrivyConfig::new("app").to_json()).unwrap();
        let client = &config["client"];
        assert_eq!(client["defaultChain"]["id"], 5_042_002);
        assert_eq!(client["supportedChains"].as_array().unwrap().len(), 1);
        assert_eq!(client["supportedChains"][0], client["defaultChain"]);
        assert_eq!(
            client["defaultChain"]["rpcUrls"]["default"]["http"][0],
            "https://rpc.testnet.arc.network"
        );
        assert_eq!(client["defaultChain"]["nativeCurrency"]["decimals"], 18);
    }

    #[test]
    fn logo_is_a_data_uri_with_no_raw_hash_or_angle_brackets() {
        let uri = svg_data_uri(LOGO_SVG);
        assert!(uri.starts_with("data:image/svg+xml,%3Csvg"));
        assert!(!uri[5..].contains(['#', '<', '>', '"']));
        assert!(uri.contains("%233B82F6"));
    }

    #[test]
    fn a_signed_in_snapshot_parses() {
        let state = PrivyState::from_json(
            r#"{"ready":true,"authenticated":true,"userId":"did:privy:1",
                "email":"Me@Example.com",
                "wallets":[{"address":"0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7","walletClientType":"privy"}],
                "identityToken":"eyJ"}"#,
        )
        .unwrap();
        assert!(state.ready && state.authenticated);
        assert_eq!(
            state.wallet(),
            Some(address!("0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7"))
        );
        assert_eq!(state.email_lowercase().as_deref(), Some("me@example.com"));
        assert_eq!(state.identity_token.as_deref(), Some("eyJ"));
    }

    #[test]
    fn a_signed_out_snapshot_has_no_wallet_or_email() {
        let state = PrivyState::from_json(
            r#"{"ready":true,"authenticated":false,"userId":null,"email":null,
                "wallets":[],"identityToken":null}"#,
        )
        .unwrap();
        assert_eq!(state.wallet(), None);
        assert_eq!(state.email_lowercase(), None);
    }

    #[test]
    fn a_snapshot_with_a_malformed_wallet_is_a_decode_error() {
        let error = PrivyState::from_json(
            r#"{"ready":true,"authenticated":true,"userId":null,"email":null,
                "wallets":[{"address":"not-an-address","walletClientType":null}],
                "identityToken":null}"#,
        )
        .unwrap_err();
        assert!(matches!(error, BridgeError::Decode(_)));
    }

    #[test]
    fn the_default_state_is_not_ready_and_signed_out() {
        let state = PrivyState::default();
        assert!(!state.ready && !state.authenticated && state.wallet().is_none());
    }

    #[test]
    fn a_transaction_names_arc_and_sends_value_as_a_decimal_string() {
        let tx = TransactionRequest {
            to: address!("0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7"),
            data: bytes!("abcdef"),
            value: Some(U256::from(10).pow(U256::from(18))),
        };
        let json: serde_json::Value = serde_json::from_str(&tx.to_json()).unwrap();
        assert_eq!(json["chainId"], 5_042_002);
        // EIP-55 form, as viem's getAddress spells it.
        assert_eq!(json["to"], "0x4469E869433Cf6Cc08DD54AFc6AC7e288b9A38f7");
        assert_eq!(json["data"], "0xabcdef");
        assert_eq!(json["value"], "1000000000000000000");
    }

    #[test]
    fn a_transaction_without_value_sends_null() {
        let tx = TransactionRequest {
            to: Address::ZERO,
            data: Bytes::new(),
            value: None,
        };
        let json: serde_json::Value = serde_json::from_str(&tx.to_json()).unwrap();
        assert!(json["value"].is_null());
        assert_eq!(json["data"], "0x");
    }
}

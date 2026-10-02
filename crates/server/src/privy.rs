//! Reading Privy identity tokens: the JWKS fetch and its cache around the pure
//! checks in [`postage_core::privy`].
//!
//! The key set is fetched once and reused. It is fetched again for exactly two
//! reasons - a `kid` the cached set does not name, and a fetch that failed -
//! and both are bounded, so neither a stream of forged kids nor a JWKS that is
//! down turns our inbound traffic into outbound traffic to Privy.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use postage_core::privy::{self, JwksBodyError, KeyId, PrivyIdentity};
use postage_core::time::seconds_from_millis;
use serde_json::Value;

use crate::config::{ConfigError, privy_app_id};

/// Bounds how often the JWKS is fetched again, for either of the two reasons
/// it ever is. Privy rotates rarely, so one refetch per window finds a real
/// rotation within a minute of the first token that names the new key.
const REFRESH_GUARD_SECONDS: i64 = 60;

/// How many failed fetches in a row open the cooldown.
///
/// Not one, because a failure is deliberately not remembered as a verdict: a
/// connection that drops once usually does not drop twice. Two, because a
/// second failure a moment after the first is a dependency that is down, and
/// asking a service that is down once per inbound request is how it is held
/// down - and we pay the egress for it.
const FAILURES_BEFORE_COOLDOWN: u32 = 2;

/// How long one JWKS request may take, as the TypeScript's `fetch` was bounded
/// at. A Privy that accepts the connection and never answers would otherwise
/// hold every inbound identity read open behind the one shared fetch.
pub const JWKS_TIMEOUT: Duration = Duration::from_secs(10);

pub fn jwks_url(app_id: &str) -> String {
    format!("https://auth.privy.io/api/v1/apps/{app_id}/jwks.json")
}

/// What came back from the JWKS endpoint, before any of it is trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwksResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JwksError {
    #[error("Privy JWKS request failed: {0}")]
    Transport(String),
    #[error("Privy JWKS returned {0}")]
    Status(u16),
    #[error(transparent)]
    Body(#[from] JwksBodyError),
}

/// The one network call this module makes, behind a trait so tests serve the
/// JWKS from memory.
pub trait JwksFetch: Send + Sync {
    fn fetch(&self, url: String) -> BoxFuture<'static, Result<JwksResponse, JwksError>>;
}

/// The production fetcher.
#[derive(Debug, Clone)]
pub struct HttpJwks {
    client: reqwest::Client,
}

impl Default for HttpJwks {
    fn default() -> Self {
        Self::with_timeout(JWKS_TIMEOUT)
    }
}

impl HttpJwks {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// A fetcher whose every request is cut off after `timeout`.
    fn with_timeout(timeout: Duration) -> Self {
        // A builder that cannot be built has no TLS backend, which
        // `Client::default()` panics on too; the fallback is never a client
        // without the bound.
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl JwksFetch for HttpJwks {
    fn fetch(&self, url: String) -> BoxFuture<'static, Result<JwksResponse, JwksError>> {
        let client = self.client.clone();
        async move {
            let transport = |error: reqwest::Error| JwksError::Transport(error.to_string());
            let response = client.get(url).send().await.map_err(transport)?;
            let status = response.status().as_u16();
            let body = response.bytes().await.map_err(transport)?.to_vec();
            Ok(JwksResponse { status, body })
        }
        .boxed()
    }
}

/// Epoch seconds. Injected so tests can stand at one exact second.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The real clock. A system clock set before 1970 reads as the epoch, which
/// makes every token expired rather than any of them live.
pub fn system_clock() -> Clock {
    Arc::new(|| {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
            });
        seconds_from_millis(millis)
    })
}

type Keys = Arc<Vec<Value>>;
/// A fetch that every concurrent reader shares, so callers arriving while it is
/// in flight wait on it rather than starting their own.
type PendingKeys = Shared<BoxFuture<'static, Result<Keys, JwksError>>>;

/// What a key lookup is to do once the cache lock is released.
enum Lookup {
    /// Await the cached (or in-flight) fetch.
    Reuse(CachedFetch),
    /// Answer with an empty key set without asking anyone.
    NoKeys,
    /// Await a fetch this caller started.
    Fetch(CachedFetch),
}

/// A fetch and the generation that started it. A waiter that sees it fail
/// forgets it by generation, so it can never forget a newer fetch.
#[derive(Clone)]
struct CachedFetch {
    generation: u64,
    pending: PendingKeys,
}

#[derive(Default)]
struct CacheState {
    cached: Option<CachedFetch>,
    /// The generation the next fetch will carry.
    next_generation: u64,
    last_refresh_attempt: i64,
    /// Consecutive failed fetches, and when the last of them was. Kept apart
    /// from `last_refresh_attempt` because they bound different things: that
    /// one bounds refetching over a healthy cache, these bound fetching at all
    /// when there is no cache to fall back on.
    failed_fetches: u32,
    last_failed_fetch: i64,
}

impl CacheState {
    /// True while a JWKS that has failed twice running is to be left alone.
    /// It lasts one window from the last failed attempt, so the next lookup
    /// after that finds out whether Privy recovered.
    fn cooling_down(&self, now: i64) -> bool {
        self.failed_fetches >= FAILURES_BEFORE_COOLDOWN
            && now - self.last_failed_fetch < REFRESH_GUARD_SECONDS
    }
}

/// Reads Privy identity tokens for one app. One instance per process: the
/// JWKS cache lives inside it.
pub struct PrivyVerifier {
    app_id: String,
    jwks_url: String,
    fetcher: Arc<dyn JwksFetch>,
    clock: Clock,
    state: Mutex<CacheState>,
}

impl fmt::Debug for PrivyVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PrivyVerifier")
            .field("app_id", &self.app_id)
            .finish_non_exhaustive()
    }
}

impl PrivyVerifier {
    pub fn new(app_id: String, fetcher: Arc<dyn JwksFetch>, clock: Clock) -> Self {
        Self {
            jwks_url: jwks_url(&app_id),
            app_id,
            fetcher,
            clock,
            state: Mutex::new(CacheState::default()),
        }
    }

    /// Reads the app id from `NEXT_PUBLIC_PRIVY_APP_ID`.
    pub fn from_env<F>(
        env: F,
        fetcher: Arc<dyn JwksFetch>,
        clock: Clock,
    ) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        Ok(Self::new(privy_app_id(env)?, fetcher, clock))
    }

    /// The identity Privy vouches for in `token`, or `None` for an absent,
    /// malformed, forged, foreign or expired one - an ordinary state that falls
    /// back to the longer signup path. An unreachable JWKS is `None` too.
    pub async fn read_identity(&self, token: Option<&str>) -> Option<PrivyIdentity> {
        let token = token.filter(|token| !token.is_empty())?;
        let unverified = privy::parse(token)?;
        let jwk = self.resolve_signing_key(unverified.kid()).await.ok()??;
        unverified.verified_identity(&jwk, &self.app_id, self.now())
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn state(&self) -> MutexGuard<'_, CacheState> {
        // The state is a handful of integers and a cache handle, each written
        // whole, so a panic elsewhere cannot leave it half-updated.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Resolves the key `kid` names, refetching the JWKS once (guarded) when
    /// the cached set doesn't have it - a rotation might have landed since the
    /// cache was built. A header naming no kid never benefits from a refetch,
    /// since nothing about a fresh fetch resolves which of several keys an
    /// absent kid means, so it goes straight to the single-key fallback.
    async fn resolve_signing_key(&self, kid: &KeyId) -> Result<Option<Value>, JwksError> {
        let keys = self.signing_keys().await?;
        if !kid.is_named() {
            return Ok(privy::match_key(&keys, kid).cloned());
        }
        if let Some(named) = privy::named_key(&keys, kid) {
            return Ok(Some(named.clone()));
        }
        let keys = self.refresh_signing_keys().await?;
        Ok(privy::match_key(&keys, kid).cloned())
    }

    async fn signing_keys(&self) -> Result<Keys, JwksError> {
        let lookup = self.plan_lookup();
        self.run(lookup).await
    }

    /// Forces one refetch, at most once per `REFRESH_GUARD_SECONDS` however
    /// many lookups miss in the meantime. Called only after the cached set
    /// has failed to name the kid.
    async fn refresh_signing_keys(&self) -> Result<Keys, JwksError> {
        let lookup = self.plan_refresh();
        self.run(lookup).await
    }

    /// Decided under the lock, carried out after it is released: the lock is
    /// never held across a fetch.
    fn plan_lookup(&self) -> Lookup {
        let mut state = self.state();
        if let Some(cached) = &state.cached {
            return Lookup::Reuse(cached.clone());
        }
        // No key signs the token while cooling down, which is both true and
        // the answer, and spares the caller an error for an expected state.
        if state.cooling_down(self.now()) {
            return Lookup::NoKeys;
        }
        Lookup::Fetch(self.start_fetch(&mut state))
    }

    fn plan_refresh(&self) -> Lookup {
        let mut state = self.state();
        let now = self.now();
        let within_guard = now - state.last_refresh_attempt < REFRESH_GUARD_SECONDS;
        if let Some(cached) = state.cached.clone().filter(|_| within_guard) {
            return Lookup::Reuse(cached);
        }
        if state.cooling_down(now) {
            return state.cached.clone().map_or(Lookup::NoKeys, Lookup::Reuse);
        }
        state.last_refresh_attempt = now;
        Lookup::Fetch(self.start_fetch(&mut state))
    }

    async fn run(&self, lookup: Lookup) -> Result<Keys, JwksError> {
        match lookup {
            Lookup::Reuse(fetch) | Lookup::Fetch(fetch) => self.settle(fetch).await,
            Lookup::NoKeys => Ok(Arc::default()),
        }
    }

    fn start_fetch(&self, state: &mut CacheState) -> CachedFetch {
        let fetcher = Arc::clone(&self.fetcher);
        let url = self.jwks_url.clone();
        let pending = async move {
            let response = fetcher.fetch(url).await?;
            if !(200..300).contains(&response.status) {
                return Err(JwksError::Status(response.status));
            }
            Ok(Arc::new(privy::jwks_keys(&response.body)?))
        }
        .boxed()
        .shared();
        let fetch = CachedFetch {
            generation: state.next_generation,
            pending,
        };
        state.next_generation += 1;
        state.cached = Some(fetch.clone());
        fetch
    }

    /// Waits on a fetch, and on failure forgets it and counts it. Every
    /// waiter does this, not only the caller that started the fetch: that
    /// caller can be dropped (a client that disconnects) before the fetch
    /// fails, and the failure it never saw would stay cached for everyone.
    /// A failed fetch must not be remembered as an answer, or one bad minute
    /// breaks signup until the process is replaced; what is remembered
    /// instead is that it failed, which is what the cooldown reads.
    ///
    /// The first waiter to look clears the generation and counts the failure;
    /// the rest find it already gone, so one failed fetch counts once however
    /// many callers waited on it.
    async fn settle(&self, fetch: CachedFetch) -> Result<Keys, JwksError> {
        let outcome = fetch.pending.await;
        let mut state = self.state();
        let still_cached = state
            .cached
            .as_ref()
            .is_some_and(|cached| cached.generation == fetch.generation);
        match &outcome {
            Ok(_) if still_cached => state.failed_fetches = 0,
            Ok(_) => {}
            Err(_) if still_cached => {
                state.cached = None;
                state.failed_fetches = state.failed_fetches.saturating_add(1);
                state.last_failed_fetch = self.now();
            }
            Err(_) => {}
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    //! Every token here is minted in this process from a key pair generated in
    //! this process. A fixture private key checked into the repo would be a
    //! credential in the repo whatever it was minted for.

    use super::*;
    use std::sync::LazyLock;
    use std::sync::atomic::{AtomicI64, Ordering};

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use p256::ecdsa::signature::{Signer, Verifier};
    use p256::ecdsa::{Signature, SigningKey};
    use rand_core::{OsRng, TryRngCore};
    use serde_json::{Map, json};

    const APP_ID: &str = "test-privy-app-id";
    const SUBJECT: &str = "did:privy:cm2testsubject";
    const EMAIL: &str = "someone@example.com";
    const WALLET: &str = "0x1111111111111111111111111111111111111111";

    /// 2026-09-08T12:00:00Z. The verifier reads the clock after a token was
    /// minted, so an assertion about one exact second is otherwise a race.
    const FROZEN: i64 = 1_788_868_800;

    /// The `kid` is kept beside the key rather than inside it: the JWKS entry
    /// carries one and the key itself has no use for it.
    struct TestKey {
        kid: &'static str,
        signing: SigningKey,
    }

    impl TestKey {
        fn jwk(&self) -> Value {
            let point = self.signing.verifying_key().to_encoded_point(false);
            let coordinate =
                |bytes: Option<&p256::FieldBytes>| URL_SAFE_NO_PAD.encode(bytes.unwrap());
            json!({
                "kty": "EC",
                "crv": "P-256",
                "x": coordinate(point.x()),
                "y": coordinate(point.y()),
                "kid": self.kid,
            })
        }
    }

    fn mint_key(kid: &'static str) -> TestKey {
        loop {
            let mut secret = [0_u8; 32];
            OsRng.try_fill_bytes(&mut secret).unwrap();
            // Out-of-range scalars are astronomically rare; draw again.
            if let Ok(signing) = SigningKey::from_slice(&secret) {
                return TestKey { kid, signing };
            }
        }
    }

    static PUBLISHED: LazyLock<TestKey> = LazyLock::new(|| mint_key("published"));
    static ALSO_PUBLISHED: LazyLock<TestKey> = LazyLock::new(|| mint_key("also-published"));
    static ROTATED: LazyLock<TestKey> = LazyLock::new(|| mint_key("rotated"));

    /// What the fake endpoint answers with right now.
    #[derive(Clone)]
    enum Served {
        Json(Value),
        Status(u16),
    }

    /// Privy's JWKS from memory, counting every request it is asked.
    struct FakeJwks {
        served: Mutex<Served>,
        requests: AtomicI64,
        /// When set, the next fetch waits for it and then fails.
        gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    impl JwksFetch for FakeJwks {
        fn fetch(&self, url: String) -> BoxFuture<'static, Result<JwksResponse, JwksError>> {
            assert_eq!(url, jwks_url(APP_ID));
            self.requests.fetch_add(1, Ordering::SeqCst);
            let response = match self.served.lock().unwrap().clone() {
                Served::Json(body) => JwksResponse {
                    status: 200,
                    body: body.to_string().into_bytes(),
                },
                Served::Status(status) => JwksResponse {
                    status,
                    body: b"gateway is unhappy".to_vec(),
                },
            };
            let gate = self.gate.lock().unwrap().take();
            async move {
                if let Some(gate) = gate {
                    let _ = gate.await;
                    return Err(JwksError::Transport("connection reset".to_owned()));
                }
                Ok(response)
            }
            .boxed()
        }
    }

    /// A verifier with a JWKS cache of its own, so no test asserts against
    /// whichever key set another test happened to load first.
    struct Harness {
        verifier: PrivyVerifier,
        jwks: Arc<FakeJwks>,
        clock: Arc<AtomicI64>,
    }

    /// Header or claim edits: `None` removes the field, as `undefined` does
    /// when the TypeScript original serialised its token.
    type Edits<'a> = &'a [(&'a str, Option<Value>)];

    impl Harness {
        fn serving(served: Served) -> Self {
            let jwks = Arc::new(FakeJwks {
                served: Mutex::new(served),
                requests: AtomicI64::new(0),
                gate: Mutex::new(None),
            });
            let clock = Arc::new(AtomicI64::new(FROZEN));
            let reader = Arc::clone(&clock);
            let env = |name: &str| (name == "NEXT_PUBLIC_PRIVY_APP_ID").then(|| APP_ID.to_owned());
            let verifier = PrivyVerifier::from_env(
                env,
                Arc::clone(&jwks) as Arc<dyn JwksFetch>,
                Arc::new(move || reader.load(Ordering::SeqCst)),
            )
            .unwrap();
            Self {
                verifier,
                jwks,
                clock,
            }
        }

        fn new(keys: &[&TestKey]) -> Self {
            Self::serving(Served::Json(key_set(keys)))
        }

        fn serve(&self, served: Served) {
            *self.jwks.served.lock().unwrap() = served;
        }

        fn serve_keys(&self, keys: &[&TestKey]) {
            self.serve(Served::Json(key_set(keys)));
        }

        /// Makes the next JWKS fetch hang until the returned sender fires,
        /// then fail.
        fn hold_next_fetch(&self) -> tokio::sync::oneshot::Sender<()> {
            let (release, gate) = tokio::sync::oneshot::channel();
            *self.jwks.gate.lock().unwrap() = Some(gate);
            release
        }

        fn requests(&self) -> i64 {
            self.jwks.requests.load(Ordering::SeqCst)
        }

        fn now(&self) -> i64 {
            self.clock.load(Ordering::SeqCst)
        }

        fn set_now(&self, now: i64) {
            self.clock.store(now, Ordering::SeqCst);
        }

        async fn read(&self, token: &str) -> Option<PrivyIdentity> {
            self.verifier.read_identity(Some(token)).await
        }

        async fn user_id(&self, token: &str) -> Option<String> {
            self.read(token).await.map(|identity| identity.user_id)
        }

        /// The claims Privy actually sends, in the shape it sends them.
        fn honest_claims(&self) -> Map<String, Value> {
            let claims = json!({
                "iss": "privy.io",
                "aud": APP_ID,
                "sub": SUBJECT,
                "exp": self.now() + 600,
                "linked_accounts": json!([
                    { "type": "email", "address": EMAIL },
                    { "type": "wallet", "address": WALLET },
                ])
                .to_string(),
            });
            object(claims)
        }

        fn token(&self, key: &TestKey) -> String {
            self.token_with(key, &[], &[])
        }

        /// A token the verifier accepts, unless the edits change the one thing
        /// under test - which is what makes each rejection attributable.
        fn token_with(&self, key: &TestKey, header: Edits<'_>, claims: Edits<'_>) -> String {
            let head = encode(&edited(
                object(json!({ "alg": "ES256", "typ": "JWT", "kid": key.kid })),
                header,
            ));
            let payload = encode(&edited(self.honest_claims(), claims));
            let signing_input = format!("{head}.{payload}");
            let signature: Signature = key.signing.sign(signing_input.as_bytes());
            format!(
                "{signing_input}.{}",
                URL_SAFE_NO_PAD.encode(signature.to_bytes())
            )
        }
    }

    fn key_set(keys: &[&TestKey]) -> Value {
        json!({ "keys": keys.iter().map(|key| key.jwk()).collect::<Vec<_>>() })
    }

    fn object(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            other => panic!("not an object: {other}"),
        }
    }

    fn edited(mut fields: Map<String, Value>, edits: Edits<'_>) -> Value {
        for (name, value) in edits {
            match value {
                Some(value) => fields.insert((*name).to_owned(), value.clone()),
                None => fields.remove(*name),
            };
        }
        Value::Object(fields)
    }

    fn encode(value: &Value) -> String {
        URL_SAFE_NO_PAD.encode(value.to_string())
    }

    fn segments(token: &str) -> Vec<String> {
        token.split('.').map(str::to_owned).collect()
    }

    fn identity(email: Option<&str>, wallets: &[&str]) -> Option<PrivyIdentity> {
        Some(PrivyIdentity {
            user_id: SUBJECT.to_owned(),
            email: email.map(str::to_owned),
            wallets: wallets.iter().map(|wallet| (*wallet).to_owned()).collect(),
        })
    }

    /// Independent proof that a rejected token carries a signature that is
    /// good, so a negative test says which check refused it.
    fn signature_verifies(token: &str, key: &TestKey) -> bool {
        let parts = segments(token);
        let signature = URL_SAFE_NO_PAD.decode(&parts[2]).unwrap();
        let signature = Signature::from_slice(&signature).unwrap();
        key.signing
            .verifying_key()
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .is_ok()
    }

    #[tokio::test]
    async fn a_token_signed_by_the_key_privy_publishes_yields_the_identity_its_claims_name() {
        let h = Harness::new(&[&PUBLISHED]);
        assert_eq!(
            h.read(&h.token(&PUBLISHED)).await,
            identity(Some(EMAIL), &[WALLET])
        );
    }

    #[tokio::test]
    async fn an_email_and_a_wallet_are_lower_cased_so_they_compare_against_what_is_stored() {
        let h = Harness::new(&[&PUBLISHED]);
        let accounts = json!([
            { "type": "email", "address": "Someone@Example.COM" },
            { "type": "wallet", "address": "0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" },
        ]);
        let shouted = h.token_with(
            &PUBLISHED,
            &[],
            &[("linked_accounts", Some(json!(accounts.to_string())))],
        );
        assert_eq!(
            h.read(&shouted).await,
            identity(Some(EMAIL), &["0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"])
        );
    }

    #[tokio::test]
    async fn a_token_with_no_linked_accounts_yields_an_identity_with_neither_email_nor_wallet() {
        let h = Harness::new(&[&PUBLISHED]);
        let bare = h.token_with(&PUBLISHED, &[], &[("linked_accounts", None)]);
        assert_eq!(h.read(&bare).await, identity(None, &[]));
    }

    #[tokio::test]
    async fn linked_accounts_arriving_as_an_array_rather_than_a_string_is_read_the_same_way() {
        let h = Harness::new(&[&PUBLISHED]);
        let accounts = json!([
            { "type": "email", "address": EMAIL },
            { "type": "wallet", "address": WALLET },
        ]);
        let nested = h.token_with(&PUBLISHED, &[], &[("linked_accounts", Some(accounts))]);
        assert_eq!(h.read(&nested).await, identity(Some(EMAIL), &[WALLET]));
    }

    #[tokio::test]
    async fn a_linked_account_that_is_not_a_0x_address_is_left_out_of_the_wallets() {
        let h = Harness::new(&[&PUBLISHED]);
        let accounts = json!([
            { "type": "wallet", "address": "vitalik.eth" },
            { "type": "wallet" },
            { "type": "wallet", "address": WALLET },
        ]);
        let odd = h.token_with(
            &PUBLISHED,
            &[],
            &[("linked_accounts", Some(json!(accounts.to_string())))],
        );
        assert_eq!(
            h.read(&odd).await.map(|identity| identity.wallets),
            Some(vec![WALLET.to_owned()])
        );
    }

    /// The token is signed with the published key over its own header, so the
    /// signature genuinely verifies and only the alg check stands between it
    /// and an identity. Both facts are asserted before the rejection.
    #[tokio::test]
    async fn a_token_whose_header_claims_alg_none_is_rejected_though_its_signature_verifies() {
        let h = Harness::new(&[&PUBLISHED]);
        let forged = h.token_with(&PUBLISHED, &[("alg", Some(json!("none")))], &[]);

        assert!(
            signature_verifies(&forged, &PUBLISHED),
            "precondition: signature verifies"
        );
        assert!(
            h.read(&h.token(&PUBLISHED)).await.is_some(),
            "precondition: these claims are accepted when the header says ES256"
        );
        assert_eq!(h.read(&forged).await, None);
    }

    #[tokio::test]
    async fn a_token_whose_header_claims_hs256_is_rejected_though_it_is_signed_with_es256() {
        let h = Harness::new(&[&PUBLISHED]);
        let confused = h.token_with(&PUBLISHED, &[("alg", Some(json!("HS256")))], &[]);

        assert!(
            signature_verifies(&confused, &PUBLISHED),
            "precondition: signature verifies"
        );
        assert_eq!(h.read(&confused).await, None);
    }

    #[tokio::test]
    async fn an_aud_listing_this_app_among_others_is_accepted() {
        let h = Harness::new(&[&PUBLISHED]);
        let many = h.token_with(
            &PUBLISHED,
            &[],
            &[("aud", Some(json!(["some-other-app", APP_ID])))],
        );
        assert_eq!(h.user_id(&many).await.as_deref(), Some(SUBJECT));
    }

    #[tokio::test]
    async fn an_aud_array_naming_only_other_apps_is_rejected() {
        let h = Harness::new(&[&PUBLISHED]);
        let elsewhere = h.token_with(
            &PUBLISHED,
            &[],
            &[("aud", Some(json!(["some-other-app", "a-third-app"])))],
        );
        assert_eq!(h.read(&elsewhere).await, None);
    }

    #[tokio::test]
    async fn an_aud_naming_another_app_is_rejected() {
        let h = Harness::new(&[&PUBLISHED]);
        let other = h.token_with(&PUBLISHED, &[], &[("aud", Some(json!("some-other-app")))]);
        assert_eq!(h.read(&other).await, None);
    }

    #[tokio::test]
    async fn an_aud_that_is_neither_a_string_nor_an_array_is_rejected() {
        let h = Harness::new(&[&PUBLISHED]);
        let shaped = h.token_with(&PUBLISHED, &[], &[("aud", Some(json!({ "id": APP_ID })))]);
        assert_eq!(h.read(&shaped).await, None);
    }

    #[tokio::test]
    async fn a_token_from_another_issuer_is_rejected() {
        let h = Harness::new(&[&PUBLISHED]);
        let evil = h.token_with(&PUBLISHED, &[], &[("iss", Some(json!("evil.io")))]);
        assert_eq!(h.read(&evil).await, None);
    }

    #[tokio::test]
    async fn a_token_that_expired_a_second_ago_is_rejected() {
        let h = Harness::new(&[&PUBLISHED]);
        let expired = h.token_with(&PUBLISHED, &[], &[("exp", Some(json!(h.now() - 1)))]);
        assert_eq!(h.read(&expired).await, None);
    }

    #[tokio::test]
    async fn a_token_whose_exp_is_the_current_second_is_rejected_because_expiry_is_not_inclusive() {
        let h = Harness::new(&[&PUBLISHED]);
        let boundary = h.token_with(&PUBLISHED, &[], &[("exp", Some(json!(h.now())))]);
        assert_eq!(h.read(&boundary).await, None);
    }

    #[tokio::test]
    async fn a_token_whose_exp_is_one_second_away_is_still_live() {
        let h = Harness::new(&[&PUBLISHED]);
        let live = h.token_with(&PUBLISHED, &[], &[("exp", Some(json!(h.now() + 1)))]);
        assert_eq!(h.user_id(&live).await.as_deref(), Some(SUBJECT));
    }

    /// A string expiry decides everything if it is compared rather than
    /// typed, in one direction or the other.
    #[tokio::test]
    async fn a_token_whose_exp_is_a_string_rather_than_a_number_is_rejected() {
        let h = Harness::new(&[&PUBLISHED]);
        let stringly = h.token_with(
            &PUBLISHED,
            &[],
            &[("exp", Some(json!((h.now() + 600).to_string())))],
        );
        assert_eq!(h.read(&stringly).await, None);
    }

    #[tokio::test]
    async fn a_token_carrying_no_exp_at_all_is_rejected() {
        let h = Harness::new(&[&PUBLISHED]);
        let endless = h.token_with(&PUBLISHED, &[], &[("exp", None)]);
        assert_eq!(h.read(&endless).await, None);
    }

    #[tokio::test]
    async fn a_token_whose_sub_is_not_a_string_is_rejected() {
        let h = Harness::new(&[&PUBLISHED]);
        let numeric = h.token_with(&PUBLISHED, &[], &[("sub", Some(json!(12345)))]);
        assert_eq!(h.read(&numeric).await, None);
    }

    /// Privy rotates, and a token minted before a rotation names a kid the
    /// JWKS no longer lists. One published key means one answer to give, and
    /// the signature still has to verify against it.
    #[tokio::test]
    async fn a_jwks_holding_one_key_signs_for_a_kid_it_does_not_name() {
        let h = Harness::new(&[&ROTATED]);
        let stale = h.token_with(
            &ROTATED,
            &[("kid", Some(json!("a-kid-nobody-publishes")))],
            &[],
        );
        assert_eq!(h.user_id(&stale).await.as_deref(), Some(SUBJECT));
    }

    #[tokio::test]
    async fn a_header_carrying_no_kid_is_served_by_the_only_key_in_the_jwks() {
        let h = Harness::new(&[&ROTATED]);
        let unnamed = h.token_with(&ROTATED, &[("kid", None)], &[]);
        assert_eq!(h.user_id(&unnamed).await.as_deref(), Some(SUBJECT));
    }

    /// The fallback is a guess, and with two keys published there is nothing
    /// to guess from.
    #[tokio::test]
    async fn a_kid_that_matches_nothing_is_rejected_once_the_jwks_holds_more_than_one_key() {
        let h = Harness::new(&[&PUBLISHED, &ALSO_PUBLISHED]);
        let unnamed = h.token_with(
            &PUBLISHED,
            &[("kid", Some(json!("a-kid-nobody-publishes")))],
            &[],
        );

        assert!(
            signature_verifies(&unnamed, &PUBLISHED),
            "precondition: a published key signed this"
        );
        assert_eq!(h.read(&unnamed).await, None);
    }

    /// Naming a kid does not make the key that signed it the key it names.
    #[tokio::test]
    async fn a_token_signed_by_one_published_key_but_claiming_the_kid_of_another_is_rejected() {
        let h = Harness::new(&[&PUBLISHED, &ALSO_PUBLISHED]);
        let misnamed = h.token_with(&ALSO_PUBLISHED, &[("kid", Some(json!(PUBLISHED.kid)))], &[]);

        assert!(
            signature_verifies(&misnamed, &ALSO_PUBLISHED),
            "precondition: a real signature by a published key, just not the one it names"
        );
        assert_eq!(h.read(&misnamed).await, None);
    }

    #[tokio::test]
    async fn a_jwks_that_lists_no_keys_at_all_reads_no_token() {
        let h = Harness::serving(Served::Json(json!({})));
        assert_eq!(h.read(&h.token(&PUBLISHED)).await, None);
    }

    #[tokio::test]
    async fn a_payload_swapped_in_after_signing_is_rejected() {
        let h = Harness::new(&[&PUBLISHED]);
        let parts = segments(&h.token(&PUBLISHED));
        let mut claims = h.honest_claims();
        claims.insert("sub".to_owned(), json!("did:privy:someone-else"));
        let swapped = format!(
            "{}.{}.{}",
            parts[0],
            encode(&Value::Object(claims)),
            parts[2]
        );
        assert_eq!(h.read(&swapped).await, None);
    }

    #[tokio::test]
    async fn a_token_with_no_signature_segment_is_rejected() {
        let h = Harness::new(&[&PUBLISHED]);
        let parts = segments(&h.token(&PUBLISHED));
        assert_eq!(h.read(&format!("{}.{}", parts[0], parts[1])).await, None);
    }

    /// A genuine token with junk appended must not verify on the three
    /// segments that were actually signed.
    #[tokio::test]
    async fn a_token_with_segments_trailing_the_signature_is_rejected_not_truncated_to_the_first_three()
     {
        let h = Harness::new(&[&PUBLISHED]);
        let extended = format!("{}.junk.more", h.token(&PUBLISHED));
        assert_eq!(h.read(&extended).await, None);
    }

    #[tokio::test]
    async fn a_header_that_is_not_json_is_rejected_rather_than_thrown_out_of() {
        let h = Harness::new(&[&PUBLISHED]);
        let parts = segments(&h.token(&PUBLISHED));
        let garbled = format!(
            "{}.{}.{}",
            URL_SAFE_NO_PAD.encode("{not json"),
            parts[1],
            parts[2]
        );
        assert_eq!(h.read(&garbled).await, None);
    }

    /// A second parser reachable from the token, run after the signature
    /// check; either way it must not escape as a panic.
    #[tokio::test]
    async fn linked_accounts_that_is_not_parseable_json_is_rejected_rather_than_thrown_out_of() {
        let h = Harness::new(&[&PUBLISHED]);
        let garbled = h.token_with(
            &PUBLISHED,
            &[],
            &[("linked_accounts", Some(json!("[{unclosed")))],
        );
        assert_eq!(h.read(&garbled).await, None);
    }

    #[tokio::test]
    async fn an_absent_token_is_no_identity_and_asks_the_jwks_nothing() {
        let h = Harness::new(&[&PUBLISHED]);
        assert_eq!(h.verifier.read_identity(None).await, None);
        assert_eq!(h.verifier.read_identity(Some("")).await, None);
        assert_eq!(h.requests(), 0);
    }

    #[tokio::test]
    async fn the_jwks_is_fetched_once_and_reused_for_every_token_whose_kid_it_already_knows() {
        let h = Harness::new(&[&PUBLISHED]);
        h.read(&h.token(&PUBLISHED)).await;
        h.read(&h.token(&PUBLISHED)).await;
        assert_eq!(h.requests(), 1);
    }

    /// Privy rotates, and a token minted after the rotation names a kid the
    /// cache has never heard of; this has to go and look rather than guess.
    #[tokio::test]
    async fn a_kid_absent_from_the_cached_jwks_triggers_one_refetch_and_the_rotated_key_found_there_is_accepted()
     {
        let h = Harness::new(&[&PUBLISHED]);
        assert_eq!(
            h.user_id(&h.token(&PUBLISHED)).await.as_deref(),
            Some(SUBJECT)
        );
        assert_eq!(
            h.requests(),
            1,
            "precondition: the cache is primed with the pre-rotation key"
        );

        h.serve_keys(&[&ROTATED]);
        let after_rotation = h.token(&ROTATED);

        assert_eq!(h.user_id(&after_rotation).await.as_deref(), Some(SUBJECT));
        assert_eq!(
            h.requests(),
            2,
            "an unknown kid must cost exactly one refetch"
        );
    }

    #[tokio::test]
    async fn a_kid_that_matches_nothing_does_not_cause_unbounded_refetching() {
        let h = Harness::new(&[&PUBLISHED, &ALSO_PUBLISHED]);
        let bogus = h.token_with(&PUBLISHED, &[("kid", Some(json!("never-published")))], &[]);

        assert_eq!(h.read(&bogus).await, None);
        assert_eq!(
            h.requests(),
            2,
            "one guarded refetch on top of the initial fetch"
        );

        h.read(&bogus).await;
        h.read(&bogus).await;
        assert_eq!(
            h.requests(),
            2,
            "repeats within the guard window must not refetch again"
        );
    }

    /// A cooldown, not a lockout: a rotation could genuinely have landed.
    #[tokio::test]
    async fn the_refetch_guard_releases_once_its_window_has_passed() {
        let h = Harness::new(&[&PUBLISHED, &ALSO_PUBLISHED]);
        let bogus = h.token_with(&PUBLISHED, &[("kid", Some(json!("never-published")))], &[]);

        h.read(&bogus).await;
        assert_eq!(h.requests(), 2);

        h.set_now(FROZEN + 61);
        h.read(&bogus).await;
        assert_eq!(
            h.requests(),
            3,
            "the window has passed, so this read gets its own refetch"
        );
    }

    /// Remembered, one unreachable minute would reject every token for the
    /// life of the process.
    #[tokio::test]
    async fn a_jwks_that_failed_to_load_is_not_remembered_so_the_next_token_still_reads() {
        let h = Harness::serving(Served::Status(503));
        assert_eq!(h.read(&h.token(&PUBLISHED)).await, None);

        h.serve_keys(&[&PUBLISHED]);

        assert_eq!(
            h.user_id(&h.token(&PUBLISHED)).await.as_deref(),
            Some(SUBJECT)
        );
        assert_eq!(h.requests(), 2, "the second read has to have asked again");
    }

    /// An unguarded retry per inbound request turns our traffic into a
    /// down dependency's traffic.
    #[tokio::test]
    async fn a_jwks_that_keeps_failing_is_not_asked_again_once_per_read() {
        let h = Harness::serving(Served::Status(503));
        for _ in 0..25 {
            assert_eq!(h.read(&h.token(&PUBLISHED)).await, None);
        }
        assert_eq!(
            h.requests(),
            2,
            "one attempt and one retry, then the cooldown answers"
        );
    }

    /// The cooldown costs no identity while it holds, never the recovery.
    #[tokio::test]
    async fn a_jwks_that_recovers_is_read_again_as_soon_as_the_cooldown_lapses() {
        let h = Harness::serving(Served::Status(503));
        for _ in 0..3 {
            h.read(&h.token(&PUBLISHED)).await;
        }
        assert_eq!(h.requests(), 2, "precondition: the cooldown is in force");

        h.serve_keys(&[&PUBLISHED]);
        assert_eq!(
            h.read(&h.token(&PUBLISHED)).await,
            None,
            "no identity while it holds"
        );
        assert_eq!(h.requests(), 2, "and no request either");

        h.set_now(FROZEN + 61);
        assert_eq!(
            h.user_id(&h.token(&PUBLISHED)).await.as_deref(),
            Some(SUBJECT)
        );
        assert_eq!(
            h.requests(),
            3,
            "the window has passed, so one read finds it recovered"
        );
    }

    #[tokio::test]
    async fn concurrent_reads_share_one_jwks_fetch() {
        let h = Harness::new(&[&PUBLISHED]);
        let token = h.token(&PUBLISHED);
        let (first, second) = tokio::join!(h.read(&token), h.read(&token));
        assert!(first.is_some() && second.is_some());
        assert_eq!(h.requests(), 1);
    }

    #[tokio::test]
    async fn a_failed_fetch_whose_starting_request_was_dropped_is_not_reused() {
        let h = Arc::new(Harness::new(&[&PUBLISHED]));
        let token = h.token(&PUBLISHED);
        let release = h.hold_next_fetch();

        let starter = {
            let (h, token) = (Arc::clone(&h), token.clone());
            tokio::spawn(async move { h.read(&token).await })
        };
        while h.requests() == 0 {
            tokio::task::yield_now().await;
        }
        // The client goes away mid-fetch, and only then does the fetch fail.
        starter.abort();
        let _ = starter.await;
        release.send(()).unwrap();

        assert!(
            h.read(&token).await.is_none(),
            "the waiter that finds the fetch failed gets no key"
        );
        assert_eq!(
            h.user_id(&token).await.as_deref(),
            Some(SUBJECT),
            "the next read asks Privy again instead of reusing the failure"
        );
        assert_eq!(h.requests(), 2);
    }

    #[tokio::test]
    async fn one_failed_fetch_counts_once_however_many_callers_waited_on_it() {
        let h = Harness::new(&[&PUBLISHED]);
        let token = h.token(&PUBLISHED);
        let release = h.hold_next_fetch();

        // The release is polled last, so all three readers are already
        // waiting on the one fetch when it fails.
        let (first, second, third, ()) =
            tokio::join!(h.read(&token), h.read(&token), h.read(&token), async {
                release.send(()).unwrap()
            });

        assert!(first.is_none() && second.is_none() && third.is_none());
        assert_eq!(h.requests(), 1);
        assert_eq!(
            h.user_id(&token).await.as_deref(),
            Some(SUBJECT),
            "one failure is not yet a cooldown"
        );
    }

    #[test]
    fn the_jwks_request_bound_is_ten_seconds() {
        assert_eq!(JWKS_TIMEOUT, Duration::from_secs(10));
    }

    #[tokio::test]
    async fn the_http_fetcher_gives_up_on_a_server_that_never_answers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/jwks.json",
            axum::routing::get(std::future::pending::<&'static str>),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await });

        let started = std::time::Instant::now();
        let response = HttpJwks::with_timeout(Duration::from_millis(100))
            .fetch(format!("http://{address}/jwks.json"))
            .await;
        server.abort();

        assert!(
            matches!(response, Err(JwksError::Transport(_))),
            "{response:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_missing_app_id_fails_construction_and_names_the_variable() {
        let built =
            PrivyVerifier::from_env(|_| None, Arc::new(HttpJwks::default()), system_clock());
        assert_eq!(
            built.map(|_| ()),
            Err(ConfigError::Missing("NEXT_PUBLIC_PRIVY_APP_ID"))
        );
    }

    #[test]
    fn the_jwks_url_names_the_app() {
        assert_eq!(
            jwks_url("abc"),
            "https://auth.privy.io/api/v1/apps/abc/jwks.json"
        );
    }

    /// The production fetcher against a loopback server, so the status and
    /// body it hands back are the ones that were served.
    #[tokio::test]
    async fn the_http_fetcher_returns_the_status_and_body_it_was_served() {
        use axum::http::StatusCode;
        use axum::routing::get;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/jwks.json",
            get(|| async { (StatusCode::SERVICE_UNAVAILABLE, "down") }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await });

        let response = HttpJwks::default()
            .fetch(format!("http://{address}/jwks.json"))
            .await;
        server.abort();

        assert_eq!(
            response,
            Ok(JwksResponse {
                status: 503,
                body: b"down".to_vec(),
            })
        );
    }
}

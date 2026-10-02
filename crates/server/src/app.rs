//! Everything a route handler reaches for, built once per process.
//!
//! Nothing is opened or read when the state is made: each collaborator is
//! built from the environment the first time a request needs it, the way the
//! TypeScript read `process.env` at the call that used a value. So a missing
//! setting fails the requests that need it, not every request, and a cold
//! start pays only for what its first request touches.
//!
//! Tests hand in their own collaborators instead (a `TestDb`, clients pointed
//! at loopback stubs, a pinned clock) through [`AppStateBuilder`]; whatever a
//! test does not hand in is still built from the environment it gave, which
//! for a test is a fixed map rather than the process's.

use std::fmt;
use std::ops::Deref;
use std::sync::{Arc, OnceLock};

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use postage_core::secret::WalletNonceKey;
use tokio::sync::OnceCell;

use crate::chain::Chain;
use crate::classify::Classifier;
use crate::cloudflare::Cloudflare;
use crate::config::{ConfigError, Env, message_id_secret};
use crate::db::{Db, DbError};
use crate::graph::Graph;
use crate::hold::MailWorker;
use crate::mail::Mailer;
use crate::privy::{
    Clock, HttpJwks, JwksError, JwksFetch, JwksResponse, PrivyVerifier, system_clock,
};
use crate::world::WorldVerify;

/// A database handle shared by every request: the process's own, or a test's
/// `TestDb`, which owns the temporary directory the file lives in.
pub type SharedDb = Arc<dyn Deref<Target = Db> + Send + Sync>;

/// The state every handler is given. Cheap to clone: one shared allocation.
#[derive(Clone)]
pub struct AppState(Arc<Services>);

struct Services {
    env: Env,
    clock: Clock,
    /// Not remembered when opening fails, so one unreachable moment at cold
    /// start does not refuse every request for the life of the process.
    db: OnceCell<SharedDb>,
    chain: OnceLock<Result<Chain, ConfigError>>,
    graph: OnceLock<Graph>,
    mailer: OnceLock<Result<Mailer, ConfigError>>,
    mail_worker: OnceLock<MailWorker>,
    cloudflare: OnceLock<Cloudflare>,
    classifier: OnceLock<Classifier>,
    /// One per process: the JWKS cache lives inside it.
    privy: OnceLock<PrivyVerifier>,
    world: OnceLock<WorldVerify>,
}

impl fmt::Debug for AppState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppState")
            .field("env", &self.0.env)
            .finish_non_exhaustive()
    }
}

impl AppState {
    /// The production state: the process environment and the system clock.
    pub fn from_process() -> Self {
        Self::builder(Env::Process).build()
    }

    pub fn builder(env: Env) -> AppStateBuilder {
        AppStateBuilder {
            env,
            clock: None,
            db: None,
            chain: None,
            graph: None,
            mailer: None,
            mail_worker: None,
            cloudflare: None,
            classifier: None,
            privy: None,
            world: None,
        }
    }

    pub fn env(&self) -> &Env {
        &self.0.env
    }

    /// Epoch seconds, from the injected clock.
    pub fn now(&self) -> i64 {
        (self.0.clock)()
    }

    /// The key wallet nonces are minted and checked under, derived from
    /// `MESSAGE_ID_SECRET` each time it is asked for, as the TypeScript did.
    pub fn wallet_nonce_key(&self) -> Result<WalletNonceKey, ConfigError> {
        Ok(WalletNonceKey::derive(&message_id_secret(
            self.0.env.lookup(),
        )?)?)
    }

    /// The database, opened and brought up to the current schema on first use
    /// from `DATABASE_URL` and `DATABASE_AUTH_TOKEN`.
    pub async fn db(&self) -> Result<&Db, DbError> {
        let env = &self.0.env;
        let shared = self
            .0
            .db
            .get_or_try_init(|| async {
                let db = Db::open_from_env(env.lookup()).await?;
                Ok::<SharedDb, DbError>(Arc::new(Box::new(db)))
            })
            .await?;
        Ok(shared)
    }

    /// The chain client. Fails only on an `ARC_RPC_URL` that is not a URL.
    pub fn chain(&self) -> Result<&Chain, ConfigError> {
        self.0
            .chain
            .get_or_init(|| Chain::from_env(self.0.env.lookup()))
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The subgraph client; a missing URL or key is reported by the query
    /// that needs it.
    pub fn graph(&self) -> &Graph {
        self.0
            .graph
            .get_or_init(|| Graph::from_env(self.0.env.lookup()))
    }

    /// Resend. Missing settings fail the send, as `required()` did inside the
    /// TypeScript's `fetch`, with the same words.
    pub fn mailer(&self) -> Result<&Mailer, ConfigError> {
        self.0
            .mailer
            .get_or_init(|| Mailer::from_env(self.0.env.lookup()))
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The mail worker's release endpoint. Missing settings cost the release,
    /// never the request that opened the gate.
    pub fn mail_worker(&self) -> &MailWorker {
        self.0
            .mail_worker
            .get_or_init(|| MailWorker::from_env_or_unconfigured(self.0.env.lookup()))
    }

    /// Cloudflare's API. Missing settings fail the calls, not construction,
    /// so a path that never asks Cloudflare anything is not refused for them.
    pub fn cloudflare(&self) -> &Cloudflare {
        self.0
            .cloudflare
            .get_or_init(|| Cloudflare::from_env_or_unconfigured(self.0.env.lookup()))
    }

    /// World's Developer Portal verify endpoint. Needs no settings of its
    /// own: the relying party id is read per call, as the TypeScript did.
    pub fn world(&self) -> &WorldVerify {
        self.0.world.get_or_init(WorldVerify::default)
    }

    pub fn classifier(&self) -> &Classifier {
        self.0
            .classifier
            .get_or_init(|| Classifier::from_env(self.0.env.lookup()))
    }

    /// The Privy identity-token reader. Without `NEXT_PUBLIC_PRIVY_APP_ID` it
    /// vouches for nobody, which is what the TypeScript came to: the app id
    /// was first read inside the JWKS fetch, whose failure reads as no
    /// identity, so the signed-proof path still works.
    pub fn privy(&self) -> &PrivyVerifier {
        self.0.privy.get_or_init(|| {
            let clock = self.0.clock.clone();
            match PrivyVerifier::from_env(
                self.0.env.lookup(),
                Arc::new(HttpJwks::default()),
                clock.clone(),
            ) {
                Ok(verifier) => verifier,
                Err(missing) => {
                    PrivyVerifier::new(String::new(), Arc::new(UnconfiguredJwks(missing)), clock)
                }
            }
        })
    }
}

/// Stands in for the JWKS endpoint when there is no app to fetch keys for.
struct UnconfiguredJwks(ConfigError);

impl JwksFetch for UnconfiguredJwks {
    fn fetch(&self, _url: String) -> BoxFuture<'static, Result<JwksResponse, JwksError>> {
        let error = JwksError::Transport(self.0.to_string());
        async move { Err(error) }.boxed()
    }
}

/// Collaborators a test supplies; anything left unset is built from the
/// state's environment on first use, exactly as in production.
pub struct AppStateBuilder {
    env: Env,
    clock: Option<Clock>,
    db: Option<SharedDb>,
    chain: Option<Chain>,
    graph: Option<Graph>,
    mailer: Option<Mailer>,
    mail_worker: Option<MailWorker>,
    cloudflare: Option<Cloudflare>,
    classifier: Option<Classifier>,
    privy: Option<PrivyVerifier>,
    world: Option<WorldVerify>,
}

impl fmt::Debug for AppStateBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppStateBuilder")
            .field("env", &self.env)
            .finish_non_exhaustive()
    }
}

impl AppStateBuilder {
    pub fn clock(mut self, clock: Clock) -> Self {
        self.clock = Some(clock);
        self
    }

    pub fn db(mut self, db: SharedDb) -> Self {
        self.db = Some(db);
        self
    }

    pub fn chain(mut self, chain: Chain) -> Self {
        self.chain = Some(chain);
        self
    }

    pub fn graph(mut self, graph: Graph) -> Self {
        self.graph = Some(graph);
        self
    }

    pub fn mailer(mut self, mailer: Mailer) -> Self {
        self.mailer = Some(mailer);
        self
    }

    pub fn mail_worker(mut self, mail_worker: MailWorker) -> Self {
        self.mail_worker = Some(mail_worker);
        self
    }

    pub fn cloudflare(mut self, cloudflare: Cloudflare) -> Self {
        self.cloudflare = Some(cloudflare);
        self
    }

    pub fn classifier(mut self, classifier: Classifier) -> Self {
        self.classifier = Some(classifier);
        self
    }

    pub fn privy(mut self, privy: PrivyVerifier) -> Self {
        self.privy = Some(privy);
        self
    }

    pub fn world(mut self, world: WorldVerify) -> Self {
        self.world = Some(world);
        self
    }

    pub fn build(self) -> AppState {
        AppState(Arc::new(Services {
            env: self.env,
            clock: self.clock.unwrap_or_else(system_clock),
            db: OnceCell::new_with(self.db),
            chain: preset(self.chain.map(Ok)),
            graph: preset(self.graph),
            mailer: preset(self.mailer.map(Ok)),
            mail_worker: preset(self.mail_worker),
            cloudflare: preset(self.cloudflare),
            classifier: preset(self.classifier),
            privy: preset(self.privy),
            world: preset(self.world),
        }))
    }
}

fn preset<T>(value: Option<T>) -> OnceLock<T> {
    value.map_or_else(OnceLock::new, OnceLock::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::TestDb;

    #[tokio::test]
    async fn a_given_clock_is_the_one_now_reads() {
        let state = AppState::builder(Env::empty())
            .clock(Arc::new(|| 1_800_000_000))
            .build();
        assert_eq!(state.now(), 1_800_000_000);
    }

    #[tokio::test]
    async fn a_given_database_is_used_rather_than_one_opened_from_the_environment() {
        let test_db = Arc::new(TestDb::fresh().await);
        let state = AppState::builder(Env::fixed([("DATABASE_URL", "nonsense://x")]))
            .db(test_db.clone())
            .build();
        assert!(state.db().await.is_ok());
    }

    #[tokio::test]
    async fn a_database_that_cannot_be_opened_is_an_error_and_is_tried_again_next_time() {
        let state = AppState::builder(Env::fixed([("DATABASE_URL", "nonsense://x")])).build();
        assert!(state.db().await.is_err());
        assert!(state.db().await.is_err());
    }

    #[test]
    fn a_missing_mailer_setting_is_reported_in_the_words_the_typescript_used() {
        let state = AppState::builder(Env::empty()).build();
        assert_eq!(
            state.mailer().map(|_| ()).unwrap_err().to_string(),
            "RESEND_API_KEY is not set"
        );
    }

    #[tokio::test]
    async fn without_a_privy_app_id_no_identity_token_is_believed() {
        let state = AppState::builder(Env::empty()).build();
        let token = "eyJhbGciOiJFUzI1NiIsImtpZCI6ImsifQ.eyJzdWIiOiJ4In0.c2ln";
        assert!(state.privy().read_identity(Some(token)).await.is_none());
    }
}

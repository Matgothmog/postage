//! Proving a person with World ID. Replaces `web/src/lib/world-id.ts`: the
//! context fetch, the Selfie Check itself, and the verify call that clears a
//! challenge.
//!
//! What can be tested natively is behind `WorldPorts`: the signed-context
//! fetch, IDKit, and the verify call. `run_selfie_check` and `verify_human`
//! are written against it, so the order of events and every failure message
//! are checked against fakes; `BrowserWorld` is the real thing.
//!
//! Three things the TypeScript checked at run time are settled earlier here.
//! The environment (`NEXT_PUBLIC_WORLD_ENVIRONMENT`) is a compile-time
//! constant checked by `config`, so a near-miss never builds. The shape of the
//! signed context is a `Deserialize` impl (`bridge::idkit::RpContext`). The
//! IDKit error codes are an enum (`core::world_id_messages`).

use serde::Deserialize;
use serde_json::Value;

use crate::api::UNREACHABLE;
use crate::bridge::BridgeError;
use crate::bridge::idkit::{
    self, RpContext, SelfieCheckCompletion, SelfieCheckHandle, SelfieCheckRequest,
};
use crate::challenge_api::IdentityMode;
use crate::config::{self, WorldEnvironment, is_world_app_id};
use crate::http::{self, HttpError, HttpRequest, HttpResponse};

pub const WORLD_CONTEXT_PATH: &str = "/api/world/context";
pub const WORLD_VERIFY_PATH: &str = "/api/world/verify";

const NOT_CONFIGURED: &str = "World ID isn't configured yet. Pay instead, or try again shortly.";
const CONTEXT_UNREACHABLE: &str = "Could not reach World ID. Check your connection and try again.";
const CONTEXT_UNREADABLE: &str = "World ID sent back something we didn't understand. Try again.";
const COULD_NOT_START: &str = "Could not start World ID verification. Try again.";
const VERIFICATION_FAILED: &str = "Verification failed";

/// The client-side World ID settings: the build's app id and environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldApp {
    /// `None` in a debug build that was not given one.
    pub app_id: Option<&'static str>,
    pub environment: WorldEnvironment,
}

impl Default for WorldApp {
    fn default() -> Self {
        Self {
            app_id: config::WORLD_APP_ID,
            environment: config::WORLD_ENVIRONMENT,
        }
    }
}

/// A refusal of the signed-context request, in the words written for the
/// sender. `/api/world/context` answers every refusal (settled, dangerous,
/// unknown, rate-limited) with a message meant to be read, so it is relayed
/// instead of being re-worded here where it could drift from the route's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RpContextError(pub String);

/// Reads the answer to `POST /api/world/context`. Anything with nothing to
/// relay (offline, an error page from in front of the app) gets the generic
/// "could not reach" line. A 200 that is not a whole context is refused, not
/// trusted: a proxy or captive portal can answer 200 with a body of its own,
/// and a context missing its timestamps must never reach the poll window.
pub fn rp_context_outcome(
    answer: Result<HttpResponse, HttpError>,
) -> Result<RpContext, RpContextError> {
    let response = answer.map_err(|_| RpContextError(CONTEXT_UNREACHABLE.to_owned()))?;
    if !response.is_ok() {
        return Err(RpContextError(
            crate::api::error_text(&response.body)
                .unwrap_or_else(|| CONTEXT_UNREACHABLE.to_owned()),
        ));
    }
    RpContext::from_json(&response.body).map_err(|_| RpContextError(CONTEXT_UNREADABLE.to_owned()))
}

/// `POST /api/world/context`: the context every World ID 4.0-shaped proof
/// request must carry, signed server-side so the key never reaches the
/// browser. Posts the challenge token, which is how the route decides whether
/// this caller may have one at all.
pub async fn fetch_rp_context(token: &str) -> Result<RpContext, RpContextError> {
    let body = serde_json::json!({ "token": token }).to_string();
    rp_context_outcome(http::send(&HttpRequest::post_json(WORLD_CONTEXT_PATH, body), None).await)
}

/// The body `/api/world/verify` expects: a bare token under mock mode, the raw
/// IDKit result alongside it under live mode. The route reads World's own
/// field names, so `proof` is forwarded untouched.
pub fn verify_request_body(token: &str, proof: Option<&Value>) -> String {
    match proof {
        Some(proof) => serde_json::json!({ "token": token, "proof": proof }),
        None => serde_json::json!({ "token": token }),
    }
    .to_string()
}

/// Turns the verify answer into "was the held message delivered" or the
/// server's words for why not. Only `status: "cleared"` counts, whatever the
/// HTTP status.
pub fn verify_outcome(answer: Result<HttpResponse, HttpError>) -> Result<bool, String> {
    #[derive(Deserialize)]
    struct Reply {
        status: Option<String>,
        delivered: Option<bool>,
        error: Option<String>,
    }
    let response = answer.map_err(|_| UNREACHABLE.to_owned())?;
    let reply = serde_json::from_str::<Reply>(&response.body).ok();
    match reply {
        Some(reply) if reply.status.as_deref() == Some("cleared") => {
            Ok(reply.delivered == Some(true))
        }
        Some(Reply {
            error: Some(error), ..
        }) => Err(error),
        _ => Err(VERIFICATION_FAILED.to_owned()),
    }
}

/// `POST /api/world/verify`.
pub async fn post_world_verify(token: &str, proof: Option<&Value>) -> Result<bool, String> {
    let request = HttpRequest::post_json(WORLD_VERIFY_PATH, verify_request_body(token, proof));
    verify_outcome(http::send(&request, None).await)
}

/// A Selfie Check waiting on the World App.
#[allow(async_fn_in_trait)] // single-threaded wasm: the futures need not be `Send`
pub trait SelfieCheckSession {
    /// The link (and QR payload) that opens this request in World App.
    fn connector_uri(&self) -> &str;
    /// Waits for the World App's answer. Never fails.
    async fn poll_until_completion(&self, timeout_ms: u64) -> SelfieCheckCompletion;
}

impl SelfieCheckSession for SelfieCheckHandle {
    fn connector_uri(&self) -> &str {
        SelfieCheckHandle::connector_uri(self)
    }

    async fn poll_until_completion(&self, timeout_ms: u64) -> SelfieCheckCompletion {
        SelfieCheckHandle::poll_until_completion(self, timeout_ms).await
    }
}

/// What the World ID path reaches outside for.
#[allow(async_fn_in_trait)] // single-threaded wasm: the futures need not be `Send`
pub trait WorldPorts {
    type Session: SelfieCheckSession;

    async fn fetch_rp_context(&self, token: &str) -> Result<RpContext, RpContextError>;
    async fn open_selfie_check(
        &self,
        request: &SelfieCheckRequest<'_>,
    ) -> Result<Self::Session, BridgeError>;
    /// Posts to `/api/world/verify`; `Ok` says whether the held message went
    /// out on its own.
    async fn post_verify(&self, token: &str, proof: Option<&Value>) -> Result<bool, String>;
}

/// The real thing: the API, and IDKit through the bridge.
#[derive(Debug, Clone, Copy, Default)]
pub struct BrowserWorld;

impl WorldPorts for BrowserWorld {
    type Session = SelfieCheckHandle;

    async fn fetch_rp_context(&self, token: &str) -> Result<RpContext, RpContextError> {
        fetch_rp_context(token).await
    }

    async fn open_selfie_check(
        &self,
        request: &SelfieCheckRequest<'_>,
    ) -> Result<SelfieCheckHandle, BridgeError> {
        idkit::open_selfie_check(request).await
    }

    async fn post_verify(&self, token: &str, proof: Option<&Value>) -> Result<bool, String> {
        post_world_verify(token, proof).await
    }
}

/// Everything between "the sender clicked the button" and "we have a proof or
/// we do not". Every failure resolves to the message to show; nothing here
/// panics or leaves the caller guessing which failure it was.
///
/// `signal` is the challenge token. It binds the proof to one challenge: left
/// empty, the proof says only "a person did this" and would clear whatever
/// challenge it is pasted into, so an unbound check is refused before any
/// network call is made.
///
/// `on_connector_ready` fires once the request can be answered, so the caller
/// can show the World App link while the poll below is still waiting on it.
/// Nothing here navigates: the poll that finishes the verification runs on
/// this page, and a redirect would unload the very thing waiting for the
/// answer.
pub async fn run_selfie_check<P: WorldPorts>(
    ports: &P,
    app: &WorldApp,
    signal: &str,
    on_connector_ready: impl FnOnce(&str),
) -> Result<Value, String> {
    let Some(app_id) = app.app_id.filter(|id| is_world_app_id(id)) else {
        leptos::logging::error!("Selfie Check cannot start: no valid World app id in this build");
        return Err(NOT_CONFIGURED.to_owned());
    };
    if signal.is_empty() {
        leptos::logging::error!("Selfie Check cannot start: it needs a signal to bind to");
        return Err(NOT_CONFIGURED.to_owned());
    }

    let rp_context = ports.fetch_rp_context(signal).await.map_err(|error| {
        leptos::logging::error!("failed to fetch rp_context for a live Selfie Check: {error}");
        error.0
    })?;

    let session = ports
        .open_selfie_check(&SelfieCheckRequest {
            app_id,
            environment: app.environment,
            rp_context: &rp_context,
            signal,
        })
        .await
        .map_err(|error| {
            leptos::logging::error!("IDKit rejected opening a Selfie Check request: {error}");
            COULD_NOT_START.to_owned()
        })?;

    on_connector_ready(session.connector_uri());

    match session
        .poll_until_completion(rp_context.poll_timeout_ms())
        .await
    {
        SelfieCheckCompletion::Verified(proof) => Ok(proof),
        SelfieCheckCompletion::Failed(failure) => Err(failure.message().to_owned()),
    }
}

/// The "I'm human" path, wallet-free. Under mock mode this posts a bare token;
/// under live mode it gets a Selfie Check proof first and forwards it. Resolves
/// to whether the held message went out on its own.
pub async fn verify_human<P: WorldPorts>(
    ports: &P,
    mode: IdentityMode,
    app: &WorldApp,
    token: &str,
    on_connector_ready: impl FnOnce(&str),
) -> Result<bool, String> {
    match mode {
        IdentityMode::Mock => ports.post_verify(token, None).await,
        IdentityMode::Live => {
            let proof = run_selfie_check(ports, app, token, on_connector_ready).await?;
            ports.post_verify(token, Some(&proof)).await
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use futures::executor::block_on;
    use postage_core::rp_context::{RpContextWindow, poll_timeout_ms};
    use postage_core::world_id_messages::{
        GENERIC_WORLD_ID_FAILURE_MESSAGE, IdKitErrorCode, describe_world_id_failure,
    };
    use serde_json::json;

    use super::*;
    use crate::bridge::idkit::IdKitFailure;

    const TOKEN: &str = "challenge-token";
    const CONNECTOR: &str = "https://worldcoin.org/verify/abc";

    fn proof() -> Value {
        json!({
            "protocol_version": "3.0",
            "nonce": "test-nonce",
            "environment": "production",
            "responses": [{
                "identifier": "selfie", "proof": "0xproof",
                "merkle_root": "0xroot", "nullifier": "world-nullifier",
            }],
        })
    }

    fn context() -> RpContext {
        RpContext {
            rp_id: "app_test_rp_id".to_owned(),
            nonce: "ctx-nonce".to_owned(),
            created_at: 1_000,
            expires_at: 1_300,
            signature: "0xsig".to_owned(),
            action: "send-free".to_owned(),
        }
    }

    fn app() -> WorldApp {
        WorldApp {
            app_id: Some("app_test"),
            environment: WorldEnvironment::Production,
        }
    }

    fn ok(status: u16, body: Value) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse {
            status,
            body: body.to_string(),
        })
    }

    fn not_json(status: u16) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse {
            status,
            body: "not json".to_owned(),
        })
    }

    // The fake: records what crossed each port, and scripts each answer.

    struct FakeSession {
        completion: SelfieCheckCompletion,
        polled_with: Rc<RefCell<Option<u64>>>,
    }

    impl SelfieCheckSession for FakeSession {
        fn connector_uri(&self) -> &str {
            CONNECTOR
        }

        async fn poll_until_completion(&self, timeout_ms: u64) -> SelfieCheckCompletion {
            *self.polled_with.borrow_mut() = Some(timeout_ms);
            self.completion.clone()
        }
    }

    struct Fake {
        context: Result<RpContext, RpContextError>,
        open: Result<SelfieCheckCompletion, BridgeError>,
        verify: Result<bool, String>,
        events: RefCell<Vec<String>>,
        polled_with: Rc<RefCell<Option<u64>>>,
        requests: RefCell<Vec<(String, String, WorldEnvironment, String)>>,
        posted: RefCell<Vec<(String, Option<Value>)>>,
    }

    impl Fake {
        fn succeeding() -> Self {
            Self {
                context: Ok(context()),
                open: Ok(SelfieCheckCompletion::Verified(proof())),
                verify: Ok(true),
                events: RefCell::default(),
                polled_with: Rc::default(),
                requests: RefCell::default(),
                posted: RefCell::default(),
            }
        }

        fn events(&self) -> Vec<String> {
            self.events.borrow().clone()
        }
    }

    impl WorldPorts for Fake {
        type Session = FakeSession;

        async fn fetch_rp_context(&self, token: &str) -> Result<RpContext, RpContextError> {
            self.events.borrow_mut().push(format!("context:{token}"));
            self.context.clone()
        }

        async fn open_selfie_check(
            &self,
            request: &SelfieCheckRequest<'_>,
        ) -> Result<FakeSession, BridgeError> {
            self.events.borrow_mut().push("open".to_owned());
            self.requests.borrow_mut().push((
                request.signal.to_owned(),
                request.rp_context.action.clone(),
                request.environment,
                request.app_id.to_owned(),
            ));
            self.open.clone().map(|completion| FakeSession {
                completion,
                polled_with: Rc::clone(&self.polled_with),
            })
        }

        async fn post_verify(&self, token: &str, proof: Option<&Value>) -> Result<bool, String> {
            self.events.borrow_mut().push("verify".to_owned());
            self.posted
                .borrow_mut()
                .push((token.to_owned(), proof.cloned()));
            self.verify.clone()
        }
    }

    fn run(fake: &Fake, app: &WorldApp, signal: &str) -> Result<Value, String> {
        block_on(run_selfie_check(fake, app, signal, |_| {}))
    }

    // Ported from `world-id.test.ts`: the request body and the verify call.

    #[test]
    fn verify_request_body_omits_proof_entirely_under_mock_mode() {
        let body: Value = serde_json::from_str(&verify_request_body("tok", None)).unwrap();
        assert_eq!(body, json!({"token": "tok"}));
    }

    #[test]
    fn verify_request_body_carries_the_proof_through_untouched_under_live_mode() {
        let body: Value =
            serde_json::from_str(&verify_request_body("tok", Some(&proof()))).unwrap();
        assert_eq!(body, json!({"token": "tok", "proof": proof()}));
    }

    #[test]
    fn a_cleared_verify_reports_whether_the_message_was_delivered() {
        assert_eq!(
            verify_outcome(ok(200, json!({"status": "cleared", "delivered": true}))),
            Ok(true)
        );
        assert_eq!(
            verify_outcome(ok(200, json!({"status": "cleared", "delivered": false}))),
            Ok(false)
        );
        assert_eq!(
            verify_outcome(ok(200, json!({"status": "cleared"}))),
            Ok(false),
            "delivered counts only when it is exactly true"
        );
    }

    #[test]
    fn verify_relays_the_servers_message_when_the_status_is_not_cleared() {
        let rejected = ok(
            200,
            json!({"status": "rejected", "error": "World ID rejected the proof"}),
        );
        assert_eq!(
            verify_outcome(rejected),
            Err("World ID rejected the proof".to_owned())
        );
        assert_eq!(
            verify_outcome(ok(
                403,
                json!({"error": "This challenge cannot be verified"})
            )),
            Err("This challenge cannot be verified".to_owned()),
            "a refusal's status is irrelevant; its words are not"
        );
    }

    #[test]
    fn verify_fails_with_a_plain_message_on_a_non_json_body() {
        assert_eq!(
            verify_outcome(not_json(200)),
            Err("Verification failed".to_owned())
        );
        assert_eq!(
            verify_outcome(ok(500, json!({}))),
            Err("Verification failed".to_owned())
        );
    }

    #[test]
    fn verify_says_so_when_the_server_cannot_be_reached() {
        let offline = Err(HttpError::Network("offline".to_owned()));
        assert_eq!(verify_outcome(offline), Err(UNREACHABLE.to_owned()));
    }

    // The context fetch.

    #[test]
    fn a_good_context_response_is_parsed() {
        let body = json!({
            "rp_id": "app_test_rp_id", "nonce": "ctx-nonce", "created_at": 1000,
            "expires_at": 1300, "signature": "0xsig", "action": "send-free",
        });
        assert_eq!(rp_context_outcome(ok(200, body)), Ok(context()));
    }

    #[test]
    fn a_non_json_error_response_gives_the_generic_message_not_a_broken_context() {
        assert_eq!(
            rp_context_outcome(not_json(500)),
            Err(RpContextError(CONTEXT_UNREACHABLE.to_owned()))
        );
        let offline = Err(HttpError::Network("offline".to_owned()));
        assert_eq!(
            rp_context_outcome(offline),
            Err(RpContextError(CONTEXT_UNREACHABLE.to_owned()))
        );
    }

    #[test]
    fn the_routes_own_refusals_are_relayed_for_dangerous_and_rate_limited_challenges() {
        let dangerous = ok(403, json!({"error": "This challenge cannot be verified"}));
        assert_eq!(
            rp_context_outcome(dangerous),
            Err(RpContextError(
                "This challenge cannot be verified".to_owned()
            ))
        );
        let limited = ok(
            429,
            json!({"error": "Too many verification attempts. Wait a moment and try again."}),
        );
        assert_eq!(
            rp_context_outcome(limited),
            Err(RpContextError(
                "Too many verification attempts. Wait a moment and try again.".to_owned()
            ))
        );
    }

    #[test]
    fn a_200_missing_its_timestamps_is_refused_rather_than_handed_back() {
        // What a proxy, CDN interstitial or captive portal could send instead.
        let body = json!({"rp_id": "app_test_rp_id", "nonce": "n", "signature": "0xsig", "action": "send-free"});
        assert_eq!(
            rp_context_outcome(ok(200, body)),
            Err(RpContextError(CONTEXT_UNREADABLE.to_owned()))
        );
    }

    #[test]
    fn a_200_that_is_not_a_context_at_all_is_refused() {
        assert_eq!(
            rp_context_outcome(ok(200, json!({"ok": true}))),
            Err(RpContextError(CONTEXT_UNREADABLE.to_owned()))
        );
    }

    // Failure copy: the table lives in core and is tested there; this pins
    // that the failures a sender can hit reach the screen as that copy.

    #[test]
    fn the_sender_stopping_and_world_saying_no_read_differently() {
        assert_ne!(
            describe_world_id_failure("user_rejected"),
            describe_world_id_failure("verification_rejected")
        );
    }

    #[test]
    fn an_uncurated_code_falls_back_to_generic_copy() {
        let message = describe_world_id_failure("generic_error");
        assert_eq!(message, GENERIC_WORLD_ID_FAILURE_MESSAGE);
        assert!(message.to_lowercase().contains("verification failed"));
    }

    #[test]
    fn a_verification_limit_rejection_does_not_invite_a_retry() {
        let message = describe_world_id_failure("max_verifications_reached");
        assert!(message.contains("already been used"));
        assert!(!message.to_lowercase().contains("try again"));
        assert!(message.to_lowercase().contains("pay instead"));
    }

    // `run_selfie_check`.

    #[test]
    fn without_an_app_id_it_reports_a_config_error_and_never_touches_the_network() {
        let fake = Fake::succeeding();
        let unconfigured = WorldApp {
            app_id: None,
            ..app()
        };

        assert_eq!(
            run(&fake, &unconfigured, TOKEN),
            Err(NOT_CONFIGURED.to_owned())
        );
        assert!(fake.events().is_empty());
    }

    #[test]
    fn a_malformed_app_id_is_a_config_error_too() {
        let fake = Fake::succeeding();
        let malformed = WorldApp {
            app_id: Some("123"),
            ..app()
        };

        assert_eq!(
            run(&fake, &malformed, TOKEN),
            Err(NOT_CONFIGURED.to_owned())
        );
        assert!(fake.events().is_empty());
    }

    #[test]
    fn it_fetches_the_context_with_the_same_token_it_holds_as_the_signal() {
        let fake = Fake::succeeding();

        run(&fake, &app(), TOKEN).unwrap();

        assert_eq!(fake.events()[0], format!("context:{TOKEN}"));
        assert_eq!(fake.requests.borrow()[0].0, TOKEN);
    }

    #[test]
    fn an_unreachable_context_endpoint_says_so() {
        let mut fake = Fake::succeeding();
        fake.context = Err(RpContextError(CONTEXT_UNREACHABLE.to_owned()));

        assert_eq!(
            run(&fake, &app(), TOKEN),
            Err("Could not reach World ID. Check your connection and try again.".to_owned())
        );
        assert_eq!(fake.events(), [format!("context:{TOKEN}")]);
    }

    #[test]
    fn the_context_endpoints_own_refusal_is_shown_for_a_settled_or_dangerous_challenge() {
        for refusal in [
            "This challenge has already been answered",
            "This challenge cannot be verified",
        ] {
            let mut fake = Fake::succeeding();
            fake.context = Err(RpContextError(refusal.to_owned()));

            assert_eq!(run(&fake, &app(), TOKEN), Err(refusal.to_owned()));
        }
    }

    #[test]
    fn idkit_refusing_to_open_the_request_is_reported() {
        let mut fake = Fake::succeeding();
        fake.open = Err(BridgeError::Sdk {
            code: "malformed_request".to_owned(),
            message: "malformed_request".to_owned(),
        });

        assert_eq!(
            run(&fake, &app(), TOKEN),
            Err("Could not start World ID verification. Try again.".to_owned())
        );
    }

    #[test]
    fn the_sender_dismissing_world_app_is_a_failure_message_after_the_link_was_shown() {
        let mut fake = Fake::succeeding();
        fake.open = Ok(SelfieCheckCompletion::Failed(IdKitFailure::Known(
            IdKitErrorCode::UserRejected,
        )));
        let announced = RefCell::new(None::<String>);

        let outcome = block_on(run_selfie_check(&fake, &app(), TOKEN, |uri| {
            *announced.borrow_mut() = Some(uri.to_owned());
        }));

        assert_eq!(
            outcome,
            Err(describe_world_id_failure("user_rejected").to_owned())
        );
        // The link must have been shown before the sender had a chance to
        // dismiss it.
        assert_eq!(announced.into_inner().as_deref(), Some(CONNECTOR));
    }

    #[test]
    fn an_unexpected_poll_failure_resolves_to_a_result_instead_of_panicking() {
        // The bridge turns a rejected poll into `generic_error` (tested in
        // `bridge_wasm`); here that arrives as an ordinary failure message.
        let mut fake = Fake::succeeding();
        fake.open = Ok(SelfieCheckCompletion::Failed(IdKitFailure::Known(
            IdKitErrorCode::GenericError,
        )));

        assert_eq!(
            run(&fake, &app(), TOKEN),
            Err(GENERIC_WORLD_ID_FAILURE_MESSAGE.to_owned())
        );
    }

    #[test]
    fn success_resolves_the_real_idkit_proof() {
        assert_eq!(run(&Fake::succeeding(), &app(), TOKEN), Ok(proof()));
    }

    #[test]
    fn the_action_comes_from_the_signed_context_not_the_caller() {
        let mut fake = Fake::succeeding();
        let mut custom = context();
        custom.action = "context-supplied-action".to_owned();
        fake.context = Ok(custom);

        run(&fake, &app(), TOKEN).unwrap();

        assert_eq!(fake.requests.borrow()[0].1, "context-supplied-action");
    }

    #[test]
    fn the_poll_timeout_is_derived_from_the_signed_contexts_own_window() {
        let fake = Fake::succeeding();

        run(&fake, &app(), TOKEN).unwrap();

        let window = RpContextWindow::new(context().created_at, context().expires_at);
        assert_eq!(*fake.polled_with.borrow(), Some(poll_timeout_ms(&window)));
        assert_eq!(poll_timeout_ms(&window), 270_000);
    }

    #[test]
    fn an_unbound_check_is_refused_before_any_network_call_and_no_proof_exists() {
        let fake = Fake::succeeding();

        let outcome = run(&fake, &app(), "");

        assert!(outcome.is_err());
        // The point is that no proof exists to be replayed, not merely that
        // the server would have refused one.
        assert!(fake.events().is_empty());
    }

    #[test]
    fn the_environment_reaches_idkit_as_configured() {
        for environment in [
            WorldEnvironment::Production,
            WorldEnvironment::Staging,
            WorldEnvironment::Sandbox,
        ] {
            let fake = Fake::succeeding();
            let configured = WorldApp {
                environment,
                ..app()
            };

            run(&fake, &configured, TOKEN).unwrap();

            assert_eq!(fake.requests.borrow()[0].2, environment);
        }
    }

    // `verify_human`: the order of events behind the button.

    fn human(fake: &Fake, mode: IdentityMode) -> Result<bool, String> {
        block_on(verify_human(fake, mode, &app(), TOKEN, |_| {}))
    }

    #[test]
    fn mock_mode_posts_a_bare_token_with_no_proof_and_never_opens_idkit() {
        let fake = Fake::succeeding();

        assert_eq!(human(&fake, IdentityMode::Mock), Ok(true));

        assert_eq!(fake.events(), ["verify"]);
        assert_eq!(*fake.posted.borrow(), [(TOKEN.to_owned(), None)]);
    }

    #[test]
    fn live_mode_runs_the_selfie_check_before_posting_and_forwards_its_proof() {
        let fake = Fake::succeeding();

        assert_eq!(human(&fake, IdentityMode::Live), Ok(true));

        assert_eq!(
            fake.events(),
            [
                format!("context:{TOKEN}"),
                "open".to_owned(),
                "verify".to_owned()
            ]
        );
        assert_eq!(*fake.posted.borrow(), [(TOKEN.to_owned(), Some(proof()))]);
    }

    #[test]
    fn a_failed_selfie_check_posts_nothing() {
        let mut fake = Fake::succeeding();
        fake.open = Ok(SelfieCheckCompletion::Failed(IdKitFailure::Known(
            IdKitErrorCode::VerificationRejected,
        )));

        let outcome = human(&fake, IdentityMode::Live);

        assert_eq!(
            outcome,
            Err(describe_world_id_failure("verification_rejected").to_owned())
        );
        assert!(fake.posted.borrow().is_empty());
    }

    #[test]
    fn a_rejected_verify_surfaces_the_servers_message() {
        let mut fake = Fake::succeeding();
        fake.verify = Err("World ID rejected the proof".to_owned());

        assert_eq!(
            human(&fake, IdentityMode::Live),
            Err("World ID rejected the proof".to_owned())
        );
    }

    #[test]
    fn the_message_delivered_flag_survives_the_whole_path() {
        let mut fake = Fake::succeeding();
        fake.verify = Ok(false);

        assert_eq!(human(&fake, IdentityMode::Live), Ok(false));
    }
}

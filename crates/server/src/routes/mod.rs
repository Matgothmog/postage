//! The API's routes, each answering exactly as the Next.js route handler it
//! replaces did: same method, path, status codes and JSON.
//!
//! What a Next.js handler did when it threw is part of that contract. Next
//! answers an unhandled error with a bare 500 and an empty body, and logs the
//! error; [`Unhandled`] is that path here. The error is logged by
//! [`log_unhandled`], which can reach the environment to redact it.

mod challenge_deliver;
mod challenge_resolve;
mod challenge_view;
mod inbox;
mod inbox_verify;
pub(crate) mod js;
mod mail_inbound;
mod network_view;
#[cfg(test)]
mod pipeline_tests;
#[cfg(test)]
pub(crate) mod testing;
mod wallet_nonce;
mod world_context;

use std::error::Error;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{Value, json};

use crate::app::AppState;
use crate::faults::{BoxError, reason_chain, redact};
use crate::log;

/// Builds the API router. Vercel rewrites every request to the single
/// function, so routes carry their full `/api/...` path.
///
/// `/api/challenge/{token}` sits beside `/api/challenge/resolve` and
/// `/api/challenge/deliver`; the static segments win, and no token is ever
/// either word.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/wallet-nonce", post(wallet_nonce::post))
        .route("/api/world/context", post(world_context::post))
        .route("/api/challenge/resolve", post(challenge_resolve::post))
        .route("/api/challenge/deliver", post(challenge_deliver::post))
        .route("/api/challenge/{token}", get(challenge_view::get))
        .route("/api/inbox", get(inbox::get).post(inbox::post))
        .route(
            "/api/inbox/verify",
            get(inbox_verify::get).post(inbox_verify::post),
        )
        .route("/api/network", get(network_view::get))
        .route("/api/mail/inbound", post(mail_inbound::post))
        .layer(from_fn_with_state(state.clone(), log_unhandled))
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// What a handler returns: its answer, or how it left early.
pub(crate) type RouteResult = Result<Response, Exit>;

/// Leaving a handler before its last line: with an answer already written (a
/// refusal), or with a failure Next.js would have turned into a bare 500.
/// Either way `?` carries it out of whichever helper found it. The answer is
/// boxed so the happy path does not carry a whole response's width.
#[derive(Debug)]
pub(crate) enum Exit {
    Answer(Box<Response>),
    Unhandled(Unhandled),
}

impl<E: Error + Send + Sync + 'static> From<E> for Exit {
    fn from(error: E) -> Self {
        Self::Unhandled(error.into())
    }
}

impl From<Unhandled> for Exit {
    fn from(unhandled: Unhandled) -> Self {
        Self::Unhandled(unhandled)
    }
}

impl IntoResponse for Exit {
    fn into_response(self) -> Response {
        match self {
            Self::Answer(response) => *response,
            Self::Unhandled(unhandled) => unhandled.into_response(),
        }
    }
}

/// An early exit with a refusal: see [`refusal`].
pub(crate) fn refuse(status: StatusCode, message: &str) -> Exit {
    Exit::Answer(Box::new(refusal(status, message)))
}

/// A request body no field of which can be read: not JSON, or a field of the
/// wrong type. The TypeScript let these throw, which Next.js answered with a
/// bare 500, usually after something had already been spent; they are refused
/// before anything is, on purpose.
pub(crate) fn invalid_request() -> Exit {
    refuse(StatusCode::BAD_REQUEST, "Invalid request")
}

/// A failure no handler answered for. Carried to [`log_unhandled`] in the
/// response's extensions, since a response cannot reach the environment the
/// log line has to be redacted against.
#[derive(Debug, Clone)]
pub(crate) struct Unhandled(Arc<dyn Error + Send + Sync + 'static>);

impl<E: Error + Send + Sync + 'static> From<E> for Unhandled {
    fn from(error: E) -> Self {
        Self(Arc::new(error))
    }
}

impl Unhandled {
    /// For a failure that arrives already boxed, as a `catch` clause's would.
    pub(crate) fn from_boxed(error: BoxError) -> Self {
        Self(Arc::from(error))
    }

    /// What will be logged, for a test that answers without the router.
    #[cfg(test)]
    pub(crate) fn error(&self) -> &(dyn Error + Send + Sync + 'static) {
        self.0.as_ref()
    }
}

impl IntoResponse for Unhandled {
    fn into_response(self) -> Response {
        let mut response = StatusCode::INTERNAL_SERVER_ERROR.into_response();
        response.extensions_mut().insert(self);
        response
    }
}

/// Logs what [`Unhandled`] carried, with this deployment's secrets taken out.
async fn log_unhandled(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let mut response = next.run(request).await;
    if let Some(Unhandled(error)) = response.extensions_mut().remove::<Unhandled>() {
        let reason = redact(state.env(), &reason_chain(error.as_ref()));
        log::error(
            "unhandled route error",
            &[("path", &path), ("reason", &reason)],
        );
    }
    response
}

/// `Response.json(body, { status })`.
pub(crate) fn json<T: Serialize>(status: StatusCode, body: &T) -> Response {
    (status, Json(body)).into_response()
}

/// `Response.json({ error: message }, { status })`, the shape every refusal
/// on these routes takes.
pub(crate) fn refusal(status: StatusCode, message: &str) -> Response {
    json(status, &json!({ "error": message }))
}

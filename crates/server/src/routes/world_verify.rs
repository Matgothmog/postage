//! `POST /api/world/verify` (`web/src/app/api/world/verify/route.ts`): the
//! free lane. The route itself lives in [`crate::world_verify`]; this only
//! hands it the request.

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;

use crate::app::AppState;
use crate::world_verify::{Personhood, handle};

pub(crate) async fn post(State(state): State<AppState>, body: Bytes) -> Response {
    handle(&state, &body, &Personhood).await
}

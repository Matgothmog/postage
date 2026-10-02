//! Vercel function entry point: serves the `postage-server` router.

use postage_server::{AppState, router};
use tower::ServiceBuilder;
use vercel_runtime::{Error, axum::VercelLayer};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let app = ServiceBuilder::new()
        .layer(VercelLayer::new())
        .service(router(AppState::from_process()));
    vercel_runtime::run(app).await
}

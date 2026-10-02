//! The browser bundle, served the way Vercel serves it (every path that is not
//! a file answers `index.html`), with one dev-only difference: the page's
//! bridge script is swapped for `/__e2e/boot.js`, which installs the mock
//! Privy/IDKit SDK from `crates/web/tests/mock_sdk.js`, because the real
//! Privy cannot run against dummy ids.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

const BRIDGE_TAG: &str = r#"<script type="module" src="/bridge/bridge.js"></script>"#;

pub struct Site {
    /// `crates/web/dist`.
    pub dist: PathBuf,
    /// The repository root, where the mock SDK and `e2e/boot.js` live.
    pub repo: PathBuf,
    /// Handed to the page as `window.__E2E` before anything else runs.
    pub config: Value,
}

pub async fn serve(site: Arc<Site>, uri: Uri) -> Response {
    let path = uri.path();
    if let Some(name) = path.strip_prefix("/__e2e/") {
        return injected(&site, name);
    }
    let Some(relative) = safe_relative(path) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if !relative.as_os_str().is_empty()
        && let Ok(bytes) = std::fs::read(site.dist.join(&relative))
    {
        return file(&relative, bytes);
    }
    // A path with an extension that is not in dist is a missing asset, not a
    // route for the client router.
    if relative.extension().is_some() {
        return StatusCode::NOT_FOUND.into_response();
    }
    index(&site)
}

fn injected(site: &Site, name: &str) -> Response {
    let source = match name {
        "boot.js" => site.repo.join("e2e/boot.js"),
        "bridge-core.js" => site.repo.join("crates/web/js/src/bridge-core.js"),
        "mock_sdk.js" => site.repo.join("crates/web/tests/mock_sdk.js"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    match std::fs::read(&source) {
        Ok(bytes) => file(Path::new(name), bytes),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn index(site: &Site) -> Response {
    let Ok(html) = std::fs::read_to_string(site.dist.join("index.html")) else {
        return (StatusCode::NOT_FOUND, "crates/web/dist is not built").into_response();
    };
    let config = site.config.to_string().replace('<', "\\u003c");
    let replacement = format!(
        r#"<script>window.__E2E = {config};</script><script type="module" src="/__e2e/boot.js"></script>"#
    );
    if !html.contains(BRIDGE_TAG) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "index.html has no bridge script to swap",
        )
            .into_response();
    }
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html.replace(BRIDGE_TAG, &replacement),
    )
        .into_response()
}

/// The request path as a relative path that cannot leave `dist`.
fn safe_relative(path: &str) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for component in Path::new(path.trim_start_matches('/')).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(relative)
}

fn file(path: &Path, bytes: Vec<u8>) -> Response {
    let kind = match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript",
        Some("css") => "text/css",
        Some("wasm") => "application/wasm",
        Some("woff2") => "font/woff2",
        Some("json") => "application/json",
        Some("ico") => "image/x-icon",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, kind)], bytes).into_response()
}

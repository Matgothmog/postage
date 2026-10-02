//! The browser's `fetch`, as a small typed layer: a request value in, a status
//! and body text out.
//!
//! `web-sys` is used directly rather than `gloo-net`. The app needs exactly
//! one thing from `fetch` (send a method, headers and a text body; read a
//! status and text back, optionally abortable), which is a page of `js-sys`
//! calls, and `gloo-net` would add a crate and its own error type to wrap the
//! same calls. Looking `fetch` up on the global object at call time (rather
//! than binding it once) also lets the browser tests replace `globalThis.fetch`
//! with a scripted one.
//!
//! Nothing here interprets a status: whether a 404 is "gone" or a 503 is "try
//! again" is the caller's decision, and lives in pure functions that run
//! natively under test.

use js_sys::{Function, Object, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{AbortSignal, Response};

/// What to send. `path` is relative to the page's origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: &'static str,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

impl HttpRequest {
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: "GET",
            path: path.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// A POST with a JSON body (and the matching `Content-Type`).
    pub fn post_json(path: impl Into<String>, body: String) -> Self {
        Self {
            method: "POST",
            path: path.into(),
            headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
            body: Some(body),
        }
    }

    /// Adds headers after the ones already set (`Content-Type` first, then
    /// the wallet proof), the order the TypeScript wrote them in.
    pub fn with_headers<K, V>(mut self, headers: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.headers
            .extend(headers.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }
}

/// An answer, whatever its status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    /// 2xx, `Response.ok`.
    pub fn is_ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Why no answer arrived. A reply of any status is an `HttpResponse`, not
/// this.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    /// The request never completed: offline, refused, blocked, or aborted.
    #[error("the request did not complete: {0}")]
    Network(String),
    /// The reply arrived but its body could not be read as text.
    #[error("the reply could not be read: {0}")]
    Body(String),
}

/// Sends `request`. `signal` aborts it from the outside (the strip's poll
/// aborts the previous tick when the next one starts).
pub async fn send(
    request: &HttpRequest,
    signal: Option<&AbortSignal>,
) -> Result<HttpResponse, HttpError> {
    let network = |cause: JsValue| HttpError::Network(describe(&cause));

    let global = js_sys::global();
    let fetch: Function = Reflect::get(&global, &"fetch".into())
        .map_err(network)?
        .dyn_into()
        .map_err(|_| HttpError::Network("fetch is not available".to_owned()))?;
    let init = init_object(request, signal).map_err(network)?;
    let promise = fetch
        .call2(&global, &JsValue::from_str(&request.path), &init)
        .map_err(network)?;
    let response: Response = JsFuture::from(js_sys::Promise::from(promise))
        .await
        .map_err(network)?
        .dyn_into()
        .map_err(|_| HttpError::Network("fetch did not return a Response".to_owned()))?;

    let status = response.status();
    let text = response
        .text()
        .map_err(|cause| HttpError::Body(describe(&cause)))?;
    let body = JsFuture::from(text)
        .await
        .map_err(|cause| HttpError::Body(describe(&cause)))?
        .as_string()
        .unwrap_or_default();
    Ok(HttpResponse { status, body })
}

/// The second argument to `fetch`: `{method, headers, body?, signal?}`.
fn init_object(request: &HttpRequest, signal: Option<&AbortSignal>) -> Result<Object, JsValue> {
    let init = Object::new();
    Reflect::set(&init, &"method".into(), &request.method.into())?;

    let headers = Object::new();
    for (name, value) in &request.headers {
        Reflect::set(&headers, &name.as_str().into(), &value.as_str().into())?;
    }
    Reflect::set(&init, &"headers".into(), &headers)?;

    if let Some(body) = &request.body {
        Reflect::set(&init, &"body".into(), &body.as_str().into())?;
    }
    if let Some(signal) = signal {
        Reflect::set(&init, &"signal".into(), signal)?;
    }
    Ok(init)
}

/// A readable line for whatever `fetch` rejected with.
fn describe(cause: &JsValue) -> String {
    if let Some(error) = cause.dyn_ref::<js_sys::Error>() {
        return error.message().into();
    }
    cause
        .as_string()
        .unwrap_or_else(|| "unknown error".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_post_declares_its_content_type_before_any_proof_headers() {
        let request = HttpRequest::post_json("/api/inbox", "{}".to_owned())
            .with_headers([("privy-id-token", "abc")]);
        assert_eq!(request.method, "POST");
        assert_eq!(
            request.headers,
            vec![
                ("Content-Type".to_owned(), "application/json".to_owned()),
                ("privy-id-token".to_owned(), "abc".to_owned()),
            ]
        );
        assert_eq!(request.body.as_deref(), Some("{}"));
    }

    #[test]
    fn a_get_has_no_body_and_no_headers_until_given_some() {
        let request = HttpRequest::get("/api/inbox");
        assert_eq!(request.method, "GET");
        assert!(request.headers.is_empty() && request.body.is_none());
    }

    #[test]
    fn only_2xx_is_ok() {
        let at = |status| HttpResponse {
            status,
            body: String::new(),
        };
        assert!(at(200).is_ok() && at(299).is_ok());
        assert!(!at(199).is_ok() && !at(300).is_ok() && !at(404).is_ok());
    }
}

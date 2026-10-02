//! The workers-rs side: implements `Edge` and `InboundMessage` over the real
//! runtime and exports the two handlers. Holds no behaviour of its own; every
//! decision is in `inbound` and `release`.

use js_sys::futures::JsFuture;
use js_sys::{Array, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;
use worker::{
    AbortSignal, Context, Env, Fetch, ForwardableEmailMessage, Headers, KvStore, Method, Request,
    RequestInit, Response, Result, console_error, event,
};

use crate::inbound::{RETRY_LATER, handle_email};
use crate::ports::{
    Edge, EdgeError, GATEWAY_TIMEOUT_MS, HttpAnswer, InboundMessage, JsonPost, MAILGUN_TIMEOUT_MS,
    MimeUpload,
};
use crate::release::{ReleaseCall, ReleaseReply, handle_release};
use crate::settings::Settings;
use postage_shared::{GatewayNotice, RELEASE_SECRET_HEADER};

/// The KV binding name in `wrangler.toml`.
const HELD_BINDING: &str = "HELD";

#[wasm_bindgen]
extern "C" {
    /// The same message viewed through the builder form of `reply()`, which
    /// Cloudflare turns into a threaded reply itself. workers-rs only binds the
    /// form that takes a finished `EmailMessage`, which would mean writing MIME
    /// and its threading headers by hand. A type of our own, because a method
    /// cannot be added to the one workers-rs defines.
    type BuilderReplyMessage;

    #[wasm_bindgen(method, catch, js_name = "reply")]
    fn reply_with_builder(
        this: &BuilderReplyMessage,
        builder: &Object,
    ) -> std::result::Result<Promise, JsValue>;
}

fn describe(cause: impl std::fmt::Display) -> EdgeError {
    EdgeError(cause.to_string())
}

fn describe_js(cause: &JsValue) -> EdgeError {
    EdgeError(
        cause
            .dyn_ref::<js_sys::Error>()
            .map(|error| String::from(error.message()))
            .or_else(|| cause.as_string())
            .unwrap_or_else(|| "unknown error".to_owned()),
    )
}

fn load_settings(env: &Env) -> std::result::Result<Settings, crate::settings::MissingSetting> {
    Settings::load(|name| env.var(name).ok().map(|value| value.to_string()))
}

struct CloudflareEdge {
    held: KvStore,
}

impl CloudflareEdge {
    fn new(env: &Env) -> Result<Self> {
        Ok(Self {
            held: env.kv(HELD_BINDING)?,
        })
    }

    async fn send(request: Request, timeout_ms: u32) -> std::result::Result<HttpAnswer, EdgeError> {
        let signal = AbortSignal::from(web_sys::AbortSignal::timeout_with_u32(timeout_ms));
        let mut response = Fetch::Request(request)
            .send_with_signal(&signal)
            .await
            .map_err(describe)?;
        let body = response.text().await.map_err(describe)?;
        Ok(HttpAnswer {
            status: response.status_code(),
            body,
        })
    }
}

impl Edge for CloudflareEdge {
    async fn put_held(
        &self,
        key: &str,
        value: &[u8],
        expiration: u64,
    ) -> std::result::Result<(), EdgeError> {
        self.held
            .put_bytes(key, value)
            .map_err(describe)?
            .expiration(expiration)
            .execute()
            .await
            .map_err(describe)
    }

    async fn get_held(&self, key: &str) -> std::result::Result<Option<Vec<u8>>, EdgeError> {
        self.held.get(key).bytes().await.map_err(describe)
    }

    async fn delete_held(&self, key: &str) -> std::result::Result<(), EdgeError> {
        self.held.delete(key).await.map_err(describe)
    }

    async fn post_json(&self, post: &JsonPost) -> std::result::Result<HttpAnswer, EdgeError> {
        let headers = Headers::new();
        headers
            .set("Content-Type", "application/json")
            .map_err(describe)?;
        headers
            .set(RELEASE_SECRET_HEADER, &post.secret)
            .map_err(describe)?;
        let mut init = RequestInit::new();
        init.with_method(Method::Post)
            .with_headers(headers)
            .with_body(Some(JsValue::from_str(&post.body)));
        let request = Request::new_with_init(&post.url, &init).map_err(describe)?;
        Self::send(request, GATEWAY_TIMEOUT_MS).await
    }

    async fn post_mime(&self, upload: &MimeUpload) -> std::result::Result<HttpAnswer, EdgeError> {
        let form = web_sys::FormData::new().map_err(|cause| describe_js(&cause))?;
        for (name, value) in &upload.fields {
            form.append_with_str(name, value)
                .map_err(|cause| describe_js(&cause))?;
        }
        let bytes = Uint8Array::from(upload.message.as_slice());
        let parts = Array::of1(&bytes);
        let options = web_sys::BlobPropertyBag::new();
        options.set_type("message/rfc822");
        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)
            .map_err(|cause| describe_js(&cause))?;
        form.append_with_blob_and_filename("message", &blob, "held.eml")
            .map_err(|cause| describe_js(&cause))?;

        let headers = Headers::new();
        headers
            .set("Authorization", &upload.authorization)
            .map_err(describe)?;
        let mut init = RequestInit::new();
        init.with_method(Method::Post)
            .with_headers(headers)
            .with_body(Some(form.into()));
        let request = Request::new_with_init(&upload.url, &init).map_err(describe)?;
        Self::send(request, MAILGUN_TIMEOUT_MS).await
    }

    fn now_seconds(&self) -> u64 {
        (js_sys::Date::now() / 1000.0) as u64
    }

    fn log_error(&self, label: &str, details: &[(&str, String)]) {
        let rendered: Vec<String> = details
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        console_error!("{} {}", label, rendered.join(" "));
    }
}

struct CloudflareMessage(ForwardableEmailMessage);

impl InboundMessage for CloudflareMessage {
    fn envelope_from(&self) -> String {
        self.0.from()
    }

    fn envelope_to(&self) -> String {
        self.0.to()
    }

    fn authentication_results(&self) -> Option<String> {
        // Plain copies first, then ARC ones, so `auth_results` can prefer the
        // plain header. Each is every copy joined with ", " in message order.
        let headers = self.0.headers();
        let read = |name: &str| headers.get(name).ok().flatten();
        let joined: Vec<String> = ["authentication-results", "arc-authentication-results"]
            .into_iter()
            .filter_map(read)
            .collect();
        (!joined.is_empty()).then(|| joined.join(", "))
    }

    async fn read_raw(&self) -> std::result::Result<Vec<u8>, EdgeError> {
        self.0.raw_bytes().await.map_err(describe)
    }

    fn set_reject(&self, reason: &str) {
        self.0.set_reject(reason);
    }

    async fn forward(&self, to: &str) -> std::result::Result<(), EdgeError> {
        self.0
            .forward(to)
            .await
            .map(|_| ())
            .map_err(|cause| describe_js(&cause))
    }

    async fn reply(
        &self,
        from_name: &str,
        from_email: &str,
        notice: &GatewayNotice,
    ) -> std::result::Result<(), EdgeError> {
        let builder =
            reply_builder(from_name, from_email, notice).map_err(|cause| describe_js(&cause))?;
        let promise = self
            .0
            .unchecked_ref::<BuilderReplyMessage>()
            .reply_with_builder(&builder)
            .map_err(|cause| describe_js(&cause))?;
        JsFuture::from(promise)
            .await
            .map(|_| ())
            .map_err(|cause| describe_js(&cause))
    }
}

/// `{ from: { name, email }, subject, text, html }`, the shape `reply()` takes.
fn reply_builder(
    from_name: &str,
    from_email: &str,
    notice: &GatewayNotice,
) -> std::result::Result<Object, JsValue> {
    let sender = Object::new();
    Reflect::set(&sender, &"name".into(), &from_name.into())?;
    Reflect::set(&sender, &"email".into(), &from_email.into())?;

    let builder = Object::new();
    Reflect::set(&builder, &"from".into(), &sender)?;
    Reflect::set(&builder, &"subject".into(), &notice.subject.as_str().into())?;
    Reflect::set(&builder, &"text".into(), &notice.text.as_str().into())?;
    Reflect::set(&builder, &"html".into(), &notice.html.as_str().into())?;
    Ok(builder)
}

#[event(email)]
async fn email(message: ForwardableEmailMessage, env: Env, _ctx: Context) -> Result<()> {
    let message = CloudflareMessage(message);
    let settings = match load_settings(&env) {
        Ok(settings) => settings,
        Err(missing) => {
            console_error!("worker is not configured: {}", missing);
            message.set_reject(RETRY_LATER);
            return Ok(());
        }
    };
    let edge = match CloudflareEdge::new(&env) {
        Ok(edge) => edge,
        Err(cause) => {
            console_error!("worker is not configured: {}", cause);
            message.set_reject(RETRY_LATER);
            return Ok(());
        }
    };
    handle_email(&edge, &settings, &message).await;
    Ok(())
}

#[event(fetch)]
async fn fetch(mut request: Request, env: Env, _ctx: Context) -> Result<Response> {
    let (settings, edge) = match (load_settings(&env), CloudflareEdge::new(&env)) {
        (Ok(settings), Ok(edge)) => (settings, edge),
        _ => {
            console_error!("worker is not configured");
            return Response::error(RETRY_LATER, 503);
        }
    };

    let path = request.path();
    let method = request.method().to_string();
    let secret = request.headers().get(RELEASE_SECRET_HEADER)?;
    // Read whatever the method, so the borrow ends before the handler runs; an
    // unreadable body reads as empty and is refused as not JSON.
    let body = request.bytes().await.unwrap_or_default();

    let call = ReleaseCall {
        method: &method,
        path: &path,
        secret: secret.as_deref(),
        body: &body,
    };
    match handle_release(&edge, &settings, call).await {
        ReleaseReply::Sent => Response::from_json(&postage_shared::ReleaseResponse { sent: true }),
        ReleaseReply::Text { status, text } => Response::error(text, status),
    }
}

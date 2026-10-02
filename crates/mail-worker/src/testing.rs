//! In-memory stand-ins for the two boundaries, mirroring the fakes in
//! `worker/src/index.test.ts`: recorders, so what is asserted is which branch
//! ran and what it did, never what a stand-in was told to say.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use postage_shared::GatewayNotice;
use serde_json::Value;

use crate::ports::{Edge, EdgeError, HttpAnswer, InboundMessage, JsonPost, MimeUpload};
use crate::settings::Settings;

pub const SENDER: &str = "sender@example.com";
pub const RECIPIENT: &str = "demo@usepostage.com";

pub const RAW_MESSAGE: &str = "From: sender@example.com\r\nTo: demo@usepostage.com\r\nSubject: A question\r\n\r\nIs this thing on?";

pub const CLEAN_AUTH: &str = "mx.cloudflare.net; dkim=pass header.d=example.com header.i=@example.com; \
     spf=pass smtp.mailfrom=sender@example.com; dmarc=pass header.from=example.com";

pub fn settings() -> Settings {
    Settings {
        api_url: "https://gateway.test".to_owned(),
        secret: "shh".to_owned(),
        mailgun_api_base: "https://api.mailgun.test".to_owned(),
        mailgun_domain: "usepostage.com".to_owned(),
        mailgun_api_key: "key".to_owned(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub key: String,
    pub value: Vec<u8>,
    pub expiration: u64,
}

/// A logged line: its label, and its details flattened for substring checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logged {
    pub label: String,
    pub text: String,
}

#[derive(Default)]
pub struct FakeEdge {
    pub puts: RefCell<Vec<Held>>,
    pub put_failure: Option<String>,
    pub store: RefCell<HashMap<String, Vec<u8>>>,
    pub get_calls: RefCell<Vec<String>>,
    pub get_failure: Option<String>,
    pub delete_calls: RefCell<Vec<String>>,
    /// How many of the next deletes fail before one works.
    pub delete_failures: Cell<usize>,
    pub gateway_calls: RefCell<Vec<JsonPost>>,
    pub mailgun_calls: RefCell<Vec<MimeUpload>>,
    gateway: Option<Result<HttpAnswer, EdgeError>>,
    mailgun: Option<Result<HttpAnswer, EdgeError>>,
    pub logs: RefCell<Vec<Logged>>,
}

impl FakeEdge {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn gateway_answers(mut self, status: u16, body: Value) -> Self {
        self.gateway = Some(Ok(HttpAnswer {
            status,
            body: body.to_string(),
        }));
        self
    }

    pub fn gateway_is_unreachable(mut self) -> Self {
        self.gateway = Some(Err(EdgeError("connect ECONNREFUSED".to_owned())));
        self
    }

    pub fn mailgun_answers(mut self, status: u16, body: &str) -> Self {
        self.mailgun = Some(Ok(HttpAnswer {
            status,
            body: body.to_owned(),
        }));
        self
    }

    pub fn mailgun_fails(mut self, cause: &str) -> Self {
        self.mailgun = Some(Err(EdgeError(cause.to_owned())));
        self
    }

    pub fn failing_puts(mut self, failure: &str) -> Self {
        self.put_failure = Some(failure.to_owned());
        self
    }

    pub fn failing_gets(mut self, failure: &str) -> Self {
        self.get_failure = Some(failure.to_owned());
        self
    }

    pub fn seeded(self, key: &str, value: &str) -> Self {
        self.store
            .borrow_mut()
            .insert(key.to_owned(), value.as_bytes().to_vec());
        self
    }

    pub fn failing_deletes(self, count: usize) -> Self {
        self.delete_failures.set(count);
        self
    }

    pub fn labels(&self) -> Vec<String> {
        self.logs
            .borrow()
            .iter()
            .map(|line| line.label.clone())
            .collect()
    }
}

impl Edge for FakeEdge {
    async fn put_held(&self, key: &str, value: &[u8], expiration: u64) -> Result<(), EdgeError> {
        if let Some(failure) = &self.put_failure {
            return Err(EdgeError(failure.clone()));
        }
        self.puts.borrow_mut().push(Held {
            key: key.to_owned(),
            value: value.to_vec(),
            expiration,
        });
        Ok(())
    }

    async fn get_held(&self, key: &str) -> Result<Option<Vec<u8>>, EdgeError> {
        self.get_calls.borrow_mut().push(key.to_owned());
        if let Some(failure) = &self.get_failure {
            return Err(EdgeError(failure.clone()));
        }
        Ok(self.store.borrow().get(key).cloned())
    }

    async fn delete_held(&self, key: &str) -> Result<(), EdgeError> {
        self.delete_calls.borrow_mut().push(key.to_owned());
        if self.delete_failures.get() > 0 {
            self.delete_failures.set(self.delete_failures.get() - 1);
            return Err(EdgeError("KV delete failed".to_owned()));
        }
        self.store.borrow_mut().remove(key);
        Ok(())
    }

    async fn post_json(&self, post: &JsonPost) -> Result<HttpAnswer, EdgeError> {
        self.gateway_calls.borrow_mut().push(post.clone());
        self.gateway
            .clone()
            .unwrap_or_else(|| Err(EdgeError("no gateway stand-in configured".to_owned())))
    }

    async fn post_mime(&self, upload: &MimeUpload) -> Result<HttpAnswer, EdgeError> {
        self.mailgun_calls.borrow_mut().push(upload.clone());
        self.mailgun
            .clone()
            .unwrap_or_else(|| Err(EdgeError("no Mailgun stand-in configured".to_owned())))
    }

    fn now_seconds(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default()
    }

    fn log_error(&self, label: &str, details: &[(&str, String)]) {
        let text = details
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(" ");
        self.logs.borrow_mut().push(Logged {
            label: label.to_owned(),
            text,
        });
    }
}

/// The message Cloudflare hands the email handler, as a recorder.
pub struct FakeMessage {
    /// `None` for a message that arrived without the header at all.
    authentication: Option<String>,
    forward_fails: bool,
    reply_fails: bool,
    pub rejects: RefCell<Vec<String>>,
    pub forwards: RefCell<Vec<String>>,
    pub replies: RefCell<Vec<String>>,
}

impl FakeMessage {
    pub fn new() -> Self {
        Self {
            authentication: Some(CLEAN_AUTH.to_owned()),
            forward_fails: false,
            reply_fails: false,
            rejects: RefCell::default(),
            forwards: RefCell::default(),
            replies: RefCell::default(),
        }
    }

    pub fn authenticated_as(mut self, header: &str) -> Self {
        self.authentication = Some(header.to_owned());
        self
    }

    pub fn without_authentication(mut self) -> Self {
        self.authentication = None;
        self
    }

    pub fn forward_fails(mut self) -> Self {
        self.forward_fails = true;
        self
    }

    pub fn reply_fails(mut self) -> Self {
        self.reply_fails = true;
        self
    }
}

impl InboundMessage for FakeMessage {
    fn envelope_from(&self) -> String {
        SENDER.to_owned()
    }

    fn envelope_to(&self) -> String {
        RECIPIENT.to_owned()
    }

    fn authentication_results(&self) -> Option<String> {
        self.authentication.clone()
    }

    async fn read_raw(&self) -> Result<Vec<u8>, EdgeError> {
        Ok(RAW_MESSAGE.as_bytes().to_vec())
    }

    fn set_reject(&self, reason: &str) {
        self.rejects.borrow_mut().push(reason.to_owned());
    }

    async fn forward(&self, to: &str) -> Result<(), EdgeError> {
        if self.forward_fails {
            return Err(EdgeError("not a verified destination".to_owned()));
        }
        self.forwards.borrow_mut().push(to.to_owned());
        Ok(())
    }

    async fn reply(
        &self,
        _from_name: &str,
        _from_email: &str,
        notice: &GatewayNotice,
    ) -> Result<(), EdgeError> {
        // Refused for a sender Cloudflare could not authenticate, which is the
        // case the caller has to survive rather than the exception.
        if self.reply_fails {
            return Err(EdgeError(
                "cannot reply to an unauthenticated sender".to_owned(),
            ));
        }
        self.replies.borrow_mut().push(notice.subject.clone());
        Ok(())
    }
}

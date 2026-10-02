//! Sending the two messages Postage writes itself, through Resend
//! (`web/src/lib/mail.ts`). What they say is composed in
//! [`postage_core::mail`]; this is only the way out.

use std::fmt;
use std::time::Duration;

use postage_core::mail::{
    relayed_text, unverified_relayed_text, verification_code_subject, verification_code_text,
};
use serde::Serialize;

use crate::config::{ConfigError, required};

pub const RESEND_ENDPOINT: &str = "https://api.resend.com/emails";

/// How long Resend may take to accept a message. The TypeScript set none.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// How much of a refusal's body is kept in the error, in UTF-16 code units as
/// `String.prototype.slice` counts them.
const REFUSAL_EXCERPT_UNITS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MailError {
    #[error("{0}")]
    Transport(String),
    /// Resend answered with a failure status: what we were doing, then the
    /// start of what Resend said.
    #[error("{what}: {detail}")]
    Refused { what: &'static str, detail: String },
}

/// A message a cleared sender pasted back in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldMessage<'a> {
    pub to: &'a str,
    pub from: &'a str,
    pub handle: &'a str,
    pub subject: &'a str,
    pub body: &'a str,
    /// Whether the receiving server confirmed `from` when the message was
    /// held. Only then is it named as the place replies go.
    pub sender_verified: bool,
}

/// The JSON Resend takes, in the order the TypeScript wrote it.
#[derive(Debug, Serialize)]
struct Outgoing<'a> {
    from: &'a str,
    to: &'a str,
    subject: &'a str,
    text: &'a str,
    /// Where a reply should go, when that is not us.
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to: Option<&'a str>,
}

#[derive(Clone)]
pub struct Mailer {
    client: reqwest::Client,
    api_key: String,
    from: String,
    endpoint: String,
}

impl fmt::Debug for Mailer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Mailer")
            .field("from", &self.from)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl Mailer {
    pub fn new(client: reqwest::Client, api_key: String, from: String) -> Self {
        Self {
            client,
            api_key,
            from,
            endpoint: RESEND_ENDPOINT.to_owned(),
        }
    }

    /// Reads `RESEND_API_KEY` and `MAIL_FROM`.
    pub fn from_env<F>(env: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        Ok(Self::new(
            reqwest::Client::default(),
            required(&env, "RESEND_API_KEY")?,
            required(&env, "MAIL_FROM")?,
        ))
    }

    /// Sends somewhere other than Resend's API.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Sends the code that proves whoever is claiming a handle can read the
    /// address they point it at. Without it anyone could aim a handle at a
    /// stranger's inbox and have us forward to it.
    pub async fn send_verification_code(
        &self,
        to: &str,
        handle: &str,
        code: &str,
    ) -> Result<(), MailError> {
        let subject = verification_code_subject(code);
        let text = verification_code_text(handle, code);
        self.send(
            Outgoing {
                from: &self.from,
                to,
                subject: &subject,
                text: &text,
                reply_to: None,
            },
            "Could not send the verification code",
        )
        .await
    }

    /// Sends a message a cleared sender pasted back in. It goes out under our
    /// own name with theirs in Reply-To rather than forged into From: a
    /// message claiming to be from them would be unsigned mail wearing their
    /// domain, which is what this gateway exists to catch.
    ///
    /// An unverified sender gets neither: anyone can write any address on an
    /// envelope, and a Reply-To naming one we never checked would hand the
    /// recipient's answer to whoever they were pretending to be.
    pub async fn relay_held_message(&self, message: &HeldMessage<'_>) -> Result<(), MailError> {
        let (text, reply_to) = if message.sender_verified {
            (
                relayed_text(message.body, message.from, message.handle),
                // An empty Reply-To was left off, as `...(replyTo ? ...)` did.
                Some(message.from).filter(|from| !from.is_empty()),
            )
        } else {
            (
                unverified_relayed_text(message.body, message.from, message.handle),
                None,
            )
        };
        self.send(
            Outgoing {
                from: &self.from,
                to: message.to,
                subject: message.subject,
                text: &text,
                reply_to,
            },
            "Could not deliver it",
        )
        .await
    }

    async fn send(&self, message: Outgoing<'_>, what: &'static str) -> Result<(), MailError> {
        let body = serde_json::to_string(&message)
            .map_err(|error| MailError::Transport(error.to_string()))?;
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .timeout(SEND_TIMEOUT)
            .send()
            .await
            .map_err(|error| MailError::Transport(error.without_url().to_string()))?;
        if response.status().is_success() {
            return Ok(());
        }

        let said = response
            .text()
            .await
            .map_err(|error| MailError::Transport(error.without_url().to_string()))?;
        Err(MailError::Refused {
            what,
            detail: utf16_prefix(&said, REFUSAL_EXCERPT_UNITS).to_owned(),
        })
    }
}

/// The longest prefix of `text` that is at most `units` UTF-16 code units,
/// which is what `slice(0, units)` keeps. Where JavaScript would cut a
/// surrogate pair in half, the whole character is left out instead.
fn utf16_prefix(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (index, character) in text.char_indices() {
        used += character.len_utf16();
        if used > units {
            return &text[..index];
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::http_stub::{Reply, Stub, closed_port, serve_with};

    const FROM: &str = "Postage <hello@usepostage.com>";

    fn mailer(stub: &Stub) -> Mailer {
        Mailer::new(
            reqwest::Client::default(),
            "re_test_key".to_owned(),
            FROM.to_owned(),
        )
        .with_endpoint(format!("{}/emails", stub.base))
    }

    async fn accepting() -> Stub {
        serve_with(|_| Reply::new(200, r#"{"id":"email-1"}"#)).await
    }

    #[tokio::test]
    async fn a_verification_code_goes_to_resend_with_our_sender_and_no_reply_to() {
        let stub = accepting().await;

        mailer(&stub)
            .send_verification_code("reader@example.com", "demo", "123456")
            .await
            .unwrap();

        let sent = stub.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].method, "POST");
        assert_eq!(sent[0].path, "/emails");
        assert_eq!(sent[0].header("authorization"), Some("Bearer re_test_key"));
        assert_eq!(sent[0].content_type.as_deref(), Some("application/json"));
        assert_eq!(
            sent[0].body,
            json!({
                "from": FROM,
                "to": "reader@example.com",
                "subject": "123456 is your Postage code",
                "text": verification_code_text("demo", "123456"),
            })
        );
    }

    #[tokio::test]
    async fn a_relayed_message_names_the_sender_in_reply_to_not_from() {
        let stub = accepting().await;

        mailer(&stub)
            .relay_held_message(&HeldMessage {
                to: "reader@example.com",
                from: "alice@example.com",
                handle: "demo",
                subject: "Hello",
                body: "Pasted back in.",
                sender_verified: true,
            })
            .await
            .unwrap();

        let sent = stub.sent();
        assert_eq!(
            sent[0].body,
            json!({
                "from": FROM,
                "to": "reader@example.com",
                "subject": "Hello",
                "text": relayed_text("Pasted back in.", "alice@example.com", "demo"),
                "reply_to": "alice@example.com",
            })
        );
    }

    #[tokio::test]
    async fn an_unverified_senders_relay_has_no_reply_to_and_says_so() {
        let stub = accepting().await;

        mailer(&stub)
            .relay_held_message(&HeldMessage {
                to: "reader@example.com",
                from: "alice@example.com",
                handle: "demo",
                subject: "Hello",
                body: "Pasted back in.",
                sender_verified: false,
            })
            .await
            .unwrap();

        let sent = stub.sent();
        assert_eq!(
            sent[0].body,
            json!({
                "from": FROM,
                "to": "reader@example.com",
                "subject": "Hello",
                "text": unverified_relayed_text("Pasted back in.", "alice@example.com", "demo"),
            })
        );
    }

    #[test]
    fn the_wire_order_is_the_one_the_typescript_wrote() {
        let outgoing = Outgoing {
            from: "f",
            to: "t",
            subject: "s",
            text: "x",
            reply_to: Some("r"),
        };
        assert_eq!(
            serde_json::to_string(&outgoing).unwrap(),
            r#"{"from":"f","to":"t","subject":"s","text":"x","reply_to":"r"}"#
        );
    }

    #[tokio::test]
    async fn a_refusal_says_what_failed_and_quotes_the_start_of_resends_answer() {
        let long = format!("{{\"message\":\"{}\"}}", "x".repeat(400));
        let stub = serve_with(move |_| Reply::new(422, long.clone())).await;

        let error = mailer(&stub)
            .send_verification_code("reader@example.com", "demo", "123456")
            .await
            .unwrap_err();

        let MailError::Refused { what, detail } = &error else {
            panic!("expected a refusal, got {error:?}");
        };
        assert_eq!(*what, "Could not send the verification code");
        assert_eq!(detail.len(), 200);
        assert!(
            error
                .to_string()
                .starts_with("Could not send the verification code: {\"message\"")
        );
    }

    #[tokio::test]
    async fn a_failed_relay_is_worded_as_a_delivery() {
        let stub = serve_with(|_| Reply::new(500, "boom")).await;

        let error = mailer(&stub)
            .relay_held_message(&HeldMessage {
                to: "reader@example.com",
                from: "alice@example.com",
                handle: "demo",
                subject: "Hello",
                body: "Body",
                sender_verified: true,
            })
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "Could not deliver it: boom");
    }

    #[tokio::test]
    async fn an_unreachable_resend_is_a_transport_failure_that_keeps_the_key_out() {
        let mailer = Mailer::new(
            reqwest::Client::default(),
            "re_test_key".to_owned(),
            FROM.to_owned(),
        )
        .with_endpoint(closed_port());

        let error = mailer
            .send_verification_code("reader@example.com", "demo", "123456")
            .await
            .unwrap_err();

        assert!(matches!(error, MailError::Transport(_)), "{error}");
        assert!(!error.to_string().contains("re_test_key"));
    }

    #[test]
    fn the_excerpt_counts_utf16_units_and_never_splits_a_character() {
        assert_eq!(utf16_prefix("abc", 2), "ab");
        assert_eq!(utf16_prefix("abc", 200), "abc");
        assert_eq!(utf16_prefix("é✉x", 2), "é✉");
        // An emoji is two units; cutting after one keeps none of it.
        assert_eq!(utf16_prefix("a\u{1F4E8}b", 2), "a");
        assert_eq!(utf16_prefix("a\u{1F4E8}b", 3), "a\u{1F4E8}");
    }

    #[test]
    fn from_env_needs_the_key_and_the_sender_and_never_shows_the_key() {
        assert_eq!(
            Mailer::from_env(|name| (name == "RESEND_API_KEY").then(|| "k".to_owned()))
                .unwrap_err(),
            ConfigError::Missing("MAIL_FROM")
        );
        let mailer = Mailer::from_env(|name| Some(format!("value-of-{name}"))).unwrap();
        let shown = format!("{mailer:?}");
        assert!(shown.contains(RESEND_ENDPOINT));
        assert!(!shown.contains("value-of-RESEND_API_KEY"));
    }
}

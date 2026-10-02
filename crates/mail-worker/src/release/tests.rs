//! `fetch()` is the other side of the hold `email()` writes: a capability token
//! and a shared secret, exchanged for the exact bytes that were kept. No test
//! body carries a real token, only stand-ins invented here, so nothing below
//! risks putting the capability itself in a failure message.

use serde_json::json;

use super::*;
use crate::testing::{FakeEdge, RAW_MESSAGE, settings};

const RETRY: &str = "Postage is temporarily unavailable, please retry";
const TOKEN: &str = "tok-release-1";
const TO: &str = "reader@personal.example";

fn valid_body() -> Vec<u8> {
    json!({"token": TOKEN, "to": TO}).to_string().into_bytes()
}

/// A well-formed release request; a test overrides only the one part it means
/// to break.
struct Request {
    method: &'static str,
    path: &'static str,
    secret: Option<&'static str>,
    body: Vec<u8>,
}

impl Request {
    fn valid() -> Self {
        Self {
            method: "POST",
            path: "/release",
            secret: Some("shh"),
            body: valid_body(),
        }
    }

    fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    fn with_json(self, body: serde_json::Value) -> Self {
        self.with_body(body.to_string())
    }
}

async fn send(edge: &FakeEdge, request: Request) -> ReleaseReply {
    let call = ReleaseCall {
        method: request.method,
        path: request.path,
        secret: request.secret,
        body: &request.body,
    };
    handle_release(edge, &settings(), call).await
}

fn text(status: u16, text: &'static str) -> ReleaseReply {
    ReleaseReply::Text { status, text }
}

#[tokio::test]
async fn a_path_other_than_release_answers_not_found_same_as_any_unknown_route() {
    let edge = FakeEdge::new();
    let reply = send(
        &edge,
        Request {
            path: "/other",
            ..Request::valid()
        },
    )
    .await;

    assert_eq!(reply, text(404, "Not found"));
    assert!(edge.get_calls.borrow().is_empty());
}

#[tokio::test]
async fn a_get_to_release_answers_not_found_only_post_releases_anything() {
    let edge = FakeEdge::new();
    let reply = send(
        &edge,
        Request {
            method: "GET",
            body: Vec::new(),
            ..Request::valid()
        },
    )
    .await;

    assert_eq!(reply, text(404, "Not found"));
}

/// The auth check beyond possession of the token: a token that is genuinely held
/// is not enough on its own, the caller also needs the secret this worker and
/// the gateway share. Seeding a real token and still getting refused is what
/// pins that the two checks are independent.
#[tokio::test]
async fn a_valid_token_on_its_own_is_not_enough_without_the_shared_secret() {
    let edge = FakeEdge::new().seeded(TOKEN, RAW_MESSAGE);
    let reply = send(
        &edge,
        Request {
            secret: Some("wrong"),
            ..Request::valid()
        },
    )
    .await;

    assert_eq!(reply, text(401, "Bad secret"));
    assert!(edge.get_calls.borrow().is_empty());
    assert!(edge.delete_calls.borrow().is_empty());
}

#[tokio::test]
async fn a_request_with_no_secret_header_at_all_is_refused_the_same_way_as_a_wrong_one() {
    let edge = FakeEdge::new();
    let reply = send(
        &edge,
        Request {
            secret: None,
            ..Request::valid()
        },
    )
    .await;

    assert_eq!(reply, text(401, "Bad secret"));
}

#[tokio::test]
async fn a_body_that_is_not_json_is_refused_as_a_bad_request() {
    let edge = FakeEdge::new();
    let reply = send(&edge, Request::valid().with_body("{not json")).await;

    assert_eq!(reply, text(400, "Body must be JSON"));
}

#[tokio::test]
async fn a_request_with_no_token_at_all_is_refused_before_kv_is_touched() {
    let edge = FakeEdge::new();
    let reply = send(&edge, Request::valid().with_json(json!({"to": TO}))).await;

    assert_eq!(reply, text(400, "token and to are required"));
    assert!(edge.get_calls.borrow().is_empty());
}

#[tokio::test]
async fn a_request_with_no_destination_is_refused_before_kv_is_touched() {
    let edge = FakeEdge::new();
    let reply = send(&edge, Request::valid().with_json(json!({"token": TOKEN}))).await;

    assert_eq!(reply, text(400, "token and to are required"));
    assert!(edge.get_calls.borrow().is_empty());
}

/// `null` is valid JSON that names neither field. It must be refused like any
/// other body without them, not blow up the handler (the TypeScript version
/// threw reading `token` off it).
#[tokio::test]
async fn a_json_null_body_is_refused_as_missing_its_fields_before_kv_is_touched() {
    let edge = FakeEdge::new();
    let reply = send(&edge, Request::valid().with_body("null")).await;

    assert_eq!(reply, text(400, "token and to are required"));
    assert!(edge.get_calls.borrow().is_empty());
}

#[tokio::test]
async fn an_empty_string_token_is_treated_as_no_token_at_all() {
    let edge = FakeEdge::new();
    let reply = send(
        &edge,
        Request::valid().with_json(json!({"token": "", "to": TO})),
    )
    .await;

    assert_eq!(reply, text(400, "token and to are required"));
}

#[tokio::test]
async fn an_unknown_token_finds_nothing_held() {
    let edge = FakeEdge::new();
    let reply = send(&edge, Request::valid()).await;

    assert_eq!(reply, text(404, "Nothing is held under that token"));
    assert_eq!(*edge.get_calls.borrow(), vec![TOKEN]);
}

/// The lookup is a straight KV read on whatever string arrives - there is no
/// separate notion of a malformed token, only found or not found. Pinned so a
/// later change that starts validating shape is a visible one.
#[tokio::test]
async fn a_malformed_looking_token_is_looked_up_the_same_as_any_other_and_found_just_as_absent() {
    let edge = FakeEdge::new();
    let reply = send(
        &edge,
        Request::valid().with_json(json!({"token": "' OR 1=1 --", "to": TO})),
    )
    .await;

    assert_eq!(reply, text(404, "Nothing is held under that token"));
}

/// A KV outage is answered, not thrown: nothing has been sent by this point and
/// the hold is untouched, so asking again is free, and the answer is the one the
/// rest of Postage gives for a fault of its own: temporary, retry.
#[tokio::test]
async fn a_kv_read_failure_answers_that_we_are_temporarily_unavailable_rather_than_escaping() {
    let edge = FakeEdge::new()
        .failing_gets("KV get failed")
        .mailgun_answers(200, "{}");
    let reply = send(&edge, Request::valid()).await;

    assert_eq!(reply, text(503, RETRY));
    assert!(
        edge.mailgun_calls.borrow().is_empty(),
        "nothing was sent, so retrying costs nothing"
    );
    assert_eq!(edge.labels(), vec!["release lookup failed"]);
}

#[tokio::test]
async fn a_valid_token_releases_the_message_to_the_address_the_request_names() {
    let edge = FakeEdge::new()
        .seeded(TOKEN, RAW_MESSAGE)
        .mailgun_answers(200, "{}");
    let reply = send(&edge, Request::valid()).await;

    assert_eq!(reply, ReleaseReply::Sent);
    let calls = edge.mailgun_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].url,
        "https://api.mailgun.test/v3/usepostage.com/messages.mime"
    );
    assert_eq!(field(&calls[0], "to"), TO);
    assert_eq!(calls[0].message, RAW_MESSAGE.as_bytes());
    assert_eq!(*edge.delete_calls.borrow(), vec![TOKEN]);
}

fn field<'a>(upload: &'a crate::ports::MimeUpload, name: &str) -> &'a str {
    upload
        .fields
        .iter()
        .find(|(field_name, _)| *field_name == name)
        .map(|(_, value)| value.as_str())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_release_goes_out_unsigned_and_untracked_so_the_senders_own_signature_still_covers_it() {
    let edge = FakeEdge::new()
        .seeded(TOKEN, RAW_MESSAGE)
        .mailgun_answers(200, "{}");

    send(&edge, Request::valid()).await;

    let calls = edge.mailgun_calls.borrow();
    assert_eq!(field(&calls[0], "o:dkim"), "no");
    assert_eq!(field(&calls[0], "o:tracking"), "no");
    assert_eq!(field(&calls[0], "o:tracking-clicks"), "no");
    assert_eq!(field(&calls[0], "o:tracking-opens"), "no");
    assert_eq!(calls[0].authorization, "Basic YXBpOmtleQ==");
}

#[tokio::test]
async fn a_token_that_already_released_once_finds_nothing_held_on_the_second_try() {
    let edge = FakeEdge::new()
        .seeded(TOKEN, RAW_MESSAGE)
        .mailgun_answers(200, "{}");

    let first = send(&edge, Request::valid()).await;
    let second = send(&edge, Request::valid()).await;

    assert_eq!(first, ReleaseReply::Sent);
    assert_eq!(second, text(404, "Nothing is held under that token"));
}

#[tokio::test]
async fn a_delivery_failure_answers_502_with_fixed_wording_not_mailguns_own_message_and_the_token_is_not_spent()
 {
    let edge = FakeEdge::new()
        .seeded(TOKEN, RAW_MESSAGE)
        .mailgun_answers(502, "boom from mailgun");

    let reply = send(&edge, Request::valid()).await;

    assert_eq!(reply, text(502, "Could not send it"));
    assert!(edge.delete_calls.borrow().is_empty());
    assert_eq!(edge.labels(), vec!["release delivery failed"]);
}

#[tokio::test]
async fn a_delivery_that_fails_before_mailgun_answers_still_gets_a_response_and_not_the_raw_network_error()
 {
    let edge = FakeEdge::new()
        .seeded(TOKEN, RAW_MESSAGE)
        .mailgun_fails("connect ECONNREFUSED");

    let reply = send(&edge, Request::valid()).await;

    assert_eq!(reply, text(502, "Could not send it"));
    assert!(edge.delete_calls.borrow().is_empty());
    assert_eq!(edge.labels(), vec!["release delivery failed"]);
}

/// The worker could be handed anything by its runtime, a rejection that is not an
/// Error among it. Here every boundary failure is already text, so this pins that
/// a bare, unadorned cause still ends in a readable log line and the same answer.
#[tokio::test]
async fn a_delivery_failure_whose_cause_is_a_bare_string_still_answers_with_something_readable() {
    let edge = FakeEdge::new()
        .seeded(TOKEN, RAW_MESSAGE)
        .mailgun_fails("boom");

    let reply = send(&edge, Request::valid()).await;

    assert_eq!(reply, text(502, "Could not send it"));
    assert_eq!(edge.labels(), vec!["release delivery failed"]);
    assert!(edge.logs.borrow()[0].text.contains("boom"));
}

/// `deliver_untouched` has already sent the message by the time the key is
/// removed. Retrying retires the token, and answering `sent` is what stops the
/// caller asking a second time for something that already happened.
#[tokio::test]
async fn a_delete_that_fails_once_is_retried_so_a_spent_token_cannot_release_the_same_message_twice()
 {
    let edge = FakeEdge::new()
        .seeded(TOKEN, RAW_MESSAGE)
        .mailgun_answers(200, "{}")
        .failing_deletes(1);

    let first = send(&edge, Request::valid()).await;

    assert_eq!(first, ReleaseReply::Sent);
    assert_eq!(
        edge.delete_calls.borrow().len(),
        2,
        "the delete that failed was tried again"
    );

    let second = send(&edge, Request::valid()).await;

    assert_eq!(second, text(404, "Nothing is held under that token"));
    assert_eq!(
        edge.mailgun_calls.borrow().len(),
        1,
        "the held message went out exactly once"
    );
}

/// The residual this design accepts. Every attempt fails, so the key outlives its
/// own release. What is still owed is an answer, and the answer is success: the
/// mail is already out, saying anything else invites a retry of a request that
/// worked and sends the message twice.
#[tokio::test]
async fn a_delete_that_never_succeeds_still_answers_that_the_message_was_sent_rather_than_crashing()
{
    let edge = FakeEdge::new()
        .seeded(TOKEN, RAW_MESSAGE)
        .mailgun_answers(200, "{}")
        .failing_deletes(usize::MAX);

    let reply = send(&edge, Request::valid()).await;

    assert_eq!(reply, ReleaseReply::Sent);
    assert_eq!(edge.mailgun_calls.borrow().len(), 1);
    assert_eq!(edge.labels(), vec!["hold not retired after release"]);
}

/// The token is the capability: whoever holds it can release the message, so a
/// failure on this path may name what went wrong and never what it went wrong on.
#[tokio::test]
async fn a_release_that_could_not_retire_its_hold_keeps_the_token_out_of_the_log() {
    let edge = FakeEdge::new()
        .seeded(TOKEN, RAW_MESSAGE)
        .mailgun_answers(200, "{}")
        .failing_deletes(usize::MAX);

    send(&edge, Request::valid()).await;

    let logs = edge.logs.borrow();
    assert_eq!(logs.len(), 1);
    assert!(
        !logs[0].label.contains(TOKEN) && !logs[0].text.contains(TOKEN),
        "the token is not written down"
    );
}

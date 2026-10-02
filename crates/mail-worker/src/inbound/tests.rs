//! `email()` is driven through the two boundaries it actually has: the gateway
//! it asks for a verdict, and the message and KV namespace Cloudflare hands it.

use serde_json::{Value, json};

use super::*;
use crate::testing::{FakeEdge, FakeMessage, RAW_MESSAGE, RECIPIENT, SENDER, settings};

const RETRY: &str = "Postage is temporarily unavailable, please retry";
const DAY_SECONDS: u64 = 24 * 60 * 60;

fn notice() -> Value {
    json!({"subject": "Held for release", "text": "Release it here", "html": "<p>Release it here</p>"})
}

async fn run(edge: &FakeEdge, message: &FakeMessage) {
    handle_email(edge, &settings(), message).await;
}

fn gateway_payload(edge: &FakeEdge) -> Value {
    serde_json::from_str(&edge.gateway_calls.borrow()[0].body).unwrap()
}

fn now_seconds() -> u64 {
    FakeEdge::new().now_seconds()
}

/// The collision fix, end to end: a header the gateway would have been told
/// nothing about now reaches it as the pass it is.
#[tokio::test]
async fn the_gateway_is_told_what_the_receiving_mta_concluded() {
    let edge = FakeEdge::new().gateway_answers(200, json!({"action": "reject", "bounce": "no"}));
    let message = FakeMessage::new()
        .authenticated_as("mx.cloudflare.net; dmarc=pass policy.dmarc=none; spf=pass");

    run(&edge, &message).await;

    let calls = edge.gateway_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].url, "https://gateway.test/api/mail/inbound");
    assert_eq!(calls[0].secret, "shh");
    let payload = gateway_payload(&edge);
    assert_eq!(payload["dmarc"], "pass");
    assert_eq!(payload["spf"], "pass");
    assert_eq!(payload["dkim"], Value::Null);
    assert_eq!(payload["from"], SENDER);
    assert_eq!(payload["to"], RECIPIENT);
    assert_eq!(payload["subject"], "A question");
    assert_eq!(payload["header_from"], SENDER);
    assert!(
        payload["body"]
            .as_str()
            .unwrap()
            .contains("Is this thing on?")
    );
}

/// A message the receiving MTA said nothing about is reported as unknown rather
/// than as anything, and the gateway - not the worker - is what decides that an
/// unauthenticated sender is not written back to.
#[tokio::test]
async fn a_message_that_arrived_with_no_authentication_header_is_reported_as_unknown() {
    let edge = FakeEdge::new().gateway_answers(200, json!({"action": "reject"}));
    let message = FakeMessage::new().without_authentication();

    run(&edge, &message).await;

    let payload = gateway_payload(&edge);
    assert_eq!(payload["spf"], Value::Null);
    assert_eq!(payload["dkim"], Value::Null);
    assert_eq!(payload["dmarc"], Value::Null);
}

#[tokio::test]
async fn a_gateway_that_cannot_be_reached_refuses_the_session_for_a_retry() {
    let edge = FakeEdge::new().gateway_is_unreachable();
    let message = FakeMessage::new();

    run(&edge, &message).await;

    assert_eq!(*message.rejects.borrow(), vec![RETRY]);
    assert!(message.forwards.borrow().is_empty());
    assert!(edge.puts.borrow().is_empty());
    assert_eq!(edge.labels(), vec!["classify failed"]);
}

#[tokio::test]
async fn a_gateway_that_answers_with_an_error_refuses_the_session_for_a_retry() {
    let edge = FakeEdge::new().gateway_answers(500, json!({"error": "boom"}));
    let message = FakeMessage::new();

    run(&edge, &message).await;

    assert_eq!(*message.rejects.borrow(), vec![RETRY]);
}

#[tokio::test]
async fn an_address_that_does_not_exist_is_refused_permanently_not_retried() {
    let edge = FakeEdge::new().gateway_answers(404, json!({}));
    let message = FakeMessage::new();

    run(&edge, &message).await;

    assert_eq!(
        *message.rejects.borrow(),
        vec!["No such address at this domain"]
    );
}

#[tokio::test]
async fn a_forward_verdict_delivers_to_the_address_the_gateway_verified() {
    let edge = FakeEdge::new().gateway_answers(
        200,
        json!({"action": "forward", "to": "owner@personal.example"}),
    );
    let message = FakeMessage::new();

    run(&edge, &message).await;

    assert_eq!(*message.forwards.borrow(), vec!["owner@personal.example"]);
    assert!(message.rejects.borrow().is_empty());
    assert!(edge.puts.borrow().is_empty());
}

#[tokio::test]
async fn a_destination_cloudflare_will_not_accept_refuses_the_session_for_a_retry() {
    let edge = FakeEdge::new().gateway_answers(
        200,
        json!({"action": "forward", "to": "owner@personal.example"}),
    );
    let message = FakeMessage::new().forward_fails();

    run(&edge, &message).await;

    assert_eq!(
        *message.rejects.borrow(),
        vec!["Postage could not deliver to that inbox, please retry"]
    );
}

/// A verdict that asks for a forward and names nowhere is our bug, and the
/// message is one the gateway meant to deliver. Refused as temporary so it waits
/// at the sending MTA instead of being written off.
#[tokio::test]
async fn a_forward_with_no_destination_is_refused_as_ours_to_fix_not_the_senders() {
    let edge = FakeEdge::new().gateway_answers(200, json!({"action": "forward"}));
    let message = FakeMessage::new();

    run(&edge, &message).await;

    assert_eq!(*message.rejects.borrow(), vec![RETRY]);
    assert!(message.forwards.borrow().is_empty());
    assert_eq!(edge.labels(), vec!["incoherent verdict"]);
}

#[tokio::test]
async fn a_hold_stores_the_bytes_that_arrived_under_the_gateways_token() {
    let held_until = now_seconds() + 900;
    let edge = FakeEdge::new().gateway_answers(
        200,
        json!({"action": "hold", "token": "tok-1", "held_until": held_until, "bounce": "Held."}),
    );
    let message = FakeMessage::new();

    run(&edge, &message).await;

    let puts = edge.puts.borrow();
    assert_eq!(puts.len(), 1);
    assert_eq!(puts[0].key, "tok-1");
    assert_eq!(puts[0].expiration, held_until);
    assert_eq!(puts[0].value, RAW_MESSAGE.as_bytes());
    assert_eq!(*message.rejects.borrow(), vec!["Held."]);
}

/// The floor under a gateway that forgot to set a deadline. Without it the value
/// would be written with no expiry at all and kept forever.
#[tokio::test]
async fn a_hold_with_no_deadline_is_stored_under_the_fallback_one() {
    let edge = FakeEdge::new().gateway_answers(200, json!({"action": "hold", "token": "tok-2"}));
    let message = FakeMessage::new();

    run(&edge, &message).await;

    let expiration = edge.puts.borrow()[0].expiration;
    assert!(
        expiration.abs_diff(now_seconds() + DAY_SECONDS) <= 2,
        "expected roughly a day out, got {expiration}"
    );
}

#[tokio::test]
async fn a_sender_who_was_sent_the_release_notice_is_not_also_refused() {
    let edge = FakeEdge::new().gateway_answers(
        200,
        json!({"action": "hold", "token": "tok-3", "notice": notice()}),
    );
    let message = FakeMessage::new();

    run(&edge, &message).await;

    assert_eq!(*message.replies.borrow(), vec!["Held for release"]);
    assert!(message.rejects.borrow().is_empty());
}

/// The forged-sender case: Cloudflare will not let us write to someone whose
/// name may not be theirs, so the link has to travel inside the SMTP refusal.
#[tokio::test]
async fn a_sender_who_cannot_be_written_to_is_told_inside_the_session_instead() {
    let edge = FakeEdge::new().gateway_answers(
        200,
        json!({"action": "hold", "token": "tok-4", "notice": notice(), "bounce": "Held, see the link"}),
    );
    let message = FakeMessage::new().reply_fails();

    run(&edge, &message).await;

    assert!(message.replies.borrow().is_empty());
    assert_eq!(*message.rejects.borrow(), vec!["Held, see the link"]);
    assert_eq!(edge.puts.borrow().len(), 1);
}

#[tokio::test]
async fn a_hold_the_gateway_sent_no_notice_for_still_refuses_the_session() {
    let edge = FakeEdge::new().gateway_answers(
        200,
        json!({"action": "hold", "token": "tok-5", "notice": null}),
    );
    let message = FakeMessage::new();

    run(&edge, &message).await;

    assert_eq!(
        *message.rejects.borrow(),
        vec!["Held. See the link in this message to release it"]
    );
}

/// KV holds the only copy. A put that did not land and a sender told the message
/// is waiting for them is the one combination that loses mail without anyone
/// noticing, so the session is refused and the bytes stay with the sending MTA.
#[tokio::test]
async fn a_hold_that_could_not_be_stored_refuses_the_session_rather_than_promising_to_keep_it() {
    let edge = FakeEdge::new()
        .gateway_answers(
            200,
            json!({"action": "hold", "token": "tok-6", "notice": notice()}),
        )
        .failing_puts("KV put failed");
    let message = FakeMessage::new();

    run(&edge, &message).await;

    assert_eq!(*message.rejects.borrow(), vec![RETRY]);
    assert!(message.replies.borrow().is_empty());
    assert_eq!(edge.labels(), vec!["hold failed"]);
}

#[tokio::test]
async fn a_hold_with_no_token_is_refused_as_ours_to_fix_not_the_senders() {
    let edge = FakeEdge::new().gateway_answers(200, json!({"action": "hold", "notice": notice()}));
    let message = FakeMessage::new();

    run(&edge, &message).await;

    assert_eq!(*message.rejects.borrow(), vec![RETRY]);
    assert!(message.replies.borrow().is_empty());
    assert!(edge.puts.borrow().is_empty());
    assert_eq!(edge.labels(), vec!["incoherent verdict"]);
}

#[tokio::test]
async fn a_rejection_carries_the_gateways_own_words_into_the_session() {
    let edge = FakeEdge::new().gateway_answers(
        200,
        json!({"action": "reject", "reason": "dangerous", "bounce": "This looks like phishing"}),
    );
    let message = FakeMessage::new();

    run(&edge, &message).await;

    assert_eq!(*message.rejects.borrow(), vec!["This looks like phishing"]);
}

#[tokio::test]
async fn a_rejection_with_nothing_to_say_still_refuses_the_session() {
    let edge = FakeEdge::new().gateway_answers(200, json!({"action": "reject"}));
    let message = FakeMessage::new();

    run(&edge, &message).await;

    assert_eq!(*message.rejects.borrow(), vec!["Not delivered."]);
}

#[tokio::test]
async fn an_inbox_the_gateway_answers_200_for_but_does_not_know_is_refused_permanently() {
    let edge = FakeEdge::new()
        .gateway_answers(200, json!({"action": "reject", "reason": "unknown_inbox"}));
    let message = FakeMessage::new();

    run(&edge, &message).await;

    assert_eq!(
        *message.rejects.borrow(),
        vec!["No such address at this domain"]
    );
}

//! Driving the router the way a client does, for the route tests: build a
//! request, send it through `oneshot`, read back the status and JSON.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

use crate::db::Db;
use crate::db::challenges::{NewChallenge, create_challenge};
use crate::privy::Clock;

/// The second every route test stands at.
pub(crate) const NOW: i64 = 1_788_868_800;

pub(crate) fn clock_at(now: i64) -> Clock {
    Arc::new(move || now)
}

/// What came back: the status, the body as text, and the body as JSON
/// (`Null` when it was empty or not JSON).
#[derive(Debug)]
pub(crate) struct Answer {
    pub status: StatusCode,
    pub text: String,
    pub body: Value,
}

pub(crate) async fn send(app: Router, request: Request<Body>) -> Answer {
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    Answer { status, text, body }
}

/// A POST of `body` exactly as written, labelled as JSON.
pub(crate) fn post(path: &str, body: &str) -> Request<Body> {
    post_with(path, body, &[])
}

pub(crate) fn post_with(path: &str, body: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut request = Request::post(path).header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.body(Body::from(body.to_owned())).unwrap()
}

pub(crate) fn get(path: &str) -> Request<Body> {
    Request::get(path).body(Body::empty()).unwrap()
}

/// The challenge row every route test seeds, held for fifteen minutes.
pub(crate) fn challenge(token: &str, tier: &str) -> NewChallenge {
    NewChallenge {
        token: token.to_owned(),
        handle: "demo".to_owned(),
        sender: "sender@x.com".to_owned(),
        message_id: format!("0x{}", "ab".repeat(32)),
        tier: tier.to_owned(),
        amount: "1".to_owned(),
        held_until: Some(NOW + 900),
        quote_json: "{}".to_owned(),
        created_at: NOW,
    }
}

pub(crate) async fn seed(db: &Db, challenge: &NewChallenge) {
    create_challenge(db, challenge).await.unwrap();
}

/// The keys of a JSON object in the order they were written, which a parsed
/// `Value` does not keep.
pub(crate) fn keys_in_order(text: &str) -> Vec<String> {
    struct Keys(Vec<String>);

    impl<'de> serde::Deserialize<'de> for Keys {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Keys;
                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("an object")
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Keys, A::Error> {
                    let mut keys = Vec::new();
                    while let Some((key, _)) = map.next_entry::<String, serde::de::IgnoredAny>()? {
                        keys.push(key);
                    }
                    Ok(Keys(keys))
                }
            }
            deserializer.deserialize_map(Visitor)
        }
    }

    serde_json::from_str::<Keys>(text).unwrap().0
}

//! How the TypeScript handlers read a request body, kept where every route can
//! share it: `await request.json()`, destructuring, truthiness, and what the
//! database driver made of a value that was not a string.
//!
//! The handlers cast the parsed body to a typed shape, and a cast checks
//! nothing, so what a caller can actually send is any JSON. These helpers say
//! what that JSON did in JavaScript, which is what the routes answered with.

use std::fmt;

use axum::body::Bytes;
use postage_core::js_number::to_js_string;
use serde_json::Value;

/// What JavaScript threw where the TypeScript did not catch: a `TypeError`
/// from destructuring `null` or calling a string method on something else.
/// Next.js answered it with a bare 500, and so do the routes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeError(pub String);

impl fmt::Display for TypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "TypeError: {}", self.0)
    }
}

impl std::error::Error for TypeError {}

/// `await request.json()`: the body decoded as UTF-8 (a leading byte-order
/// mark dropped and invalid sequences replaced, as `TextDecoder` does), then
/// parsed as any JSON value.
pub(crate) fn request_json(body: &Bytes) -> Result<Value, serde_json::Error> {
    let text = String::from_utf8_lossy(body);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    serde_json::from_str(text)
}

/// `const { name } = body`: a property of the parsed body, `None` where
/// JavaScript reads `undefined`. Destructuring `null` throws; any other
/// non-object simply has no such property.
pub(crate) fn property<'a>(body: &'a Value, name: &str) -> Result<Option<&'a Value>, TypeError> {
    match body {
        Value::Null => Err(TypeError(format!(
            "Cannot destructure property '{name}' of 'body' as it is null."
        ))),
        Value::Object(fields) => Ok(fields.get(name)),
        _ => Ok(None),
    }
}

/// JavaScript truthiness for a value read off a JSON body.
pub(crate) fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// The text a truthy value matched a `TEXT` column as, once the TypeScript
/// handed it to the database driver uncast: a string as itself, a number as
/// SQLite compares one against a text column (its decimal form), `true` as
/// `1`. An array or object cannot be bound at all, and the driver threw.
pub(crate) fn lookup_text(value: &Value) -> Result<String, TypeError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(to_js_string(number.as_f64().unwrap_or(f64::NAN))),
        Value::Bool(true) => Ok("1".to_owned()),
        Value::Bool(false) | Value::Null => Ok("0".to_owned()),
        Value::Array(_) | Value::Object(_) => Err(TypeError(
            "Unsupported type of value passed to the database".to_owned(),
        )),
    }
}

/// A string's `length`: UTF-16 code units, not bytes or characters.
pub(crate) fn js_length(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `String.prototype.trim`: JavaScript's whitespace set, which counts U+FEFF
/// and not U+0085, unlike Rust's `trim`.
pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(|character: char| {
        character == '\u{feff}' || (character.is_whitespace() && character != '\u{85}')
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_body_with_a_byte_order_mark_still_parses() {
        let body = Bytes::from("\u{feff}{\"a\":1}");
        assert_eq!(request_json(&body).unwrap(), json!({ "a": 1 }));
    }

    #[test]
    fn a_body_that_is_not_json_is_an_error() {
        assert!(request_json(&Bytes::from("not json")).is_err());
        assert!(request_json(&Bytes::new()).is_err());
    }

    #[test]
    fn destructuring_null_throws_and_anything_else_reads_undefined() {
        assert!(property(&Value::Null, "token").is_err());
        assert_eq!(property(&json!(5), "token").unwrap(), None);
        assert_eq!(property(&json!("text"), "token").unwrap(), None);
        assert_eq!(
            property(&json!({ "token": "t" }), "token").unwrap(),
            Some(&json!("t"))
        );
    }

    #[test]
    fn truthiness_follows_javascript() {
        for falsy in [json!(null), json!(false), json!(0), json!(-0.0), json!("")] {
            assert!(!is_truthy(Some(&falsy)), "{falsy}");
        }
        for truthy in [json!(true), json!(1), json!("0"), json!([]), json!({})] {
            assert!(is_truthy(Some(&truthy)), "{truthy}");
        }
        assert!(!is_truthy(None));
    }

    #[test]
    fn a_number_is_looked_up_by_its_decimal_text_and_an_object_cannot_be() {
        assert_eq!(lookup_text(&json!(42)).unwrap(), "42");
        assert_eq!(lookup_text(&json!(true)).unwrap(), "1");
        assert!(lookup_text(&json!({ "a": 1 })).is_err());
        assert!(lookup_text(&json!([1])).is_err());
    }

    #[test]
    fn trim_and_length_count_the_way_javascript_does() {
        assert_eq!(js_trim("\u{feff} hi \n"), "hi");
        assert_eq!(js_trim("\u{85}hi"), "\u{85}hi");
        assert_eq!(js_length("é😀"), 3);
    }
}

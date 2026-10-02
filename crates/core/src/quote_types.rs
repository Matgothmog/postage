use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The signed-quote shape as it exists on either side of a serialization
/// boundary: parsed back out of `challenges.quote_json`, or received by a
/// client component. Both crossings lose the typed hex and tier the signer
/// produced, so every field is the plain string that survives the trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteFields {
    pub message_id: String,
    pub inbox: String,
    pub tier: String,
    pub amount: String,
    pub expires_at: u64,
    pub signature: String,
}

/// `QuoteFields` plus the reasons the classifier and pricing gave, stored
/// alongside the quote and shown to the sender on the challenge page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredQuote {
    #[serde(flatten)]
    pub quote: QuoteFields,
    pub reasons: Vec<String>,
}

/// The widths the escrow call built from a quote reads off it: `messageId` as
/// a `bytes32`, `inbox` as an `address`, `signature` as the 65 bytes of r, s
/// and v.
const MESSAGE_ID_HEX_DIGITS: usize = 64;
const ADDRESS_HEX_DIGITS: usize = 40;
const SIGNATURE_HEX_DIGITS: usize = 130;

/// The escrow takes the deadline as a `uint40`.
const MAX_EXPIRES_AT: u64 = (1 << 40) - 1;

/// `0x` followed by exactly `digits` hex digits.
fn is_hex_of_width(value: &str, digits: usize) -> bool {
    value
        .strip_prefix("0x")
        .is_some_and(|rest| rest.len() == digits && rest.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A whole non-negative number written out in digits, the only notation a
/// decimal parse reads the way this column means it: no exponent, sign,
/// radix prefix, separators or surrounding whitespace.
fn is_plain_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

/// An integral JSON number in `0..=2^40-1`; fractions, negatives and anything
/// larger have no escrow call to build.
fn expiry_of(value: &Value) -> Option<u64> {
    let seconds = match value.as_u64() {
        Some(whole) => whole,
        None => {
            let float = value.as_f64()?;
            if float.fract() != 0.0 || float < 0.0 || float > MAX_EXPIRES_AT as f64 {
                return None;
            }
            float as u64
        }
    };
    (seconds <= MAX_EXPIRES_AT).then_some(seconds)
}

fn string_field<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key)?.as_str()
}

fn validated(value: &Value) -> Option<StoredQuote> {
    let object = value.as_object()?;
    let message_id = string_field(object, "messageId")?;
    let inbox = string_field(object, "inbox")?;
    let amount = string_field(object, "amount")?;
    let signature = string_field(object, "signature")?;
    if !is_hex_of_width(message_id, MESSAGE_ID_HEX_DIGITS)
        || !is_hex_of_width(inbox, ADDRESS_HEX_DIGITS)
        || !is_plain_digits(amount)
        || !is_hex_of_width(signature, SIGNATURE_HEX_DIGITS)
    {
        return None;
    }
    let reasons = object
        .get("reasons")?
        .as_array()?
        .iter()
        .map(|reason| reason.as_str().map(str::to_owned))
        .collect::<Option<Vec<String>>>()?;

    Some(StoredQuote {
        quote: QuoteFields {
            message_id: message_id.to_owned(),
            inbox: inbox.to_owned(),
            tier: string_field(object, "tier")?.to_owned(),
            amount: amount.to_owned(),
            expires_at: expiry_of(object.get("expiresAt")?)?,
            signature: signature.to_owned(),
        },
        reasons,
    })
}

/// Turns the raw `challenges.quote_json` column back into a `StoredQuote`, or
/// `None` rather than letting a row an older schema wrote, a truncated write,
/// or anything else that isn't the shape we expect take the page down.
///
/// Values, not only types: everything a caller does with this quote it does by
/// handing a field to something that fails on a string of the right type
/// carrying the wrong value. Fields outside the shape are dropped.
pub fn parse_stored_quote(raw: &str) -> Option<StoredQuote> {
    let value: Value = serde_json::from_str(raw).ok()?;
    validated(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_json() -> Value {
        json!({
            "messageId": format!("0x{}", "ab".repeat(32)),
            "inbox": format!("0x{}", "cd".repeat(20)),
            "tier": "commercial",
            "amount": "1000000",
            "expiresAt": 1_700_000_000u64,
            "signature": format!("0x{}", "ef".repeat(65)),
            "reasons": ["looked automated", "no prior pass"],
        })
    }

    fn with_field(field: &str, value: Value) -> String {
        let mut quote = valid_json();
        quote[field] = value;
        quote.to_string()
    }

    #[test]
    fn a_well_formed_quote_round_trips() {
        let parsed = parse_stored_quote(&valid_json().to_string()).unwrap();

        assert_eq!(serde_json::to_value(&parsed).unwrap(), valid_json());
        assert_eq!(parsed.quote.tier, "commercial");
        assert_eq!(parsed.reasons, ["looked automated", "no prior pass"]);
    }

    #[test]
    fn text_that_is_not_json_returns_none_instead_of_failing() {
        assert_eq!(parse_stored_quote("{not json"), None);
    }

    #[test]
    fn json_that_is_not_an_object_returns_none() {
        for raw in ["\"just a string\"", "42", "null", "[1,2,3]"] {
            assert_eq!(parse_stored_quote(raw), None, "{raw}");
        }
    }

    #[test]
    fn an_object_missing_a_required_field_returns_none() {
        let mut without_signature = valid_json();
        without_signature
            .as_object_mut()
            .unwrap()
            .remove("signature");

        assert_eq!(parse_stored_quote(&without_signature.to_string()), None);
    }

    #[test]
    fn an_object_with_a_field_of_the_wrong_type_returns_none() {
        assert_eq!(
            parse_stored_quote(&with_field("amount", json!(1_000_000))),
            None
        );
    }

    #[test]
    fn a_reasons_array_holding_a_non_string_entry_returns_none() {
        assert_eq!(
            parse_stored_quote(&with_field("reasons", json!(["fine", 7]))),
            None
        );
    }

    #[test]
    fn an_amount_a_decimal_parse_cannot_read_returns_none() {
        for amount in [
            "1e18", "1.5", "", "  12  ", "0x10", "-1", "12n", "one", "1_000",
        ] {
            assert_eq!(
                parse_stored_quote(&with_field("amount", json!(amount))),
                None,
                "amount {amount:?}"
            );
        }
    }

    #[test]
    fn an_amount_of_plain_digits_is_read_however_long_it_is() {
        let wide = "9".repeat(40);
        let parsed = parse_stored_quote(&with_field("amount", json!(wide))).unwrap();

        assert_eq!(parsed.quote.amount, wide);
    }

    #[test]
    fn a_message_id_that_is_not_32_bytes_of_hex_returns_none() {
        let bad = [
            "0xabc".to_owned(),
            "0x".to_owned(),
            String::new(),
            "ab".repeat(32),
            format!("0x{}", "ab".repeat(31)),
            format!("0x{}", "ab".repeat(33)),
            format!("0x{}", "zz".repeat(32)),
        ];
        for message_id in bad {
            assert_eq!(
                parse_stored_quote(&with_field("messageId", json!(message_id))),
                None,
                "messageId {message_id:?}"
            );
        }
    }

    #[test]
    fn an_inbox_that_is_not_a_20_byte_address_returns_none() {
        let bad = [
            "0xdef".to_owned(),
            "0x".to_owned(),
            String::new(),
            format!("0x{}", "cd".repeat(19)),
            format!("0x{}", "cd".repeat(21)),
            format!("0x{}", "gg".repeat(20)),
        ];
        for inbox in bad {
            assert_eq!(
                parse_stored_quote(&with_field("inbox", json!(inbox))),
                None,
                "inbox {inbox:?}"
            );
        }
    }

    #[test]
    fn a_signature_that_is_not_the_65_bytes_a_quote_is_signed_with_returns_none() {
        let bad = [
            "0x1234".to_owned(),
            "0x".to_owned(),
            String::new(),
            "0xabc".to_owned(),
            format!("0x{}", "ef".repeat(64)),
            format!("0x{}", "zz".repeat(65)),
        ];
        for signature in bad {
            assert_eq!(
                parse_stored_quote(&with_field("signature", json!(signature))),
                None,
                "signature {signature:?}"
            );
        }
    }

    #[test]
    fn an_expires_at_outside_the_uint40_the_escrow_takes_returns_none() {
        let bad = [
            json!(-1),
            json!(1.5),
            json!(2u64.pow(40)),
            json!(9_007_199_254_740_991u64),
        ];
        for expires_at in bad {
            assert_eq!(
                parse_stored_quote(&with_field("expiresAt", expires_at.clone())),
                None,
                "expiresAt {expires_at}"
            );
        }
    }

    /// An exponent past what a double holds reads back as infinity in
    /// JavaScript; here the parse itself refuses it. Either way no quote.
    #[test]
    fn an_expires_at_that_overflows_a_double_returns_none() {
        let overflowed = valid_json()
            .to_string()
            .replace("\"expiresAt\":1700000000", "\"expiresAt\":1e400");

        assert!(
            overflowed.contains("1e400"),
            "precondition: row was rewritten"
        );
        assert_eq!(parse_stored_quote(&overflowed), None);
    }
}

use std::fmt;

/// Whatever a failure carried. A JS `catch` hands back anything that was
/// thrown, and this models the shapes that matter: a genuine error, or some
/// other value that is simply stringified.
#[derive(Debug, Clone, PartialEq)]
pub enum Cause {
    /// A genuine error; its message is what a caller wants surfaced.
    Error(String),
    Text(String),
    Number(f64),
    Object,
    Null,
    Undefined,
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Cause::Error(message) | Cause::Text(message) => f.write_str(message),
            Cause::Number(number) => write!(f, "{number}"),
            Cause::Object => f.write_str("[object Object]"),
            Cause::Null => f.write_str("null"),
            Cause::Undefined => f.write_str("undefined"),
        }
    }
}

impl<E: std::error::Error> From<&E> for Cause {
    fn from(error: &E) -> Self {
        Cause::Error(error.to_string())
    }
}

/// The one place the message-of-a-failure decision is made, so every caller
/// gets the same answer: an error yields its message, anything else is
/// stringified.
pub fn cause_message(cause: &Cause) -> String {
    cause.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("quote expired")]
    struct QuoteExpiredError;

    #[test]
    fn an_error_yields_its_message() {
        assert_eq!(
            cause_message(&Cause::Error("db unreachable".into())),
            "db unreachable"
        );
    }

    #[test]
    fn a_typed_error_yields_its_message() {
        assert_eq!(
            cause_message(&Cause::from(&QuoteExpiredError)),
            "quote expired"
        );
    }

    #[test]
    fn a_thrown_string_is_stringified_rather_than_treated_as_an_error() {
        assert_eq!(cause_message(&Cause::Text("timed out".into())), "timed out");
    }

    #[test]
    fn a_thrown_plain_object_is_stringified() {
        assert_eq!(cause_message(&Cause::Object), "[object Object]");
    }

    #[test]
    fn a_thrown_number_is_stringified() {
        assert_eq!(cause_message(&Cause::Number(42.0)), "42");
    }

    #[test]
    fn a_thrown_null_is_stringified() {
        assert_eq!(cause_message(&Cause::Null), "null");
    }

    #[test]
    fn a_thrown_undefined_is_stringified() {
        assert_eq!(cause_message(&Cause::Undefined), "undefined");
    }
}

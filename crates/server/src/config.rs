//! Configuration read from the process environment.
//!
//! Every reader takes the lookup as a parameter rather than calling
//! `std::env` itself, so tests hand in a map instead of mutating the real
//! environment underneath other tests running in parallel. Production passes
//! [`process_env`].

use postage_core::secret::{MessageIdSecret, SecretError};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("{0} is not set")]
    Missing(&'static str),
    #[error(
        "IDENTITY_MODE is set to an unrecognised value: \"{0}\". Expected \"live\" or \"mock\"."
    )]
    UnrecognisedIdentityMode(String),
    #[error(transparent)]
    Secret(#[from] SecretError),
}

/// The real environment. A value that is not valid UTF-8 is read lossily, so
/// it surfaces as an unrecognised value rather than passing as unset.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var_os(name).map(|value| value.to_string_lossy().into_owned())
}

/// Fails loudly at the call site rather than letting a missing value reach an
/// API. Empty counts as missing.
pub fn required<F>(env: F, name: &'static str) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    env(name)
        .filter(|value| !value.is_empty())
        .ok_or(ConfigError::Missing(name))
}

/// The root secret every purpose-bound key is derived from.
pub fn message_id_secret<F>(env: F) -> Result<MessageIdSecret, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    Ok(MessageIdSecret::new(required(env, "MESSAGE_ID_SECRET")?)?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityMode {
    Live,
    Mock,
}

/// Unset (or empty) defaults to live: the safer failure mode for an
/// identity-verification gate is to require a real proof, not to skip it.
/// Anything else must be exactly "live" or "mock". A near-miss (wrong case,
/// stray whitespace, a typo) is an error rather than being coerced to
/// whichever value it resembles: an operator who typed "Mock " meant mock, and
/// silently resolving that to live would be the dangerous outcome.
pub fn identity_mode<F>(env: F) -> Result<IdentityMode, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match env("IDENTITY_MODE").as_deref() {
        None | Some("") | Some("live") => Ok(IdentityMode::Live),
        Some("mock") => Ok(IdentityMode::Mock),
        Some(other) => Err(ConfigError::UnrecognisedIdentityMode(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_identity_mode(value: Option<&str>) -> impl Fn(&str) -> Option<String> {
        let value = value.map(str::to_owned);
        move |name| match name {
            "IDENTITY_MODE" => value.clone(),
            _ => None,
        }
    }

    fn unrecognised(value: &str) -> Result<IdentityMode, ConfigError> {
        Err(ConfigError::UnrecognisedIdentityMode(value.to_owned()))
    }

    #[test]
    fn defaults_to_live_when_identity_mode_is_unset() {
        assert_eq!(
            identity_mode(with_identity_mode(None)),
            Ok(IdentityMode::Live)
        );
    }

    #[test]
    fn defaults_to_live_when_identity_mode_is_the_empty_string() {
        assert_eq!(
            identity_mode(with_identity_mode(Some(""))),
            Ok(IdentityMode::Live)
        );
    }

    #[test]
    fn returns_live_for_the_exact_value_live() {
        assert_eq!(
            identity_mode(with_identity_mode(Some("live"))),
            Ok(IdentityMode::Live)
        );
    }

    #[test]
    fn returns_mock_for_the_explicit_value_mock() {
        assert_eq!(
            identity_mode(with_identity_mode(Some("mock"))),
            Ok(IdentityMode::Mock)
        );
    }

    #[test]
    fn rejects_a_capitalised_near_miss_instead_of_silently_resolving_to_live() {
        assert_eq!(
            identity_mode(with_identity_mode(Some("Live"))),
            unrecognised("Live")
        );
    }

    #[test]
    fn rejects_an_all_caps_near_miss_instead_of_silently_resolving_to_live() {
        assert_eq!(
            identity_mode(with_identity_mode(Some("LIVE"))),
            unrecognised("LIVE")
        );
    }

    #[test]
    fn rejects_a_trailing_space_near_miss_instead_of_silently_resolving_to_live() {
        assert_eq!(
            identity_mode(with_identity_mode(Some("live "))),
            unrecognised("live ")
        );
    }

    #[test]
    fn rejects_an_unrelated_value_instead_of_silently_resolving_to_live() {
        assert_eq!(
            identity_mode(with_identity_mode(Some("production"))),
            unrecognised("production")
        );
    }

    #[test]
    fn names_both_the_variable_and_the_offending_value_in_the_error() {
        let message = match identity_mode(with_identity_mode(Some("Live"))) {
            Ok(mode) => format!("unexpectedly resolved to {mode:?}"),
            Err(error) => error.to_string(),
        };
        assert!(message.contains("IDENTITY_MODE"), "{message}");
        assert!(message.contains("\"Live\""), "{message}");
    }

    #[test]
    fn required_returns_a_set_value() {
        let env = |name: &str| (name == "A").then(|| "value".to_owned());
        assert_eq!(required(env, "A"), Ok("value".to_owned()));
    }

    #[test]
    fn required_treats_unset_and_empty_alike_and_names_the_variable() {
        assert_eq!(required(|_| None, "A"), Err(ConfigError::Missing("A")));
        assert_eq!(
            required(|_| Some(String::new()), "A"),
            Err(ConfigError::Missing("A"))
        );
        assert_eq!(ConfigError::Missing("A").to_string(), "A is not set");
    }

    #[test]
    fn a_missing_message_id_secret_is_reported_without_a_value() {
        assert_eq!(
            message_id_secret(|_| None).map(|_| ()),
            Err(ConfigError::Missing("MESSAGE_ID_SECRET"))
        );
    }

    #[test]
    fn a_set_message_id_secret_is_accepted() {
        let env = |_: &str| Some("x".repeat(32));
        assert!(message_id_secret(env).is_ok());
    }
}

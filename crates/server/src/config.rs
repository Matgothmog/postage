//! Configuration read from the process environment.
//!
//! Every reader takes the lookup as a parameter rather than calling
//! `std::env` itself, so tests hand in a map instead of mutating the real
//! environment underneath other tests running in parallel. Production passes
//! [`process_env`].

use alloy_signer_local::PrivateKeySigner;
use postage_core::quote::{QuoteSigner, private_key_bytes};
use postage_core::secret::{MessageIdSecret, SecretError};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("{0} is not set")]
    Missing(&'static str),
    #[error(
        "IDENTITY_MODE is set to an unrecognised value: \"{0}\". Expected \"live\" or \"mock\"."
    )]
    UnrecognisedIdentityMode(String),
    /// Set, but to something that cannot be what the name says. The value is
    /// left out: several of these are keys.
    #[error("{0} is set but is not valid")]
    Invalid(&'static str),
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

/// The key quotes are signed with; the escrow only honours quotes from the
/// address it has registered for it.
pub fn classifier_signer<F>(env: F) -> Result<QuoteSigner, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let key = required(env, "CLASSIFIER_PRIVATE_KEY")?;
    QuoteSigner::from_hex(&key).map_err(|_| ConfigError::Invalid("CLASSIFIER_PRIVATE_KEY"))
}

/// The key that pays gas for attestations, read the same way viem reads it.
pub fn relayer_signer<F>(env: F) -> Result<PrivateKeySigner, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let invalid = || ConfigError::Invalid("RELAYER_PRIVATE_KEY");
    let bytes = private_key_bytes(&required(env, "RELAYER_PRIVATE_KEY")?).map_err(|_| invalid())?;
    PrivateKeySigner::from_slice(&bytes).map_err(|_| invalid())
}

/// The Privy app identity tokens must be issued for, and whose JWKS signs them.
/// The name keeps its `NEXT_PUBLIC_` prefix because the same value configures
/// the browser SDK.
pub fn privy_app_id<F>(env: F) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    required(env, "NEXT_PUBLIC_PRIVY_APP_ID")
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
    fn privy_app_id_is_required() {
        assert_eq!(
            privy_app_id(|_| None),
            Err(ConfigError::Missing("NEXT_PUBLIC_PRIVY_APP_ID"))
        );
        let env = |name: &str| (name == "NEXT_PUBLIC_PRIVY_APP_ID").then(|| "app".to_owned());
        assert_eq!(privy_app_id(env), Ok("app".to_owned()));
    }

    #[test]
    fn a_missing_message_id_secret_is_reported_without_a_value() {
        assert_eq!(
            message_id_secret(|_| None).map(|_| ()),
            Err(ConfigError::Missing("MESSAGE_ID_SECRET"))
        );
    }

    fn with_key(name: &'static str, value: &str) -> impl Fn(&str) -> Option<String> {
        let value = value.to_owned();
        move |asked| (asked == name).then(|| value.clone())
    }

    #[test]
    fn a_classifier_key_is_read_into_a_signer() {
        let key = format!("0x{}", "11".repeat(32));
        let signer = classifier_signer(with_key("CLASSIFIER_PRIVATE_KEY", &key)).unwrap();
        assert_eq!(
            signer.address().to_checksum(None),
            "0x19E7E376E7C213B7E7e7e46cc70A5dD086DAff2A"
        );
    }

    #[test]
    fn a_missing_or_malformed_classifier_key_is_named_without_its_value() {
        assert_eq!(
            classifier_signer(|_| None).map(|_| ()),
            Err(ConfigError::Missing("CLASSIFIER_PRIVATE_KEY"))
        );
        let error = classifier_signer(with_key("CLASSIFIER_PRIVATE_KEY", "0xdeadbeef"))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(error, ConfigError::Invalid("CLASSIFIER_PRIVATE_KEY"));
        assert!(!error.to_string().contains("deadbeef"));
    }

    #[test]
    fn a_relayer_key_is_read_into_a_signer_with_the_same_address_viem_derives() {
        let key = format!("0x{}", "11".repeat(32));
        let signer = relayer_signer(with_key("RELAYER_PRIVATE_KEY", &key)).unwrap();
        assert_eq!(
            signer.address().to_checksum(None),
            "0x19E7E376E7C213B7E7e7e46cc70A5dD086DAff2A"
        );
    }

    #[test]
    fn a_relayer_key_without_its_prefix_or_of_zero_is_invalid() {
        for key in ["11".repeat(32), format!("0x{}", "00".repeat(32))] {
            assert_eq!(
                relayer_signer(with_key("RELAYER_PRIVATE_KEY", &key)).map(|_| ()),
                Err(ConfigError::Invalid("RELAYER_PRIVATE_KEY")),
                "{key}"
            );
        }
    }

    #[test]
    fn a_set_message_id_secret_is_accepted() {
        let env = |_: &str| Some("x".repeat(32));
        assert!(message_id_secret(env).is_ok());
    }
}

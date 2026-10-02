//! The worker's configuration: two vars from `wrangler.toml` and three secrets.

/// Names are the ones `worker/wrangler.toml` and `wrangler secret put` use.
pub const POSTAGE_API_URL: &str = "POSTAGE_API_URL";
pub const POSTAGE_SECRET: &str = "POSTAGE_SECRET";
pub const MAILGUN_API_BASE: &str = "MAILGUN_API_BASE";
pub const MAILGUN_DOMAIN: &str = "MAILGUN_DOMAIN";
pub const MAILGUN_API_KEY: &str = "MAILGUN_API_KEY";

#[derive(Clone, PartialEq, Eq)]
pub struct Settings {
    pub api_url: String,
    pub secret: String,
    pub mailgun_api_base: String,
    pub mailgun_domain: String,
    pub mailgun_api_key: String,
}

/// Which setting was absent or empty. Names the variable only, never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{0} is not set")]
pub struct MissingSetting(pub &'static str);

impl Settings {
    /// An empty value counts as missing. For the shared secret that is a
    /// deliberate tightening: an empty `POSTAGE_SECRET` would otherwise let a
    /// request with an empty header through.
    pub fn load(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, MissingSetting> {
        let required = |name: &'static str| {
            lookup(name)
                .filter(|value| !value.is_empty())
                .ok_or(MissingSetting(name))
        };
        Ok(Self {
            api_url: required(POSTAGE_API_URL)?,
            secret: required(POSTAGE_SECRET)?,
            mailgun_api_base: required(MAILGUN_API_BASE)?,
            mailgun_domain: required(MAILGUN_DOMAIN)?,
            mailgun_api_key: required(MAILGUN_API_KEY)?,
        })
    }
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings")
            .field("api_url", &self.api_url)
            .field("mailgun_api_base", &self.mailgun_api_base)
            .field("mailgun_domain", &self.mailgun_domain)
            .field("secret", &"<redacted>")
            .field("mailgun_api_key", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full(name: &str) -> Option<String> {
        Some(format!("value-of-{name}"))
    }

    #[test]
    fn every_setting_present_loads() {
        let settings = Settings::load(full).unwrap();
        assert_eq!(settings.mailgun_domain, "value-of-MAILGUN_DOMAIN");
    }

    #[test]
    fn a_missing_setting_is_named() {
        let result = Settings::load(|name| (name != MAILGUN_API_KEY).then(|| "x".to_owned()));
        assert_eq!(result.unwrap_err(), MissingSetting(MAILGUN_API_KEY));
    }

    #[test]
    fn an_empty_secret_counts_as_missing() {
        let result = Settings::load(|name| {
            Some(if name == POSTAGE_SECRET {
                String::new()
            } else {
                "x".into()
            })
        });
        assert_eq!(result.unwrap_err(), MissingSetting(POSTAGE_SECRET));
    }

    #[test]
    fn debug_output_hides_the_secrets() {
        let shown = format!("{:?}", Settings::load(full).unwrap());
        assert!(!shown.contains("value-of-POSTAGE_SECRET"));
        assert!(!shown.contains("value-of-MAILGUN_API_KEY"));
    }
}

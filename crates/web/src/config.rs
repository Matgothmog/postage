//! Client configuration, fixed at compile time. Replaces the `NEXT_PUBLIC_*`
//! values Next inlined into the browser bundle of `web/`.
//!
//! The variable names are kept as they were, so an environment that already
//! builds the Next app (Vercel's project env, a developer's shell) builds this
//! one unchanged: `NEXT_PUBLIC_PRIVY_APP_ID=... trunk build`. They are public
//! client ids, never secrets: everything here ships in the wasm binary.
//!
//! The checks run as `const` assertions, so a bad value is a compile error
//! naming the variable rather than a broken page:
//! - a release build (`trunk build --release`) needs both app ids;
//! - a debug build may leave them unset, and the app then reports itself
//!   unconfigured at run time (the setup notice of `web/src/app/providers.tsx`);
//! - a World app id must start `app_`, and the World environment must be one
//!   IDKit accepts, in every build;
//! - a Vercel production build refuses a non-production World environment
//!   (the guard of `web/next.config.ts`).
//!
//! `NEXT_PUBLIC_WORLD_ACTION` is deliberately not read: the action IDKit is
//! given comes from the signed rp_context (`bridge::idkit::RpContext`), so the
//! server's signature and the browser's request can never name different
//! actions (see `web/src/lib/world-id.ts`).

/// The Privy app id, or `None` in a debug build that was not given one.
pub const PRIVY_APP_ID: Option<&str> = non_empty(option_env!("NEXT_PUBLIC_PRIVY_APP_ID"));

/// The World ID app id (`app_...`), or `None` in a debug build that was not
/// given one.
pub const WORLD_APP_ID: Option<&str> = non_empty(option_env!("NEXT_PUBLIC_WORLD_APP_ID"));

const WORLD_ENVIRONMENT_PARSED: Option<WorldEnvironment> =
    WorldEnvironment::from_build_value(option_env!("NEXT_PUBLIC_WORLD_ENVIRONMENT"));

/// Which World environment IDKit requests target. Unset means production,
/// IDKit's own default.
pub const WORLD_ENVIRONMENT: WorldEnvironment = match WORLD_ENVIRONMENT_PARSED {
    Some(environment) => environment,
    None => WorldEnvironment::Production,
};

const _: () = assert!(
    WORLD_ENVIRONMENT_PARSED.is_some(),
    "NEXT_PUBLIC_WORLD_ENVIRONMENT must be \"production\", \"staging\", \"sandbox\" or unset"
);

const _: () = assert!(
    match WORLD_APP_ID {
        Some(id) => is_world_app_id(id),
        None => true,
    },
    "NEXT_PUBLIC_WORLD_APP_ID must start with \"app_\""
);

const _: () = assert!(
    production_environment_allowed(option_env!("VERCEL_ENV"), WORLD_ENVIRONMENT),
    "Refusing a Vercel production build: NEXT_PUBLIC_WORLD_ENVIRONMENT is not \"production\". \
     The value travels inside the encrypted IDKit request, so World would be asked to accept \
     non-production Selfie Check proofs in production with nothing downstream to catch it."
);

#[cfg(not(debug_assertions))]
const _: () = assert!(
    PRIVY_APP_ID.is_some(),
    "NEXT_PUBLIC_PRIVY_APP_ID must be set for a release build"
);

#[cfg(not(debug_assertions))]
const _: () = assert!(
    WORLD_APP_ID.is_some(),
    "NEXT_PUBLIC_WORLD_APP_ID must be set for a release build"
);

/// The values `IDKitRequestConfig.environment` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WorldEnvironment {
    Production,
    Staging,
    Sandbox,
}

impl WorldEnvironment {
    /// Unset or empty is production; anything other than the three accepted
    /// spellings is `None`, never coerced, since a silent fallback to
    /// production is the sandbox/production mismatch this value exists to
    /// catch.
    pub const fn from_build_value(raw: Option<&str>) -> Option<Self> {
        let Some(raw) = raw else {
            return Some(Self::Production);
        };
        if raw.is_empty() || str_eq(raw, "production") {
            Some(Self::Production)
        } else if str_eq(raw, "staging") {
            Some(Self::Staging)
        } else if str_eq(raw, "sandbox") {
            Some(Self::Sandbox)
        } else {
            None
        }
    }
}

/// Only Vercel's production build (`VERCEL_ENV=production`) is held to a
/// production World environment; local and preview builds may use sandbox.
pub const fn production_environment_allowed(
    vercel_env: Option<&str>,
    environment: WorldEnvironment,
) -> bool {
    match vercel_env {
        Some(env) if str_eq(env, "production") => {
            matches!(environment, WorldEnvironment::Production)
        }
        _ => true,
    }
}

pub const fn is_world_app_id(id: &str) -> bool {
    let id = id.as_bytes();
    let prefix = b"app_";
    if id.len() <= prefix.len() {
        return false;
    }
    let mut index = 0;
    while index < prefix.len() {
        if id[index] != prefix[index] {
            return false;
        }
        index += 1;
    }
    true
}

const fn non_empty(value: Option<&'static str>) -> Option<&'static str> {
    match value {
        Some(text) if !text.is_empty() => Some(text),
        _ => None,
    }
}

const fn str_eq(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_or_empty_environment_is_production() {
        assert_eq!(
            WorldEnvironment::from_build_value(None),
            Some(WorldEnvironment::Production)
        );
        assert_eq!(
            WorldEnvironment::from_build_value(Some("")),
            Some(WorldEnvironment::Production)
        );
    }

    #[test]
    fn the_three_idkit_environments_parse() {
        assert_eq!(
            WorldEnvironment::from_build_value(Some("staging")),
            Some(WorldEnvironment::Staging)
        );
        assert_eq!(
            WorldEnvironment::from_build_value(Some("sandbox")),
            Some(WorldEnvironment::Sandbox)
        );
        assert_eq!(
            WorldEnvironment::from_build_value(Some("production")),
            Some(WorldEnvironment::Production)
        );
    }

    #[test]
    fn a_near_miss_environment_is_refused_not_coerced() {
        assert_eq!(WorldEnvironment::from_build_value(Some("Sandbox")), None);
        assert_eq!(WorldEnvironment::from_build_value(Some("prod")), None);
    }

    #[test]
    fn environment_serializes_as_idkit_spells_it() {
        assert_eq!(
            serde_json::to_string(&WorldEnvironment::Sandbox).unwrap(),
            "\"sandbox\""
        );
    }

    #[test]
    fn vercel_production_requires_the_production_environment() {
        assert!(!production_environment_allowed(
            Some("production"),
            WorldEnvironment::Sandbox
        ));
        assert!(production_environment_allowed(
            Some("production"),
            WorldEnvironment::Production
        ));
    }

    #[test]
    fn preview_and_local_builds_may_use_sandbox() {
        assert!(production_environment_allowed(
            Some("preview"),
            WorldEnvironment::Sandbox
        ));
        assert!(production_environment_allowed(
            None,
            WorldEnvironment::Sandbox
        ));
    }

    #[test]
    fn world_app_ids_need_the_app_prefix_and_a_body() {
        assert!(is_world_app_id("app_abc"));
        assert!(!is_world_app_id("app_"));
        assert!(!is_world_app_id("abc"));
        assert!(!is_world_app_id("APP_abc"));
    }
}

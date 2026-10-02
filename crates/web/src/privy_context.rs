//! Sharing the one running `Privy` through Leptos context.
//!
//! `Privy::start` may run once per page, so the root component starts it and
//! provides the handle; everything below reads it with `use_privy`.

use leptos::prelude::*;

use crate::bridge::privy::Privy;

/// The page's Privy handle. Outside a provider (a component rendered on its
/// own) this is a detached one: not ready, signed out, wired to no SDK.
pub fn use_privy() -> Privy {
    use_context::<Privy>().unwrap_or_else(Privy::detached)
}

/// Opens the Privy login modal from an event handler. Closing the modal is
/// not a failure; anything else is logged, as no screen has a better place
/// to put it.
pub fn start_login(privy: Privy) {
    leptos::task::spawn_local(async move {
        if let Err(error) = privy.login().await
            && !error.is_user_cancel()
        {
            leptos::logging::error!("sign-in failed: {error}");
        }
    });
}

//! One component per route.

use leptos::prelude::*;
use leptos_router::components::A;

use crate::account::Account;
use crate::chrome::QUIET_BUTTON;
use crate::landing::Landing;

/// `/network`, which has its own module: the page, its fetch and its timer.
pub use crate::network_page::NetworkPage;

/// `/`: the inbox for a signed-in owner, the pitch for everyone else.
/// Until Privy reports `ready` the pitch shows, so nobody waits on a wallet
/// library to read it.
#[component]
pub fn HomePage() -> impl IntoView {
    view! { <Account landing=ViewFn::from(|| view! { <Landing /> }) /> }
}

#[component]
pub(crate) fn GetAnAddress() -> impl IntoView {
    view! {
        <A href="/" attr:class=QUIET_BUTTON>
            "Get an address"
        </A>
    }
}

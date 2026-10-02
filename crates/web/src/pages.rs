//! One component per route.

use leptos::prelude::*;
use leptos_router::components::A;

use crate::account::Account;
use crate::chrome::{QUIET_BUTTON, Shell};
use crate::landing::Landing;

/// `/`: the inbox for a signed-in owner, the pitch for everyone else.
/// Until Privy reports `ready` the pitch shows, so nobody waits on a wallet
/// library to read it.
#[component]
pub fn HomePage() -> impl IntoView {
    view! { <Account landing=ViewFn::from(|| view! { <Landing /> }) /> }
}

/// `/network`: the public ledger.
#[component]
pub fn NetworkPage() -> impl IntoView {
    view! {
        <Shell actions=GetAnAddress>
            <main class="mx-auto w-full max-w-5xl px-6 py-16">
                <h1 class="text-2xl font-semibold tracking-[-0.02em] text-fg">"Ledger"</h1>
            </main>
        </Shell>
    }
}

#[component]
fn GetAnAddress() -> impl IntoView {
    view! {
        <A href="/" attr:class=QUIET_BUTTON>
            "Get an address"
        </A>
    }
}

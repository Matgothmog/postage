//! One component per route.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

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

/// `/c/:token`: the challenge a stranger gets in a bounce email.
#[component]
pub fn ChallengePage() -> impl IntoView {
    let params = use_params_map();
    let token = move || params.read().get("token").unwrap_or_default();
    view! {
        <Shell actions=GetAnAddress>
            <main class="mx-auto w-full max-w-xl px-6 py-16" data-token=token>
                <h1 class="text-2xl font-semibold tracking-[-0.02em] text-fg">"Challenge"</h1>
            </main>
        </Shell>
    }
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

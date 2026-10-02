//! One component per route.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use postage_core::format::short_address;

use crate::chrome::{QUIET_BUTTON, Shell};
use crate::landing::Landing;
use crate::privy_context::{start_login, use_privy};

/// `/`: the inbox for a signed-in owner, the pitch for everyone else.
/// Until Privy reports `ready` the pitch shows, so nobody waits on a wallet
/// library to read it.
#[component]
pub fn HomePage() -> impl IntoView {
    let privy = use_privy();
    let signed_in = move || privy.ready().get() && privy.authenticated().get();
    move || {
        if signed_in() {
            view! { <AccountPage /> }.into_any()
        } else {
            view! {
                <Shell actions=SignedOutActions>
                    <Landing />
                </Shell>
            }
            .into_any()
        }
    }
}

#[component]
fn SignedOutActions() -> impl IntoView {
    let privy = use_privy();
    view! {
        <A href="/network" attr:class=QUIET_BUTTON>
            "Ledger"
        </A>
        <button
            type="button"
            class=QUIET_BUTTON
            disabled=move || !privy.ready().get()
            on:click=move |_| start_login(privy)
        >
            "Sign in"
        </button>
    }
}

/// The signed-in home screen. A stand-in until the inbox, claim flow and
/// pending-claim recovery land.
#[component]
pub fn AccountPage() -> impl IntoView {
    let privy = use_privy();
    let wallet = move || {
        privy
            .wallet()
            .get()
            .map(|wallet| short_address(&wallet.to_string()))
    };
    view! {
        <Shell actions=SignedInActions>
            <main class="mx-auto w-full max-w-3xl px-6 py-16">
                <p class="text-sm text-muted">
                    "Signed in as " <span class="font-mono text-fg">{wallet}</span>
                </p>
            </main>
        </Shell>
    }
}

#[component]
fn SignedInActions() -> impl IntoView {
    let privy = use_privy();
    view! {
        <A href="/network" attr:class=QUIET_BUTTON>
            "Ledger"
        </A>
        <button
            type="button"
            class=QUIET_BUTTON
            on:click=move |_| {
                leptos::task::spawn_local(async move {
                    if let Err(error) = privy.logout().await {
                        leptos::logging::error!("sign-out failed: {error}");
                    }
                });
            }
        >
            "Sign out"
        </button>
    }
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

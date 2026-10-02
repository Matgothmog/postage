//! The root component: starts Privy once, provides it, and routes.
//! Replaces `web/src/app/layout.tsx` and `providers.tsx`.
//!
//! The document shell (`<html>` classes, title, theme colour, fonts) lives in
//! `index.html`. Deployment must serve `index.html` for every path that is not
//! a file (`/c/<token>`, `/network`, anything else), or a reload or a pasted
//! link 404s before this router ever runs.

use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use crate::bridge::privy::{Privy, PrivyConfig};
use crate::challenge_page::ChallengePage;
use crate::config::PRIVY_APP_ID;
use crate::pages::{HomePage, NetworkPage};
use crate::screens::{ErrorGuard, NotFound};

#[component]
pub fn App() -> impl IntoView {
    let Some(app_id) = PRIVY_APP_ID else {
        return view! { <SetupNotice message="NEXT_PUBLIC_PRIVY_APP_ID is not set." /> }.into_any();
    };
    match Privy::start(&PrivyConfig::new(app_id)) {
        Ok(privy) => {
            provide_context(privy);
            view! { <AppRoutes /> }.into_any()
        }
        Err(error) => view! {
            <SetupNotice message=format!("Postage could not start its sign-in library: {error}") />
        }
        .into_any(),
    }
}

/// The routes and their error guard; the root minus Privy, so tests can mount
/// it over a mock bridge.
#[component]
pub fn AppRoutes() -> impl IntoView {
    view! {
        <Router>
            <ErrorGuard>
                <Routes fallback=NotFound>
                    <Route path=path!("/") view=HomePage />
                    <Route path=path!("/c/:token") view=ChallengePage />
                    <Route path=path!("/network") view=NetworkPage />
                </Routes>
            </ErrorGuard>
        </Router>
    }
}

/// Shown instead of the app when it cannot be configured (`providers.tsx`'s
/// missing-app-id screen).
#[component]
fn SetupNotice(#[prop(into)] message: String) -> impl IntoView {
    view! {
        <main class="m-auto max-w-md p-8 text-sm">
            <p class="font-medium">{message}</p>
            <p class="mt-2 text-muted">
                "Set the NEXT_PUBLIC_PRIVY_APP_ID build variable to your Privy app id and rebuild."
            </p>
        </main>
    }
}

//! The two full-page dead ends: the 404 and the error screen. Replace
//! `web/src/app/not-found.tsx` and `error.tsx`.

use leptos::prelude::*;
use leptos_router::components::A;

use crate::chrome::{PRIMARY_BUTTON, SECONDARY_BUTTON, Shell};

#[component]
pub fn NotFound() -> impl IntoView {
    view! {
        <Shell>
            <main class="mx-auto flex w-full max-w-5xl flex-col items-center gap-6 px-6 py-24 text-center sm:py-32">
                <p class="text-[11px] font-semibold uppercase tracking-[0.2em] text-accent">
                    "404"
                </p>
                <h1 class="text-[2.25rem] font-semibold tracking-[-0.03em] text-fg sm:text-[2.75rem]">
                    "Nothing here."
                </h1>
                <p class="text-sm text-muted">"Dead link. The rest of your mail is fine."</p>
                <A href="/" attr:class=PRIMARY_BUTTON>
                    "Go home"
                </A>
            </main>
        </Shell>
    }
}

/// What the user sees when a screen fails. `on_retry` re-renders the failed
/// screen (Next's `reset`).
#[component]
pub fn ErrorScreen(on_retry: Callback<()>) -> impl IntoView {
    view! {
        <Shell>
            <main class="mx-auto flex w-full max-w-5xl flex-col items-center gap-6 px-6 py-24 text-center sm:py-32">
                <p class="text-[11px] font-semibold uppercase tracking-[0.2em] text-bad">
                    "Error"
                </p>
                <h1 class="text-[2.25rem] font-semibold tracking-[-0.03em] text-fg sm:text-[2.75rem]">
                    "Lost in transit."
                </h1>
                <p class="text-sm text-muted">"Not your fault. Try again."</p>
                <div class="flex flex-wrap items-center justify-center gap-3">
                    <button type="button" class=PRIMARY_BUTTON on:click=move |_| on_retry.run(())>
                        "Try again"
                    </button>
                    <A href="/" attr:class=SECONDARY_BUTTON>
                        "Go home"
                    </A>
                </div>
            </main>
        </Shell>
    }
}

/// Shows `ErrorScreen` in place of `children` when a child view yields an
/// `Err`, and logs the error. "Try again" builds the children afresh.
///
/// A Rust panic is not an `Err`: it aborts the wasm instance and nothing here
/// can catch it, so unlike Next's boundary this covers returned errors only.
#[component]
pub fn ErrorGuard(children: ChildrenFn) -> impl IntoView {
    let attempt = RwSignal::new(0_u32);
    let retry = Callback::new(move |()| attempt.update(|attempt| *attempt += 1));
    move || {
        attempt.track();
        let children = children.clone();
        view! {
            <ErrorBoundary fallback=move |errors| {
                Effect::new(move |_| leptos::logging::error!("{:?}", errors.get()));
                view! { <ErrorScreen on_retry=retry /> }
            }>{children()}</ErrorBoundary>
        }
    }
}

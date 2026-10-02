//! The page a stranger opens. Replaces `web/src/app/Landing.tsx`.

use leptos::prelude::*;
use leptos_router::components::A;
use postage_core::handle::{is_valid_handle, postage_address};

use crate::account::ClaimFlow;
use crate::chrome::{AddressCard, PRIMARY_BUTTON};
use crate::claim_strip::HandleField;

/// The whole product as four outcomes: what happens to each kind of mail.
const WHO_PAYS: [(&str, &str, &str); 4] = [
    ("Codes and receipts", "free, always", "text-good"),
    ("Real people", "free, once proven", "text-good"),
    ("Newsletters and bots", "they pay you", "text-accent-hi"),
    ("Phishing", "never arrives", "text-bad"),
];

#[component]
pub fn Landing() -> impl IntoView {
    view! {
        <main>
            <section class="glow">
                <div class="mx-auto grid w-full max-w-5xl gap-14 px-6 pt-20 pb-24 lg:grid-cols-[1.15fr_auto] lg:items-center lg:gap-20">
                    <div class="rise">
                        <h1 class="text-[3.25rem] leading-[0.95] font-semibold tracking-[-0.04em] text-fg sm:text-[4.25rem]">
                            "Make spam pay."
                        </h1>
                        <p class="mt-6 max-w-xl text-[19px] leading-relaxed text-muted">
                            "One address. Humans get through free. Machines pay you in USDC."
                        </p>

                        <ClaimHero />

                        <div class="mt-5 flex flex-wrap items-center gap-x-5 gap-y-2">
                            <p class="text-xs text-faint">"Free. No card. Keep your inbox."</p>
                            <A
                                href="/network"
                                attr:class="text-xs text-accent-hi transition hover:text-accent"
                            >
                                "See the money →"
                            </A>
                        </div>
                    </div>

                    <div class="flex justify-center lg:justify-end">
                        <AddressCard
                            handle=postage_address("you")
                            price="$0.01"
                            caption="What a stranger pays to reach you"
                        />
                    </div>
                </div>
            </section>

            <section class="border-t border-line">
                <div class="mx-auto w-full max-w-5xl px-6 py-16">
                    <h2 class="text-2xl font-semibold tracking-[-0.02em] text-fg">"Who pays."</h2>
                    <div class="mt-6 divide-y divide-line border-y border-line">
                        {WHO_PAYS
                            .into_iter()
                            .map(|(who, outcome, tone)| {
                                view! {
                                    <p class="flex flex-wrap items-baseline gap-x-2 py-4 text-[15px]">
                                        <span class="text-fg">{who}</span>
                                        <span class="text-faint" aria-hidden="true">
                                            "—"
                                        </span>
                                        <span class=tone>{outcome}</span>
                                    </p>
                                }
                            })
                            .collect_view()}
                    </div>
                </div>
            </section>
        </main>
    }
}

/// The one part of the landing page that needs a session, and the only click
/// the short path asks for: the handle, then "Claim it". It reads the claim
/// flow out of context rather than taking props, because the page it sits in
/// is static markup that does not know about the flow. Rendered outside an
/// `Account` there is no flow, and so nothing to show.
#[component]
pub fn ClaimHero() -> impl IntoView {
    let Some(flow) = use_context::<ClaimFlow>() else {
        return ().into_any();
    };
    let handle = flow.handle();
    let busy = flow.busy();
    let ready = flow.privy_ready();
    let valid = move || is_valid_handle(&handle.get().trim().to_lowercase());

    view! {
        <form
            on:submit=move |event| {
                event.prevent_default();
                flow.start();
            }
            class="mt-9 flex max-w-xl flex-col gap-3 sm:flex-row sm:items-start"
        >
            <div class="flex-1">
                <label for="hero-handle" class="sr-only">
                    "Your Postage address"
                </label>
                <HandleField
                    id="hero-handle"
                    value=handle
                    on_change=Callback::new(move |handle| flow.set_handle(handle))
                />
            </div>
            // Disabled until Privy is ready too: a click before then would
            // reach no sign-in, and the handle would sit queued for whatever
            // sign-in happened next.
            <button
                type="submit"
                disabled=move || busy.get() || !valid() || !ready.get()
                class=format!("{PRIMARY_BUTTON} shrink-0 sm:mt-3")
            >
                {move || if busy.get() { "Claiming…" } else { "Claim it" }}
            </button>
        </form>
    }
    .into_any()
}

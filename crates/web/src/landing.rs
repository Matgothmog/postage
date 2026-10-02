//! The page a stranger opens. Replaces `web/src/app/Landing.tsx`.

use leptos::prelude::*;
use leptos_router::components::A;
use postage_core::handle::postage_address;

use crate::chrome::{AddressCard, PRIMARY_BUTTON};
use crate::privy_context::{start_login, use_privy};

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

/// The landing page's one call to action: sign in and get an address. The
/// handle form (`ClaimHero` in `Account.tsx`) replaces this with the claim
/// flow; until then the click opens the same Privy login.
#[component]
pub fn ClaimHero() -> impl IntoView {
    let privy = use_privy();
    view! {
        <div class="mt-9 flex max-w-xl">
            <button
                type="button"
                class=PRIMARY_BUTTON
                disabled=move || !privy.ready().get()
                on:click=move |_| start_login(privy)
            >
                "Claim your address"
            </button>
        </div>
    }
}

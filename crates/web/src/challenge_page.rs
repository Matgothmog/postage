//! `/c/:token`: the page a stranger lands on from a bounce email. Replaces
//! `web/src/app/c/[token]/page.tsx`.
//!
//! The Next page was a server component that read the database before it
//! rendered. This one asks `GET /api/challenge/{token}` and shows the
//! same three states (settled, dead link, open), plus the two the browser adds:
//! the wait for the answer, and the answer being a 404 or not arriving at all.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};
use postage_core::format::format_usdc;
use postage_core::handle::postage_address;

use crate::challenge_actions::{ChallengeActions, Lane};
use crate::challenge_api::{ChallengeView, OpenChallenge, ViewError, get_challenge};
use crate::chrome::{PRIMARY_BUTTON, QUIET_BUTTON, Shell};
use crate::screens::{ErrorScreen, NotFound};

/// Where the page is in learning what the challenge is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Load {
    Loading,
    Ready(ChallengeView),
    Failed(ViewError),
}

#[component]
pub fn ChallengePage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let token = Memo::new(move |_| params.read().get("token").unwrap_or_default());
    let load = RwSignal::new(Load::Loading);
    let attempt = RwSignal::new(0_u32);
    // Bumped by every read, so an answer for a token the page has since moved
    // on from (or for a read a retry has replaced) is dropped, not shown.
    let generation = StoredValue::new(0_u64);

    Effect::new(move |_| {
        attempt.track();
        let token = token.get();
        let mine = generation.try_update_value(|generation| {
            *generation += 1;
            *generation
        });
        load.set(Load::Loading);
        spawn_local(async move {
            let answer = get_challenge(&token).await;
            if generation.try_get_value() != mine {
                return;
            }
            load.try_set(match answer {
                Ok(view) => Load::Ready(view),
                Err(failure) => Load::Failed(failure),
            });
        });
    });

    let retry = Callback::new(move |()| attempt.update(|attempt| *attempt += 1));
    // The `?as=` of the link in the email, read once when the open challenge
    // appears: the sender's earlier answer, not something that changes under
    // them.
    let lane = move || Lane::from_query(query.read_untracked().get("as").as_deref());

    move || match load.get() {
        Load::Loading => view! {
            <Frame>
                <p class="text-sm text-faint" aria-busy="true">
                    "Loading"
                </p>
            </Frame>
        }
        .into_any(),
        Load::Failed(ViewError::NotFound) => view! { <NotFound /> }.into_any(),
        Load::Failed(ViewError::Unavailable) => view! { <ErrorScreen on_retry=retry /> }.into_any(),
        Load::Ready(ChallengeView::Resolved) => view! {
            <Frame>
                <h1 class="text-2xl font-semibold tracking-[-0.02em] text-fg">"Done."</h1>
                <p class="mt-3 text-[15px] leading-relaxed text-muted">
                    "That one's dealt with. A pass lasts 15 minutes."
                </p>
                <Advert />
            </Frame>
        }
        .into_any(),
        Load::Ready(ChallengeView::Dead) => view! {
            <Frame>
                <h1 class="text-2xl font-semibold tracking-[-0.02em] text-fg">"Dead link."</h1>
                <p class="mt-3 text-[15px] leading-relaxed text-muted">
                    "Write again for a fresh one."
                </p>
                <Advert />
            </Frame>
        }
        .into_any(),
        Load::Ready(ChallengeView::Open(challenge)) => {
            view! { <OpenView token=token.get_untracked() challenge=*challenge lane=lane() /> }
                .into_any()
        }
    }
}

/// The header (with its one link) and the narrow column every state sits in.
#[component]
fn Frame(children: Children) -> impl IntoView {
    view! {
        <Shell actions=WhatIsThis>
            <main class="mx-auto w-full max-w-xl px-6 py-14">{children()}</main>
        </Shell>
    }
}

#[component]
fn WhatIsThis() -> impl IntoView {
    view! {
        <A href="/" attr:class=QUIET_BUTTON>
            "What is this?"
        </A>
    }
}

/// An unanswered challenge: why it was stopped, what it costs, and the
/// actions.
#[component]
fn OpenView(token: String, challenge: OpenChallenge, lane: Lane) -> impl IntoView {
    let dangerous = challenge.dangerous;
    let held = challenge.held;
    let address = postage_address(&challenge.handle);
    let price = format_usdc(challenge.amount);
    let reasons = challenge.quote.reasons.clone();

    let lede = if dangerous {
        view! { "Reads as an attempt to deceive. Money won't fix that." }.into_any()
    } else if held {
        view! {
            "Your mail to " <span class="font-mono text-fg">{address}</span>
            " is safe. One click sends it."
        }
        .into_any()
    } else {
        view! { "That one bounced. Clear this and the next goes straight through." }.into_any()
    };

    view! {
        <Frame>
            <p class="text-[11px] font-semibold uppercase tracking-[0.2em] text-accent">
                {if held { "HELD" } else { "BOUNCED" }}
            </p>
            <h1 class="mt-4 text-3xl leading-[1.1] font-semibold tracking-[-0.03em] text-fg">
                {if dangerous { "Blocked. For good." } else { "Held at the door." }}
            </h1>
            <p class="mt-3 text-[15px] leading-relaxed text-muted">{lede}</p>

            <div class="mt-8 rounded-2xl border border-line bg-surface p-5">
                <p class="text-[11px] font-semibold uppercase tracking-[0.14em] text-faint">
                    "Why"
                </p>
                <ul class="mt-3 space-y-2">
                    {reasons
                        .into_iter()
                        .map(|reason| {
                            view! {
                                <li class="flex gap-2.5 text-sm text-muted">
                                    <span class="mt-2 h-1 w-1 shrink-0 rounded-full bg-line-strong" />
                                    {reason}
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            </div>

            {(!dangerous)
                .then(|| {
                    view! {
                        <p class="mt-5 text-sm leading-relaxed text-muted">
                            "Humans go free. Machines pay "
                            <span class="font-mono text-fg">{price}</span> " — to them, not us."
                        </p>
                    }
                })}

            <ChallengeActions token challenge lane />

            <Advert />
        </Frame>
    }
}

/// The pitch goes here and nowhere near a forwarded message. Whoever is
/// reading this is on the wrong side of exactly the problem Postage sells.
#[component]
fn Advert() -> impl IntoView {
    view! {
        <aside class="mt-14 rounded-2xl border border-accent/25 bg-accent-soft p-6">
            <p class="text-[11px] font-semibold uppercase tracking-[0.14em] text-accent">
                "POSTAGE"
            </p>
            <p class="mt-3 text-[17px] leading-snug font-medium text-fg">
                "Someone just got paid for this."
            </p>
            <p class="mt-2 text-sm leading-relaxed text-muted">
                "Charge strangers for your inbox. Keep the one you have."
            </p>
            <A href="/" attr:class=format!("mt-5 {PRIMARY_BUTTON}")>
                "Get paid too"
            </A>
        </aside>
    }
}

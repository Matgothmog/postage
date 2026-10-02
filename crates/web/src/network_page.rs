//! `/network`: the public ledger. Replaces `web/src/app/network/page.tsx`.
//!
//! The Next page was a server component that asked the subgraph while it
//! rendered. This one asks `GET /api/network` (which has already done the
//! sums and the "14m ago"s on the server's clock) and keeps asking: every
//! `REFRESH_EVERY` while the page is open.
//!
//! Reads never stack: a tick that finds one still out is skipped. A read that
//! a newer one has replaced (the visitor pressing "Try again" while a timed
//! read is out) is dropped when it lands. A timed read that fails after the
//! page has shown a ledger keeps that ledger on screen with a note, rather
//! than swapping a good page for an error one.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use postage_core::contracts::{ENCLAVE_REGISTRY, HUMAN_REGISTRY, POSTAGE_ESCROW, POSTAGE_VAULT};
use postage_core::format::{format_usdc, short_address};

use crate::api::ApiError;
use crate::auto_refresh::AutoRefresh;
use crate::chrome::{Callout, CalloutTone, SECONDARY_BUTTON, Shell};
use crate::network_api::{Enclave, FeedItem, Inbox, NetworkView, Sender, Vault, get_network};
use crate::network_parts::{Empty, Flow, Section, Stat, Tag, TagTone, Tier};
use crate::pages::GetAnAddress;

/// How often the page re-reads the ledger.
pub const REFRESH_EVERY: Duration = Duration::from_secs(20);

const EXPLORER: &str = "https://testnet.arcscan.app";

/// Where the page is. The ledger itself lives in its own signal so a refresh
/// changes the figures in place instead of rebuilding the page (and closing
/// the "Proof" panel the visitor opened).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    Loading,
    Ready,
    Failed(String),
}

/// What a finished read does to the page.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Settled {
    Show(Box<NetworkView>),
    /// A failed re-read: the ledger already shown stays.
    KeepWhatIsShown,
    Fail(String),
}

fn settle(showing_a_ledger: bool, answer: Result<NetworkView, ApiError>) -> Settled {
    match answer {
        Ok(view) => Settled::Show(Box::new(view)),
        Err(_) if showing_a_ledger => Settled::KeepWhatIsShown,
        Err(ApiError(reason)) => Settled::Fail(reason),
    }
}

/// Why a read starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// The countdown ran out: yields to a read already out.
    Timer,
    /// The page opened, or "Try again" was pressed: replaces any read out.
    Visitor,
}

/// How a sender stands, from what the server sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    VerifiedPerson,
    Reported { percent: u32, flagged: bool },
}

/// A verified person outranks any report rate. Otherwise at half or more
/// reported the tag turns red. A rate that is not a number reads as none: the
/// subgraph only ever writes decimals.
fn standing_of(still_human: bool, spam_rate: &str) -> Standing {
    if still_human {
        return Standing::VerifiedPerson;
    }
    let rate = spam_rate.parse::<f64>().unwrap_or(0.0);
    Standing::Reported {
        percent: (rate * 100.0).round() as u32,
        flagged: rate >= 0.5,
    }
}

/// `/network`.
#[component]
pub fn NetworkPage() -> impl IntoView {
    view! { <Ledger refresh_every=REFRESH_EVERY /> }
}

/// The ledger page; `refresh_every` is a prop so a test can wait for a tick.
#[component]
pub fn Ledger(refresh_every: Duration) -> impl IntoView {
    let phase = RwSignal::new(Phase::Loading);
    let ledger = RwSignal::new(NetworkView::default());
    let refresh_failed = RwSignal::new(false);
    // Bumped by every read, so an answer for a read since replaced is dropped.
    let generation = StoredValue::new(0_u64);
    let in_flight = StoredValue::new(false);

    let read = move |trigger: Trigger| {
        if trigger == Trigger::Timer && in_flight.get_value() {
            return;
        }
        let Some(mine) = generation.try_update_value(|generation| {
            *generation += 1;
            *generation
        }) else {
            return;
        };
        in_flight.set_value(true);
        if trigger == Trigger::Visitor {
            phase.set(Phase::Loading);
        }
        spawn_local(async move {
            let answer = get_network().await;
            if generation.try_get_value() != Some(mine) {
                return;
            }
            in_flight.try_set_value(false);
            let showing = phase.try_get_untracked() == Some(Phase::Ready);
            match settle(showing, answer) {
                Settled::Show(view) => {
                    ledger.try_set(*view);
                    refresh_failed.try_set(false);
                    phase.try_set(Phase::Ready);
                }
                Settled::KeepWhatIsShown => {
                    refresh_failed.try_set(true);
                }
                Settled::Fail(reason) => {
                    phase.try_set(Phase::Failed(reason));
                }
            }
        });
    };
    read(Trigger::Visitor);

    let on_tick = Callback::new(move |()| read(Trigger::Timer));
    let on_retry = Callback::new(move |()| read(Trigger::Visitor));
    // The page is rebuilt when the phase changes, not when the ledger does.
    let page = Memo::new(move |_| phase.get());

    move || match page.get() {
        Phase::Loading => view! { <Reading /> }.into_any(),
        Phase::Failed(reason) => {
            view! { <Refused reason on_retry refresh_every on_tick /> }.into_any()
        }
        Phase::Ready => view! {
            <Frame>
                <Overview
                    ledger=ledger.into()
                    refresh_failed=refresh_failed.into()
                    refresh_every
                    on_tick
                />
            </Frame>
        }
        .into_any(),
    }
}

#[component]
fn Reading() -> impl IntoView {
    view! {
        <Frame>
            <div class="mx-auto w-full max-w-5xl px-6 py-20">
                <h1 class="text-2xl font-semibold tracking-[-0.02em] text-fg">"Ledger"</h1>
                <p class="mt-4 text-sm text-faint" aria-busy="true">
                    "Reading the chain"
                </p>
            </div>
        </Frame>
    }
}

/// The read failed and there is no ledger to fall back on. The countdown
/// stays, so the page recovers by itself once the chain answers.
#[component]
fn Refused(
    reason: String,
    on_retry: Callback<()>,
    refresh_every: Duration,
    on_tick: Callback<()>,
) -> impl IntoView {
    view! {
        <Frame>
            <div class="mx-auto w-full max-w-5xl px-6 py-20">
                <h1 class="text-2xl font-semibold tracking-[-0.02em] text-fg">"Ledger"</h1>
                <div class="mt-4">
                    <Callout tone=CalloutTone::Bad title="Can't reach the chain.">
                        {reason}
                    </Callout>
                </div>
                <div class="mt-5 flex flex-wrap items-center gap-4">
                    <button
                        type="button"
                        class=SECONDARY_BUTTON
                        on:click=move |_| on_retry.run(())
                    >
                        "Try again"
                    </button>
                    <AutoRefresh every=refresh_every on_refresh=on_tick />
                </div>
            </div>
        </Frame>
    }
}

#[component]
fn Frame(children: Children) -> impl IntoView {
    view! { <Shell actions=GetAnAddress>{children()}</Shell> }
}

/// The ledger itself. Everything that depends on the figures is a closure
/// over `ledger`, so a refresh updates it in place.
#[component]
fn Overview(
    ledger: Signal<NetworkView>,
    refresh_failed: Signal<bool>,
    refresh_every: Duration,
    on_tick: Callback<()>,
) -> impl IntoView {
    view! {
        <main class="mx-auto w-full max-w-5xl px-6 py-14">
            <header class="flex flex-wrap items-end justify-between gap-4">
                <div>
                    <p class="text-[11px] font-semibold uppercase tracking-[0.2em] text-accent">
                        "Ledger"
                    </p>
                    <h1 class="mt-4 text-[2.5rem] leading-[1.02] font-semibold tracking-[-0.035em] text-fg">
                        "Every cent," <br /> "public."
                    </h1>
                    <p class="mt-4 max-w-xl text-[15px] leading-relaxed text-muted">
                        "No login. No trust required — check it yourself, on chain."
                    </p>
                </div>
                <div class="flex flex-col items-end gap-1.5">
                    <AutoRefresh every=refresh_every on_refresh=on_tick />
                    <Show when=move || refresh_failed.get()>
                        <span class="text-xs text-bad" role="status">
                            "Couldn't re-read just now. Showing the last answer."
                        </span>
                    </Show>
                </div>
            </header>

            <div class="mt-12 grid grid-cols-2 gap-px overflow-hidden rounded-2xl border border-line bg-line lg:grid-cols-4">
                {move || ledger.with(headline)}
            </div>

            <Section title="Live" hint="Every payment and verification, newest first.">
                {move || ledger.with(|ledger| feed_view(&ledger.feed))}
            </Section>

            <details class="group mt-12 rounded-2xl border border-line bg-surface">
                <summary class="flex cursor-pointer list-none items-center justify-between gap-4 px-5 py-4 text-[15px] font-medium text-fg [&::-webkit-details-marker]:hidden">
                    <span>"Proof"</span>
                    <span class="flex items-center gap-2 text-xs font-normal text-faint">
                        <span class="hidden sm:inline">"Vault, signer, contracts, standings"</span>
                        <span aria-hidden="true" class="transition-transform group-open:rotate-180">
                            "⌄"
                        </span>
                    </span>
                </summary>
                <div class="space-y-10 border-t border-line px-5 py-6">
                    {move || ledger.with(proof_view)}
                </div>
            </details>
        </main>
    }
}

/// The four figures across the top.
fn headline(ledger: &NetworkView) -> impl IntoView + use<> {
    let pool = ledger.vault.as_ref().map_or_else(
        || "$0.00".to_owned(),
        |vault| format_usdc(vault.to_sponsorship),
    );
    view! {
        <Stat label="Paid out" value=format_usdc(ledger.earned) note="earned by inboxes" />
        <Stat
            label="Verified free"
            value=ledger.verified_count.to_string()
            note="World ID, no wallet"
        />
        <Stat label="Held, then paid" value=ledger.delivered.to_string() note="messages" />
        <Stat
            label="Sponsor pool"
            value=pool
            note=format!("funds ~{} more", ledger.sponsored)
        />
    }
}

fn feed_view(feed: &[FeedItem]) -> AnyView {
    if feed.is_empty() {
        return view! { <Empty>"Nothing yet."</Empty> }.into_any();
    }
    view! {
        <ul class="divide-y divide-line">
            {feed.iter().map(feed_row).collect_view()}
        </ul>
    }
    .into_any()
}

fn feed_row(item: &FeedItem) -> AnyView {
    match item {
        FeedItem::Payment {
            since,
            tier,
            sender,
            inbox,
            tx,
            total,
        } => view! {
            <li class="flex items-center gap-4 px-5 py-3.5 text-sm">
                <Tier tier=tier.clone() />
                <a
                    href=format!("{EXPLORER}/address/{sender}")
                    class="font-mono text-muted hover:text-fg"
                >
                    {short_address(sender)}
                </a>
                <span class="hidden text-faint sm:inline">"paid"</span>
                <a
                    href=format!("{EXPLORER}/address/{inbox}")
                    class="hidden font-mono text-muted hover:text-fg sm:inline"
                >
                    {short_address(inbox)}
                </a>
                <span class="ml-auto shrink-0 text-xs text-faint tabular-nums">
                    {since.clone()}
                </span>
                <a
                    href=format!("{EXPLORER}/tx/{tx}")
                    class="w-16 shrink-0 text-right font-mono tabular-nums text-fg hover:text-accent"
                >
                    {format_usdc(*total)}
                </a>
            </li>
        }
        .into_any(),
        FeedItem::Verification { since, wallet } => view! {
            <li class="flex items-center gap-4 px-5 py-3.5 text-sm">
                <span class="flex shrink-0 items-center gap-2">
                    <span class="h-2 w-2 rounded-full bg-accent" />
                    <span class="w-20 text-xs text-faint">"verified"</span>
                </span>
                <a
                    href=format!("{EXPLORER}/address/{wallet}")
                    class="font-mono text-muted hover:text-fg"
                >
                    {short_address(wallet)}
                </a>
                <span class="hidden text-faint sm:inline">"proved human"</span>
                <span class="ml-auto shrink-0 text-xs text-faint tabular-nums">
                    {since.clone()}
                </span>
                <span class="w-16 shrink-0 text-right font-mono tabular-nums text-good">
                    {format_usdc(0)}
                </span>
            </li>
        }
        .into_any(),
    }
}

/// What is inside the "Proof" panel.
fn proof_view(ledger: &NetworkView) -> impl IntoView + use<> {
    view! {
        <div class="grid gap-6 lg:grid-cols-2">
            <div>
                <h3 class="text-sm font-medium text-fg">"Senders"</h3>
                <p class="mt-1 text-xs text-muted">"What one pays next time is decided here."</p>
                {senders_view(&ledger.senders)}
            </div>
            <div>
                <h3 class="text-sm font-medium text-fg">"Inboxes"</h3>
                <p class="mt-1 text-xs text-muted">"What each charges, and what it has taken."</p>
                {inboxes_view(&ledger.inboxes)}
            </div>
        </div>
        {vault_view(ledger.vault.as_ref())}
        {signers_view(&ledger.enclaves, ledger.has_active_signer)}
        <ContractList />
    }
}

fn senders_view(senders: &[Sender]) -> AnyView {
    if senders.is_empty() {
        return view! { <Empty>"No sender has a history yet."</Empty> }.into_any();
    }
    view! {
        <div class="mt-3 overflow-x-auto rounded-xl border border-line">
            <table class="w-full text-sm">
                <thead class="text-left text-[11px] uppercase tracking-[0.12em] text-faint">
                    <tr class="border-b border-line">
                        <th class="px-4 pb-2.5 pt-2 font-medium">"Wallet"</th>
                        <th class="pb-2.5 pt-2 font-medium">"Paid"</th>
                        <th class="px-4 pb-2.5 pt-2 text-right font-medium">"Standing"</th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-line">
                    {senders
                        .iter()
                        .map(|sender| {
                            view! {
                                <tr>
                                    <td class="px-4 py-2.5 font-mono text-muted">
                                        {short_address(&sender.id)}
                                    </td>
                                    <td class="py-2.5 tabular-nums text-fg">{sender.paid_count}</td>
                                    <td class="px-4 py-2.5 text-right">
                                        {standing_tag(standing_of(sender.still_human, &sender.spam_rate))}
                                    </td>
                                </tr>
                            }
                        })
                        .collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

fn standing_tag(standing: Standing) -> AnyView {
    match standing {
        Standing::VerifiedPerson => {
            view! { <Tag tone=TagTone::Good>"verified person"</Tag> }.into_any()
        }
        Standing::Reported { percent, flagged } => {
            let tone = if flagged {
                TagTone::Bad
            } else {
                TagTone::Quiet
            };
            view! { <Tag tone>{format!("{percent}% reported")}</Tag> }.into_any()
        }
    }
}

fn inboxes_view(inboxes: &[Inbox]) -> AnyView {
    if inboxes.is_empty() {
        return view! { <Empty>"No inbox has been paid yet."</Empty> }.into_any();
    }
    view! {
        <ul class="mt-3 divide-y divide-line rounded-xl border border-line">
            {inboxes
                .iter()
                .map(|inbox| {
                    view! {
                        <li class="flex items-baseline gap-3 px-4 py-2.5 text-sm">
                            <span class="font-mono text-muted">{short_address(&inbox.id)}</span>
                            <span class="text-xs text-faint">
                                {format!("{} floor", format_usdc(inbox.floor_price))}
                            </span>
                            <span class="ml-auto font-mono tabular-nums text-fg">
                                {format_usdc(inbox.earned)}
                            </span>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
    .into_any()
}

fn vault_view(vault: Option<&Vault>) -> impl IntoView + use<> {
    let money = |pick: fn(&Vault) -> u128| {
        vault.map_or_else(|| "$0.00".to_owned(), |vault| format_usdc(pick(vault)))
    };
    let funded = money(|vault| vault.total_funded);
    let sponsored = money(|vault| vault.to_sponsorship);
    let spent = money(|vault| vault.refilled_to_relayer);
    let events = vault.map_or(0, |vault| vault.funding_events);
    view! {
        <div>
            <h3 class="text-sm font-medium text-fg">"The vault"</h3>
            <p class="mt-1 max-w-2xl text-xs text-muted">
                "A share of every payment funds free verification for wallets holding nothing."
            </p>
            <div class="mt-3 grid gap-px overflow-hidden rounded-xl bg-line sm:grid-cols-3">
                <Flow label="Into the vault" value=funded>
                    {format!("from {events} payments")}
                </Flow>
                <Flow label="Set aside to sponsor" value=sponsored>
                    "70% of it, gas only"
                </Flow>
                <Flow label="Spent on gas" value=spent>
                    "verifications nobody paid for"
                </Flow>
            </div>
        </div>
    }
}

fn signers_view(enclaves: &[Enclave], has_active_signer: bool) -> impl IntoView + use<> {
    let list = if enclaves.is_empty() {
        view! { <Empty>"No signing key is registered."</Empty> }.into_any()
    } else {
        view! {
            <ul class="mt-3 divide-y divide-line rounded-xl border border-line">
                {enclaves
                    .iter()
                    .map(|enclave| {
                        let state = if enclave.revoked {
                            view! { <Tag tone=TagTone::Quiet>"revoked"</Tag> }.into_any()
                        } else {
                            view! { <Tag tone=TagTone::Good>"active"</Tag> }.into_any()
                        };
                        view! {
                            <li class="px-4 py-3">
                                <div class="flex items-center gap-3 text-sm">
                                    <a
                                        href=format!("{EXPLORER}/address/{}", enclave.id)
                                        class="font-mono text-fg hover:text-accent"
                                    >
                                        {short_address(&enclave.id)}
                                    </a>
                                    {state}
                                </div>
                                <p class="mt-1.5 truncate font-mono text-xs text-faint">
                                    {format!("measurement {}…", enclave.measurement_prefix)}
                                </p>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        }
        .into_any()
    };
    let caveat = if has_active_signer {
        "The live key's measurement matches an ordinary server, not an attested enclave. Said plainly, not hidden."
    } else {
        "Nothing can be priced until a key is registered."
    };
    view! {
        <div>
            <h3 class="text-sm font-medium text-fg">"Who can set a price"</h3>
            <p class="mt-1 max-w-2xl text-xs text-muted">
                "The escrow reverts on any signer not listed here."
            </p>
            {list} <p class="mt-2 text-xs leading-relaxed text-faint">{caveat}</p>
        </div>
    }
}

/// The four deployed contracts: constants, so they never wait on the answer.
#[component]
fn ContractList() -> impl IntoView {
    let contracts = [
        (
            "PostageEscrow",
            POSTAGE_ESCROW,
            "takes payment against a signed quote",
        ),
        (
            "HumanRegistry",
            HUMAN_REGISTRY,
            "records that someone proved they are a person",
        ),
        (
            "PostageVault",
            POSTAGE_VAULT,
            "turns paid mail into free verification",
        ),
        (
            "EnclaveRegistry",
            ENCLAVE_REGISTRY,
            "which keys may set a price",
        ),
    ];
    view! {
        <div>
            <h3 class="text-sm font-medium text-fg">"The contracts"</h3>
            <p class="mt-1 text-xs text-muted">"Read them yourself."</p>
            <ul class="mt-3 divide-y divide-line rounded-xl border border-line text-sm">
                {contracts
                    .into_iter()
                    .map(|(name, address, role)| {
                        let address = format!("{address:#x}");
                        let href = format!("{EXPLORER}/address/{address}");
                        view! {
                            <li class="flex flex-wrap items-baseline gap-x-3 gap-y-1 px-4 py-3">
                                <span class="w-36 shrink-0 text-fg">{name}</span>
                                <a
                                    href=href
                                    class="font-mono text-xs text-muted hover:text-accent"
                                >
                                    {address}
                                </a>
                                <span class="text-xs text-faint">{role}</span>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure() -> ApiError {
        ApiError("subgraph timed out".to_owned())
    }

    #[test]
    fn a_ledger_answer_is_shown() {
        assert_eq!(
            settle(false, Ok(NetworkView::default())),
            Settled::Show(Box::default())
        );
    }

    #[test]
    fn a_failed_first_read_fails_the_page_with_the_reason() {
        assert_eq!(
            settle(false, Err(failure())),
            Settled::Fail("subgraph timed out".to_owned())
        );
    }

    #[test]
    fn a_failed_re_read_keeps_the_ledger_already_shown() {
        assert_eq!(settle(true, Err(failure())), Settled::KeepWhatIsShown);
    }

    #[test]
    fn a_verified_person_outranks_a_high_report_rate() {
        assert_eq!(standing_of(true, "0.9"), Standing::VerifiedPerson);
    }

    #[test]
    fn half_reported_or_more_is_flagged_and_the_percentage_rounds_as_javascript_does() {
        assert_eq!(
            standing_of(false, "0.5"),
            Standing::Reported {
                percent: 50,
                flagged: true
            }
        );
        assert_eq!(
            standing_of(false, "0.285"),
            Standing::Reported {
                percent: 28,
                flagged: false
            }
        );
    }

    #[test]
    fn a_rate_that_is_not_a_number_reads_as_none() {
        assert_eq!(
            standing_of(false, ""),
            Standing::Reported {
                percent: 0,
                flagged: false
            }
        );
    }
}

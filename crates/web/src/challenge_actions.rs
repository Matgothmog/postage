//! What a sender can do about a challenge: prove they are a person, pay, or
//! (when the hold has run out) paste the message again. Replaces
//! `web/src/app/c/[token]/ChallengeActions.tsx`.
//!
//! Each of the three is a component with its own state. `ChallengeActions`
//! owns only the fork between them and the outcome that ends it; `PayLane` is
//! the only one that needs an account, because only that side moves money.

use std::future::Future;
use std::time::Duration;

use alloy_primitives::aliases::U40;
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_sol_types::SolCall;
use futures::channel::oneshot;
use leptos::leptos_dom::helpers::set_timeout;
use leptos::prelude::*;
use leptos::task::spawn_local;
use postage_core::contracts::{POSTAGE_ESCROW, PostageEscrow};
use postage_core::format::format_usdc;
use postage_core::handle::postage_address;
use postage_core::quote_types::QuoteFields;
use postage_core::tiers::tier_index_of;

use crate::bridge::BridgeError;
use crate::bridge::privy::TransactionRequest;
use crate::challenge_api::{self, IdentityMode, OpenChallenge, Resolution};
use crate::chrome::{Callout, CalloutTone, FIELD, PRIMARY_BUTTON, QUIET_BUTTON, SECONDARY_BUTTON};
use crate::privy_context::use_privy;
use crate::qr::WorldIdQr;
use crate::world_id::{BrowserWorld, WorldApp, verify_human};

/// Rechecks after the first ask. Long enough to cover a block on Arc and the
/// indexing behind it, short enough that the sender is not left watching a
/// spinner. Running out is not a verdict on the payment (see `settle`), only
/// the point at which the waiting stops being automatic and becomes theirs to
/// repeat.
const SETTLEMENT_ATTEMPTS: u32 = 10;
const SETTLEMENT_INTERVAL: Duration = Duration::from_millis(1_500);

const STILL_NOTHING: &str =
    "Still nothing on our side. It can take a few minutes — check again shortly.";

/// How long to keep asking whether a broadcast payment has settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settlement {
    /// Asks after the first one.
    pub attempts: u32,
    pub interval: Duration,
}

impl Default for Settlement {
    fn default() -> Self {
        Self {
            attempts: SETTLEMENT_ATTEMPTS,
            interval: SETTLEMENT_INTERVAL,
        }
    }
}

/// Which answer the sender already gave, if any. All three are links the mail
/// they were sent can carry, so arriving here having chosen should not mean
/// choosing again: `Human` and `Paying` each collapse the choice to the one
/// full-width button for that answer, `Choosing` shows both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lane {
    #[default]
    Choosing,
    Human,
    Paying,
}

impl Lane {
    /// The `?as=` query parameter of the link in the bounce email.
    pub fn from_query(value: Option<&str>) -> Self {
        match value {
            Some("bot") => Self::Paying,
            Some("human") => Self::Human,
            _ => Self::Choosing,
        }
    }
}

/// How a challenge ended for the sender.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Cleared { delivered: bool },
    Charged,
    Error(String),
}

/// The only outcomes the pay lane may hand back up, deliberately narrower than
/// `Outcome`: the pay lane's screen replaces the one that renders an error, so
/// an error passed up from it would be shown to nobody. Anything the sender
/// still has to act on stays inside `PayLane`, next to the controls that can
/// act on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    Cleared { delivered: bool },
    Charged,
}

impl From<Settled> for Outcome {
    fn from(settled: Settled) -> Self {
        match settled {
            Settled::Cleared { delivered } => Self::Cleared { delivered },
            Settled::Charged => Self::Charged,
        }
    }
}

/// A transaction that has been broadcast is not yet one that has been mined.
/// Asking once would tell most senders their payment failed a second after it
/// succeeded.
///
/// `None` when the window runs out with the payment still unaccounted for.
/// That is an answer we do not have yet, not a failure, and it must never be
/// reported as one: the money has already moved, and no window is long enough
/// to outlast an indexer having a bad afternoon.
pub async fn settle<Ask, AskFuture, Sleep, SleepFuture>(
    plan: Settlement,
    mut ask: Ask,
    mut sleep: Sleep,
) -> Option<Settled>
where
    Ask: FnMut() -> AskFuture,
    AskFuture: Future<Output = Resolution>,
    Sleep: FnMut(Duration) -> SleepFuture,
    SleepFuture: Future<Output = ()>,
{
    let mut resolution = ask().await;
    for _ in 0..plan.attempts {
        if resolution != Resolution::NotYet {
            break;
        }
        sleep(plan.interval).await;
        resolution = ask().await;
    }
    match resolution {
        Resolution::Cleared { delivered } => Some(Settled::Cleared { delivered }),
        Resolution::Charged => Some(Settled::Charged),
        Resolution::NotYet => None,
    }
}

async fn sleep(duration: Duration) {
    let (wake, woken) = oneshot::channel();
    set_timeout(
        move || {
            // The sleeper may have been dropped with its page; nobody to wake.
            let _ = wake.send(());
        },
        duration,
    );
    let _ = woken.await;
}

/// The call data for paying a quote: the escrow's `payToSend`, with exactly
/// what the server signed.
pub fn pay_to_send_data(quote: &QuoteFields, amount: u128) -> Result<Bytes, String> {
    let message_id: B256 = quote
        .message_id
        .parse()
        .map_err(|_| "The quote's message id is not valid".to_owned())?;
    let inbox: Address = quote
        .inbox
        .parse()
        .map_err(|_| "The quote's inbox address is not valid".to_owned())?;
    let signature: Bytes = quote
        .signature
        .parse()
        .map_err(|_| "The quote's signature is not valid".to_owned())?;
    let expires_at = U40::try_from(quote.expires_at)
        .map_err(|_| "The quote's expiry is out of range".to_owned())?;
    let call = PostageEscrow::payToSendCall {
        messageId: message_id,
        inbox,
        tier: tier_index_of(&quote.tier),
        amount: U256::from(amount),
        expiresAt: expires_at,
        enclaveSignature: signature,
    };
    Ok(Bytes::from(call.abi_encode()))
}

/// What the sender is shown for a bridge failure: the SDK's own words.
fn bridge_message(error: BridgeError) -> String {
    match error {
        BridgeError::Sdk { message, .. } => message,
        other => other.to_string(),
    }
}

/// Which view is up. A memo over `Outcome` and `Lane` so a change to either
/// that does not change the view (a new error message) leaves it standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Charged,
    Sent,
    Deliver,
    Pay,
    Choose,
}

fn screen_for(outcome: Option<&Outcome>, lane: Lane) -> Screen {
    match outcome {
        Some(Outcome::Charged) => Screen::Charged,
        // The held message went out on its own, so there is nothing left to
        // ask of the sender. Only a hold that ran out sends them back to the
        // compose box.
        Some(Outcome::Cleared { delivered: true }) => Screen::Sent,
        Some(Outcome::Cleared { delivered: false }) => Screen::Deliver,
        Some(Outcome::Error(_)) | None if lane == Lane::Paying => Screen::Pay,
        Some(Outcome::Error(_)) | None => Screen::Choose,
    }
}

#[component]
pub fn ChallengeActions(
    #[prop(into)] token: String,
    challenge: OpenChallenge,
    #[prop(optional)] lane: Lane,
    #[prop(optional)] settlement: Settlement,
    #[prop(optional)] world: WorldApp,
) -> impl IntoView {
    let dangerous = challenge.dangerous;
    // A dangerous challenge offers no paying: the only way through is a person.
    let lane = RwSignal::new(if dangerous { Lane::Choosing } else { lane });
    let outcome = RwSignal::new(None::<Outcome>);
    let screen = Memo::new(move |_| screen_for(outcome.read().as_ref(), lane.get()));

    let token = StoredValue::new(token);
    let challenge = StoredValue::new(challenge);
    let handle = challenge.with_value(|challenge| challenge.handle.clone());

    let on_settled = Callback::new(move |settled: Settled| outcome.set(Some(settled.into())));
    let on_back = Callback::new(move |()| lane.set(Lane::Choosing));

    move || match screen.get() {
        Screen::Charged => view! {
            <div class="mt-8">
                <Callout tone=CalloutTone::Bad title="Charged. Still blocked.">
                    "Paying is the penalty here, not a price."
                </Callout>
            </div>
        }
        .into_any(),
        Screen::Sent => view! {
            <div class="mt-8">
                <Callout tone=CalloutTone::Good title="Sent.">
                    "Same words, same sender. Already in their inbox."
                </Callout>
            </div>
        }
        .into_any(),
        Screen::Deliver => {
            view! { <Deliver token=token.get_value() handle=handle.clone() /> }.into_any()
        }
        Screen::Pay => view! {
            <PayLane
                token=token.get_value()
                quote=challenge.with_value(|challenge| challenge.quote.quote.clone())
                amount=challenge.with_value(|challenge| challenge.amount)
                settlement
                on_settled
                on_back
            />
        }
        .into_any(),
        Screen::Choose => view! {
            <ChooseLane
                token=token.get_value()
                mode=challenge.with_value(|challenge| challenge.identity_mode)
                amount=challenge.with_value(|challenge| challenge.amount)
                dangerous
                lane
                outcome
                world
            />
        }
        .into_any(),
    }
}

/// The fork: "I'm human" (free, no wallet) or "I'm a bot" (pay).
#[component]
fn ChooseLane(
    token: String,
    mode: IdentityMode,
    amount: u128,
    dangerous: bool,
    lane: RwSignal<Lane>,
    outcome: RwSignal<Option<Outcome>>,
    world: WorldApp,
) -> impl IntoView {
    let verifying = RwSignal::new(false);
    // Set only under live mode, once IDKit has a request ready to be answered.
    // Lets the sender open World App while the poll is still waiting on them;
    // otherwise they are looking at a spinner with no way to finish it.
    // Rendered below as a code to scan and a link to tap, and nothing here
    // navigates on the sender's behalf: the poll that finishes this
    // verification is running on this page, so an automatic redirect would
    // unload the very thing waiting for the answer. A sender without World App
    // would land on a fallthrough page with the check already dead behind
    // them, and no way back to the code they never saw.
    let connector_uri = RwSignal::new(None::<String>);

    let token = StoredValue::new(token);
    // Proving personhood is not a payment, so it should not need an account
    // to make one: no wallet anywhere on this path.
    let verify = move |_| {
        verifying.set(true);
        outcome.set(None);
        connector_uri.set(None);
        let token = token.get_value();
        spawn_local(async move {
            let result = verify_human(&BrowserWorld, mode, &world, &token, |uri| {
                connector_uri.try_set(Some(uri.to_owned()));
            })
            .await;
            outcome.try_set(Some(match result {
                Ok(delivered) => Outcome::Cleared { delivered },
                Err(message) => Outcome::Error(message),
            }));
            // Whatever happened, the button is live again and the link is gone.
            verifying.try_set(false);
            connector_uri.try_set(None);
        });
    };

    let pay_amount = format_usdc(amount);
    let human_lane = lane.get_untracked() == Lane::Human;
    let failed = move || matches!(outcome.read().as_ref(), Some(Outcome::Error(_)));

    view! {
        <div class="mt-8 space-y-3">
            <button
                type="button"
                on:click=verify
                disabled=move || verifying.get()
                class=format!("{PRIMARY_BUTTON} w-full")
            >
                {move || if verifying.get() { "Checking…" } else { "I'm human — free" }}
            </button>
            <p class="px-1 text-xs leading-relaxed text-faint">
                "World ID. No wallet, no account."
            </p>

            {move || {
                connector_uri
                    .get()
                    .map(|uri| view! { <ConnectorPanel uri /> })
            }}

            {(!dangerous && !human_lane)
                .then(|| {
                    let label = format!("I'm a bot — pay {pay_amount}");
                    view! {
                        <button
                            type="button"
                            on:click=move |_| lane.set(Lane::Paying)
                            disabled=move || verifying.get()
                            class=format!("{SECONDARY_BUTTON} w-full")
                        >
                            {label}
                        </button>
                        <p class="px-1 text-xs leading-relaxed text-faint">
                            "Goes to them, not us."
                        </p>
                    }
                })}

            {move || {
                match outcome.read().as_ref() {
                    Some(Outcome::Error(message)) => {
                        Some(view! { <p class="text-sm text-bad">{message.clone()}</p> })
                    }
                    _ => None,
                }
            }}

            // The way back out of the human lane, symmetric with the "I'm
            // human" button the pay lane offers. Arriving from the mail's
            // human link collapses the choice to one button, but it must not
            // lock the other door: World ID fails for senders who have no
            // World App, decline it, or hit an outage, and paying is the only
            // thing left between them and a hold that erases the message.
            // Quiet while the free path is still worth a try, a real button
            // once a verification has actually failed.
            {(!dangerous && human_lane)
                .then(|| {
                    let label = format!("Pay {} instead", format_usdc(amount));
                    view! {
                        <button
                            type="button"
                            on:click=move |_| lane.set(Lane::Paying)
                            disabled=move || verifying.get()
                            class=move || {
                                let tone = if failed() { SECONDARY_BUTTON } else { QUIET_BUTTON };
                                format!("{tone} w-full")
                            }
                        >
                            {label}
                        </button>
                    }
                })}
        </div>
    }
}

/// The World App connector as a link to tap and a code to scan.
#[component]
fn ConnectorPanel(uri: String) -> impl IntoView {
    view! {
        <div class="space-y-3 rounded-2xl border border-line bg-surface p-5">
            <a
                href=uri.clone()
                target="_blank"
                rel="noreferrer"
                class=format!("{PRIMARY_BUTTON} w-full")
            >
                "Open World App"
            </a>
            <p class="text-center text-xs leading-relaxed text-faint">
                "Leave this page open — it finishes on its own when you do."
            </p>
            <div class="mx-auto w-fit rounded-xl bg-white p-3">
                <WorldIdQr uri />
            </div>
            <p class="text-center text-xs leading-relaxed text-faint">
                "No World App on this device? Scan this with the phone that has it."
            </p>
        </div>
    }
}

/// Only this side of the fork needs an account, because only this side moves
/// money. A person who is simply a person never reaches it.
#[component]
fn PayLane(
    token: String,
    quote: QuoteFields,
    amount: u128,
    settlement: Settlement,
    on_settled: Callback<Settled>,
    on_back: Callback<()>,
) -> impl IntoView {
    let privy = use_privy();
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    // The hash of a payment that has been broadcast but not yet been seen to
    // settle. Its presence is what replaces the Pay button with a confirmation
    // view: an indexer can always lag past whatever window we poll for, and a
    // sender who is shown "Pay" again after their money has already moved
    // will reasonably click it and pay twice. The hash is kept because it is
    // the one thing they can point at while they wait.
    let broadcast_hash = RwSignal::new(None::<B256>);
    // Set by the pay button's own click and consumed by the effect below, so a
    // login (which ends with a wallet Privy creates on the way) can still end
    // in a payment without a second click. Not a signal: the intent to pay is
    // not something any view reflects, only something the next change of
    // wallet needs to check.
    let wants_to_pay = StoredValue::new(false);

    let token = StoredValue::new(token);
    let quote = StoredValue::new(quote);

    let ask_and_settle = move || async move {
        let token = token.get_value();
        settle(
            settlement,
            || {
                let token = token.clone();
                async move { challenge_api::post_resolve(&token).await }
            },
            sleep,
        )
        .await
    };

    let pay = move || {
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let sent: Result<B256, String> = async {
                let quote = quote.get_value();
                let tx = TransactionRequest {
                    to: POSTAGE_ESCROW,
                    data: pay_to_send_data(&quote, amount)?,
                    value: Some(U256::from(amount)),
                };
                privy.send_transaction(&tx).await.map_err(bridge_message)
            }
            .await;
            match sent {
                Err(message) => {
                    error.try_set(Some(message));
                }
                Ok(hash) => {
                    // The money has moved. Everything past this line is about
                    // telling the sender what happened to it, and this sender
                    // is never offered a Pay button again.
                    broadcast_hash.try_set(Some(hash));
                    if let Some(settled) = ask_and_settle().await {
                        on_settled.run(settled);
                    }
                }
            }
            busy.try_set(false);
        });
    };

    // The way on from a payment that broadcast but has not been seen to
    // settle: one more pass of the same polling, on the sender's own timing.
    // It moves no money; the only control offered once a payment exists must
    // not be able to make a second one.
    let recheck = move |_| {
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match ask_and_settle().await {
                Some(settled) => on_settled.run(settled),
                None => {
                    error.try_set(Some(STILL_NOTHING.to_owned()));
                }
            }
            busy.try_set(false);
        });
    };

    // Continues a pay click into the payment itself once login has produced a
    // wallet, so "Pay" is one click rather than "log in" then "pay". It never
    // fires on its own: the intent is only ever set by the button's click.
    // Every signal is read before the intent is checked, or a first run with no
    // intent would subscribe to nothing and never run again.
    Effect::new(move |_| {
        let busy_now = busy.get();
        let paid = broadcast_hash.read().is_some();
        let authenticated = privy.authenticated().get();
        let has_wallet = privy.wallet().get().is_some();
        if !wants_to_pay.get_value() || busy_now || paid || !authenticated || !has_wallet {
            return;
        }
        wants_to_pay.set_value(false);
        pay();
    });

    let handle_pay = move |_| {
        // Already able to pay right now: fire it directly and leave the
        // intent unset, or the effect above would see `busy` return to false
        // once this finishes and fire a second, redundant payment.
        if privy.authenticated().get_untracked() && privy.wallet().get_untracked().is_some() {
            pay();
            return;
        }
        wants_to_pay.set_value(true);
        if privy.authenticated().get_untracked() {
            return;
        }
        spawn_local(async move {
            let Err(failure) = privy.login().await else {
                return;
            };
            // Closing the modal withdraws the click: a later sign-in by some
            // other route must not turn into a payment nobody asked for.
            wants_to_pay.try_set_value(false);
            if !failure.is_user_cancel() {
                leptos::logging::error!("sign-in failed: {failure}");
            }
        });
    };

    let pay_label = move || {
        if busy.get() {
            "Paying…".to_owned()
        } else if privy.authenticated().get() && privy.wallet().get().is_none() {
            "Setting up your wallet…".to_owned()
        } else {
            format!("Pay {}", format_usdc(amount))
        }
    };

    // The broadcast view comes ahead of the `ready` gate on purpose: once a
    // payment exists, its hash is the most important thing on this page and
    // nothing about Privy's own readiness should be able to replace it with a
    // spinner.
    move || {
        if let Some(hash) = broadcast_hash.get() {
            return view! {
                <div class="mt-8 space-y-3">
                    <Callout tone=CalloutTone::Good title="Paid. Confirming.">
                        "It's on the chain. Confirming can take a few minutes, and paying again would charge you twice."
                    </Callout>
                    <p class="px-1 font-mono text-xs break-all text-faint" data-testid="tx-hash">
                        {hash.to_string()}
                    </p>
                    <button
                        type="button"
                        on:click=recheck
                        disabled=move || busy.get()
                        class=format!("{PRIMARY_BUTTON} w-full")
                    >
                        {move || if busy.get() { "Checking…" } else { "Check again" }}
                    </button>
                    {move || error.get().map(|message| view! { <p class="text-sm text-warn">{message}</p> })}
                </div>
            }
            .into_any();
        }
        if !privy.ready().get() {
            return view! { <p class="mt-8 text-sm text-faint">"Loading"</p> }.into_any();
        }
        view! {
            <div class="mt-8 space-y-3">
                <button
                    type="button"
                    on:click=handle_pay
                    disabled=move || {
                        busy.get() || (privy.authenticated().get() && privy.wallet().get().is_none())
                    }
                    class=format!("{PRIMARY_BUTTON} w-full")
                >
                    {pay_label}
                </button>

                // Shut while a transaction is in flight: leaving unmounts this
                // lane, and with it the record of a payment that may already
                // have been broadcast a moment later.
                <button
                    type="button"
                    on:click=move |_| on_back.run(())
                    disabled=move || busy.get()
                    class=format!("{QUIET_BUTTON} w-full")
                >
                    "I'm human"
                </button>
                {move || error.get().map(|message| view! { <p class="text-sm text-bad">{message}</p> })}
            </div>
        }
        .into_any()
    }
}

/// The way back when the hold has already run out, or when the send failed.
/// The message it puts through is one the sender writes here, so it goes out
/// under our name rather than pretending to be theirs.
#[component]
fn Deliver(token: String, handle: String) -> impl IntoView {
    let subject = RwSignal::new(String::new());
    let body = RwSignal::new(String::new());
    let sending = RwSignal::new(false);
    let sent = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let token = StoredValue::new(token);

    let deliver = move |_| {
        sending.set(true);
        error.set(None);
        let (subject, body) = (subject.get_untracked(), body.get_untracked());
        spawn_local(async move {
            match challenge_api::post_deliver(&token.get_value(), &subject, &body).await {
                Ok(()) => {
                    sent.try_set(true);
                }
                Err(failure) => {
                    error.try_set(Some(failure.to_string()));
                }
            }
            sending.try_set(false);
        });
    };

    let placeholder = format!("Paste what you wrote to {}", postage_address(&handle));

    move || {
        if sent.get() {
            return view! {
                <div class="mt-8">
                    <Callout tone=CalloutTone::Good title="Sent.">
                        "Replies come straight to you."
                    </Callout>
                </div>
            }
            .into_any();
        }
        view! {
            <div class="mt-8 space-y-4">
                <Callout tone=CalloutTone::Good title="Cleared.">
                    "The hold expired. Paste it again."
                </Callout>

                <input
                    type="text"
                    prop:value=move || subject.get()
                    on:input=move |event| subject.set(event_target_value(&event))
                    placeholder="Subject"
                    class=FIELD
                />
                <textarea
                    prop:value=move || body.get()
                    on:input=move |event| body.set(event_target_value(&event))
                    rows="7"
                    placeholder=placeholder.clone()
                    class=format!("{FIELD} resize-y")
                />
                <button
                    type="button"
                    on:click=deliver
                    disabled=move || sending.get() || body.read().trim().is_empty()
                    class=format!("{PRIMARY_BUTTON} w-full")
                >
                    {move || if sending.get() { "Sending…" } else { "Send" }}
                </button>
                {move || error.get().map(|message| view! { <p class="text-sm text-bad">{message}</p> })}
            </div>
        }
        .into_any()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use alloy_primitives::hex;
    use futures::executor::block_on;

    use super::*;

    fn quote() -> QuoteFields {
        QuoteFields {
            message_id: format!("0x{}", "ab".repeat(32)),
            inbox: format!("0x{}", "cd".repeat(20)),
            tier: "commercial".to_owned(),
            amount: "1000000".to_owned(),
            expires_at: 1_800_000_000,
            signature: format!("0x{}", "ef".repeat(65)),
        }
    }

    fn plan() -> Settlement {
        Settlement {
            attempts: 3,
            interval: Duration::from_millis(1_500),
        }
    }

    /// Runs `settle` over a scripted sequence of answers, recording the sleeps.
    fn settle_over(answers: &[Resolution]) -> (Option<Settled>, usize, Vec<Duration>) {
        let asked = Cell::new(0_usize);
        let slept = RefCell::new(Vec::new());
        let settled = block_on(settle(
            plan(),
            || {
                let index = asked.get().min(answers.len() - 1);
                asked.set(asked.get() + 1);
                let answer = answers[index];
                async move { answer }
            },
            |duration| {
                slept.borrow_mut().push(duration);
                async {}
            },
        ));
        (settled, asked.get(), slept.into_inner())
    }

    #[test]
    fn the_pay_call_is_the_escrows_pay_to_send_with_exactly_what_the_server_signed() {
        let data = pay_to_send_data(&quote(), 1_000_000).unwrap();

        assert_eq!(data[..4], hex!("b6ba947b"));
        let decoded = PostageEscrow::payToSendCall::abi_decode(&data).unwrap();
        assert_eq!(decoded.messageId, B256::repeat_byte(0xab));
        assert_eq!(decoded.inbox, Address::repeat_byte(0xcd));
        assert_eq!(decoded.tier, 2, "commercial is escrow tier 2");
        assert_eq!(decoded.amount, U256::from(1_000_000_u64));
        assert_eq!(decoded.expiresAt, U40::from(1_800_000_000_u64));
        assert_eq!(decoded.enclaveSignature, Bytes::from(vec![0xef; 65]));
    }

    #[test]
    fn each_tier_maps_to_its_escrow_index() {
        for (tier, index) in [
            ("human", 0),
            ("important", 1),
            ("commercial", 2),
            ("dangerous", 3),
        ] {
            let quote = QuoteFields {
                tier: tier.to_owned(),
                ..quote()
            };
            let data = pay_to_send_data(&quote, 1).unwrap();
            assert_eq!(
                PostageEscrow::payToSendCall::abi_decode(&data)
                    .unwrap()
                    .tier,
                index
            );
        }
    }

    #[test]
    fn a_quote_that_cannot_be_encoded_is_an_error_message_not_a_panic() {
        let broken = |edit: fn(&mut QuoteFields)| {
            let mut quote = quote();
            edit(&mut quote);
            pay_to_send_data(&quote, 1)
        };
        assert!(broken(|quote| quote.message_id = "0x12".to_owned()).is_err());
        assert!(broken(|quote| quote.inbox = "nope".to_owned()).is_err());
        assert!(broken(|quote| quote.signature = "0xzz".to_owned()).is_err());
        assert!(broken(|quote| quote.expires_at = 1 << 40).is_err());
    }

    #[test]
    fn a_payment_that_clears_on_the_first_ask_never_sleeps() {
        let (settled, asked, slept) = settle_over(&[Resolution::Cleared { delivered: true }]);

        assert_eq!(settled, Some(Settled::Cleared { delivered: true }));
        assert_eq!((asked, slept.len()), (1, 0));
    }

    #[test]
    fn a_payment_the_chain_shows_late_is_found_on_a_later_ask() {
        let (settled, asked, slept) = settle_over(&[
            Resolution::NotYet,
            Resolution::NotYet,
            Resolution::Cleared { delivered: false },
        ]);

        assert_eq!(settled, Some(Settled::Cleared { delivered: false }));
        assert_eq!(asked, 3);
        assert_eq!(slept, [Duration::from_millis(1_500); 2]);
    }

    #[test]
    fn a_charge_is_an_answer_too() {
        let (settled, ..) = settle_over(&[Resolution::NotYet, Resolution::Charged]);

        assert_eq!(settled, Some(Settled::Charged));
    }

    #[test]
    fn the_window_running_out_reads_as_unconfirmed_never_as_a_failed_payment() {
        let (settled, asked, slept) = settle_over(&[Resolution::NotYet]);

        assert_eq!(settled, None);
        assert_eq!(asked, 4, "the first ask plus every retry");
        assert_eq!(slept.len(), 3);
    }

    #[test]
    fn the_default_window_is_ten_retries_a_second_and_a_half_apart() {
        assert_eq!(
            Settlement::default(),
            Settlement {
                attempts: 10,
                interval: Duration::from_millis(1_500)
            }
        );
    }

    #[test]
    fn the_query_picks_the_lane_and_anything_else_is_the_choice() {
        assert_eq!(Lane::from_query(Some("bot")), Lane::Paying);
        assert_eq!(Lane::from_query(Some("human")), Lane::Human);
        for other in [None, Some(""), Some("Bot"), Some("both")] {
            assert_eq!(Lane::from_query(other), Lane::Choosing, "{other:?}");
        }
    }

    #[test]
    fn the_screen_follows_the_outcome_then_the_lane() {
        let cleared = |delivered| Some(Outcome::Cleared { delivered });
        assert_eq!(
            screen_for(Some(&Outcome::Charged), Lane::Choosing),
            Screen::Charged
        );
        assert_eq!(
            screen_for(cleared(true).as_ref(), Lane::Paying),
            Screen::Sent
        );
        assert_eq!(
            screen_for(cleared(false).as_ref(), Lane::Choosing),
            Screen::Deliver
        );
        assert_eq!(screen_for(None, Lane::Paying), Screen::Pay);
        assert_eq!(screen_for(None, Lane::Human), Screen::Choose);
        let failed = Outcome::Error("no".to_owned());
        assert_eq!(screen_for(Some(&failed), Lane::Human), Screen::Choose);
        assert_eq!(screen_for(Some(&failed), Lane::Paying), Screen::Pay);
    }

    #[test]
    fn a_bridge_failure_shows_the_sdks_own_words() {
        let sdk = BridgeError::Sdk {
            code: "4001".to_owned(),
            message: "User rejected request".to_owned(),
        };
        assert_eq!(bridge_message(sdk), "User rejected request");
        assert!(bridge_message(BridgeError::NotLoaded).contains("bridge"));
    }
}

//! The claim screens: the form that picks an address, the strip that waits for
//! the two confirmations, and the handle field both the hero and the form
//! use. Replaces `web/src/app/ClaimStrip.tsx`.

use std::time::Duration;

use alloy_primitives::Address;
use leptos::leptos_dom::helpers::set_interval_with_handle;
use leptos::prelude::*;
use leptos::task::spawn_local;
use postage_core::handle::{MAIL_DOMAIN, is_valid_handle, keep_handle_chars, postage_address};
use postage_core::wallet_proof::confirm_statement;
use web_sys::{AbortController, HtmlInputElement};

use crate::api::{self, ClaimState};
use crate::chrome::{
    AddressCard, Callout, CalloutTone, FIELD, PRIMARY_BUTTON, QUIET_BUTTON, SECONDARY_BUTTON,
};
use crate::pending_claim::{ClaimProgress, PollVerdict, poll_verdict};
use crate::privy_context::use_privy;
use crate::proof::{BrowserProofIo, wallet_proof};

/// How often the strip asks whether Cloudflare's link has been clicked.
pub const POLL_INTERVAL: Duration = Duration::from_secs(4);

/// The code a claim is confirmed with is this many digits.
const CODE_LENGTH: usize = 6;

/// The submit button's gate. The same predicate `/api/inbox` enforces
/// server-side, composed from the same shape, length and repeated-dot rules in
/// `postage_core::handle`, so this cannot disable the button for a shorter
/// list of reasons than the server would reject on, or enable it for one the
/// server accepts. Both arguments are already trimmed and lowercased.
pub fn claim_setup_ready(name: &str, destination: &str) -> bool {
    is_valid_handle(name) && destination.contains('@')
}

/// `/api/inbox` skips the emailed code only when the destination is the one
/// Privy already verified for this session. Cloudflare destinations are
/// account-wide, so an address somebody else confirmed reads as confirmed to
/// us: every other address has to answer a code of ours first.
pub fn needs_code(destination: &str, email: Option<&str>) -> bool {
    destination != email.unwrap_or_default().trim().to_lowercase()
}

/// Whether the digits now in the field are a code to send. The field sends
/// itself on the sixth digit (there is nothing else on it to decide, and a
/// code just read off an email is not improved by a second gesture confirming
/// it was read correctly), but it slices to six, so a seventh keystroke leaves
/// those same six digits in place. Sending them again spends one of the
/// attempts this claim gets before its code is dead for good.
pub fn ready_to_submit(digits: &str, busy: bool, failed: bool, last_sent: Option<&str>) -> bool {
    if digits.chars().count() != CODE_LENGTH {
        return false;
    }
    if busy || failed {
        return false;
    }
    last_sent != Some(digits)
}

/// The first six digits of whatever was typed or pasted.
fn code_digits(raw: &str) -> String {
    raw.chars()
        .filter(char::is_ascii_digit)
        .take(CODE_LENGTH)
        .collect()
}

/// The handle input and its domain, drawn as one field so the address reads
/// as a whole. Shared by the hero and the setup panel, which ask for exactly
/// the same thing in two different places.
#[component]
pub fn HandleField(
    id: &'static str,
    #[prop(into)] value: Signal<String>,
    on_change: Callback<String>,
) -> impl IntoView {
    view! {
        <div class="mt-3 flex items-stretch overflow-hidden rounded-xl border border-line-strong bg-surface-2 transition focus-within:border-accent">
            <input
                id=id
                prop:value=move || value.get()
                on:input=move |event| {
                    let input = event_target::<HtmlInputElement>(&event);
                    let kept = keep_handle_chars(&input.value());
                    // The signal alone would leave a rejected character on
                    // screen when the kept text equals what it already held.
                    input.set_value(&kept);
                    on_change.run(kept);
                }
                placeholder="you"
                autocomplete="off"
                spellcheck="false"
                maxlength="31"
                class="w-full min-w-0 bg-transparent px-4 py-3 font-mono text-[15px] text-fg outline-none placeholder:text-faint"
            />
            <span class="grid shrink-0 place-items-center pr-4 font-mono text-[15px] text-faint">
                {format!("@{MAIL_DOMAIN}")}
            </span>
        </div>
    }
}

/// The last thing asked of somebody signing in without an address on file,
/// and the only place the destination is chosen. One panel rather than a
/// wizard: the handle came in from the hero, and the address it forwards to
/// is filled in already for every session Privy could read one from.
#[component]
pub fn ClaimSetup(
    #[prop(into)] handle: Signal<String>,
    on_handle: Callback<String>,
    #[prop(into)] destination: Signal<String>,
    on_destination: Callback<String>,
    #[prop(into)] email: Signal<Option<String>>,
    #[prop(into)] busy: Signal<bool>,
    #[prop(into)] error: Signal<Option<String>>,
    on_submit: Callback<()>,
) -> impl IntoView {
    let name = move || handle.get().trim().to_lowercase();
    let to = move || destination.get().trim().to_lowercase();
    let ready = move || claim_setup_ready(&name(), &to());
    let code_needed = move || needs_code(&to(), email.get().as_deref());

    view! {
        <main class="rise mx-auto w-full max-w-xl px-6 py-16">
            <h1 class="text-3xl font-semibold tracking-[-0.03em] text-fg">"Pick your address."</h1>
            <p class="mt-3 text-[15px] leading-relaxed text-muted">
                "Mail sent to it is read, judged, and forwarded to the inbox you already use."
            </p>

            <form
                on:submit=move |event| {
                    event.prevent_default();
                    on_submit.run(());
                }
                class="mt-9 rounded-2xl border border-line bg-surface p-6"
            >
                <label for="handle" class="text-sm font-medium text-fg">
                    "Your Postage address"
                </label>
                <HandleField id="handle" value=handle on_change=on_handle />

                <label for="destination" class="mt-6 block text-sm font-medium text-fg">
                    "Forwards to"
                </label>
                <input
                    id="destination"
                    prop:value=move || destination.get()
                    on:input=move |event| on_destination.run(event_target_value(&event))
                    placeholder="you@gmail.com"
                    inputmode="email"
                    autocomplete="email"
                    class=format!("{FIELD} mt-3")
                />
                <p class="mt-2 text-xs text-faint">
                    {move || {
                        if code_needed() {
                            "We will email a code there first, to check you can read it."
                        } else {
                            "The address you signed in with, so there is no code to type."
                        }
                    }}
                </p>

                <button
                    type="submit"
                    disabled=move || busy.get() || !ready()
                    class=format!("{PRIMARY_BUTTON} mt-6 w-full")
                >
                    {move || if busy.get() { "Claiming…" } else { "Claim it" }}
                </button>
                {move || {
                    error
                        .get()
                        .map(|message| view! { <p class="mt-3 text-sm text-bad">{message}</p> })
                }}
            </form>

            <p class="mt-4 px-1 text-xs leading-relaxed text-faint">
                "Cloudflare carries the mail and confirms the destination with its owner itself, so its one link is the last step."
            </p>
        </main>
    }
}

/// What is left of the claim, over the dashboard it is turning into. The
/// waiting is real (only the person reading that mailbox can click
/// Cloudflare's link), so the wait happens in front of the thing being waited
/// for rather than on a screen of its own.
///
/// `claim` is the claim as the owner of this component last knew it;
/// `on_claim` hands back a step that moved, `on_live` says the inbox is real,
/// `on_restart` says the claim is gone or abandoned.
#[component]
pub fn ClaimStrip(
    #[prop(into)] claim: Signal<ClaimProgress>,
    wallet: Address,
    on_claim: Callback<ClaimProgress>,
    on_live: Callback<()>,
    on_restart: Callback<()>,
    #[prop(default = POLL_INTERVAL)] poll_interval: Duration,
) -> impl IntoView {
    let privy = use_privy();
    let code = RwSignal::new(String::new());
    // The server has stopped asking Cloudflare about this claim, so polling it
    // can only ever return the same answer.
    let stalled = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    // An attempt that came back wrong. Until one does, the sixth digit is the
    // whole of what we are asking for and a button to confirm it asks twice.
    let failed = RwSignal::new(false);
    let in_flight = StoredValue::new_local(None::<AbortController>);
    // The code the last attempt went out with. Not rendered, and it must not
    // cause a render: what it is for is deciding, inside the very keystroke
    // that would send it, whether these six digits have already been sent.
    let last_sent = StoredValue::new(None::<String>);

    // Only writes back a step that actually moved. An unconditional write is a
    // new value every four seconds, which would re-run the poll effect below
    // and restart the interval it just set; the poll then never reaches its
    // own deadline.
    let apply = Callback::new(move |next: ClaimState| {
        if next.live {
            on_live.run(());
            return;
        }
        if next.stalled {
            stalled.set(true);
        }
        let current = claim.get_untracked();
        let code_verified = current.code_verified || next.code_verified;
        let cloudflare_verified = current.cloudflare_verified || next.cloudflare_verified;
        if code_verified == current.code_verified
            && cloudflare_verified == current.cloudflare_verified
        {
            return;
        }
        on_claim.run(ClaimProgress {
            code_verified,
            cloudflare_verified,
            ..current
        });
    });

    // Cloudflare's half turns green when the user clicks the link in its
    // email, which happens outside this page, so it has to be asked for.
    let handle = Memo::new(move |_| claim.with(|claim| claim.handle.clone()));
    let cloudflare_verified = Memo::new(move |_| claim.with(|claim| claim.cloudflare_verified));
    Effect::new(move |_| {
        if cloudflare_verified.get() || stalled.get() {
            return;
        }
        let handle = handle.get();
        let tick = move || {
            spawn_local(poll_once(
                handle.clone(),
                claim,
                in_flight,
                apply,
                on_restart,
            ));
        };
        match set_interval_with_handle(tick, poll_interval) {
            Ok(interval) => on_cleanup(move || {
                interval.clear();
                abort_in_flight(in_flight);
            }),
            Err(cause) => leptos::logging::error!("could not start the claim poll: {cause:?}"),
        }
    });

    let submit = move |entered: String| {
        last_sent.set_value(Some(entered.clone()));
        busy.set(true);
        error.set(None);
        let current = claim.get_untracked();
        spawn_local(async move {
            let result = confirm_code(privy, wallet, &current.handle, &entered).await;
            busy.try_set(false);
            match result {
                Ok(state) => {
                    failed.try_set(false);
                    apply.run(state);
                }
                Err(message) => {
                    failed.try_set(true);
                    error.try_set(Some(message));
                }
            }
        });
    };

    let on_code_input = move |event: leptos::ev::Event| {
        let input = event_target::<HtmlInputElement>(&event);
        let digits = code_digits(&input.value());
        input.set_value(&digits);
        code.set(digits.clone());
        let send = ready_to_submit(
            &digits,
            busy.get_untracked(),
            failed.get_untracked(),
            last_sent.get_value().as_deref(),
        );
        if send {
            submit(digits);
        }
    };

    view! {
        <main class="rise mx-auto w-full max-w-3xl px-6 py-14">
            <div class="glow rounded-2xl border border-line-strong bg-surface p-6 sm:p-7">
                <p class="text-[11px] font-semibold uppercase tracking-[0.2em] text-accent">
                    "One click left"
                </p>
                <h1 class="mt-3 font-mono text-xl break-all text-fg sm:text-2xl">
                    {move || postage_address(&handle.get())}
                </h1>

                {move || {
                    (!claim.with(|claim| claim.code_verified))
                        .then(|| {
                            view! {
                                <div class="mt-6">
                                    <label for="code" class="text-sm text-fg">
                                        "Code from your email"
                                    </label>
                                    <div class="mt-3 flex flex-wrap gap-2">
                                        <input
                                            id="code"
                                            prop:value=move || code.get()
                                            on:input=on_code_input
                                            inputmode="numeric"
                                            autocomplete="one-time-code"
                                            placeholder="000000"
                                            class="w-36 rounded-xl border border-line-strong bg-surface-2 px-4 py-3 text-center font-mono text-[15px] tracking-[0.3em] text-fg outline-none transition focus:border-accent"
                                        />
                                        {move || {
                                            failed
                                                .get()
                                                .then(|| {
                                                    view! {
                                                        <button
                                                            type="button"
                                                            on:click=move |_| submit(code.get_untracked())
                                                            disabled=move || {
                                                                busy.get() || code.get().chars().count() != CODE_LENGTH
                                                            }
                                                            class=SECONDARY_BUTTON
                                                        >
                                                            {move || if busy.get() { "Checking" } else { "Confirm" }}
                                                        </button>
                                                    }
                                                })
                                        }}
                                    </div>
                                </div>
                            }
                        })
                }}

                <p class="mt-6 text-[15px] leading-relaxed text-muted">
                    {move || {
                        let claim = claim.get();
                        if claim.code_verified {
                            view! {
                                "Cloudflare emailed "
                                <span class="font-mono text-fg">{claim.destination}</span>
                                ". Click its link and you’re live."
                            }
                                .into_any()
                        } else {
                            view! {
                                "We emailed a code to "
                                <span class="font-mono text-fg">{claim.destination}</span>
                                ". Cloudflare’s own link follows."
                            }
                                .into_any()
                        }
                    }}
                </p>

                {move || {
                    let (code_verified, cloudflare_verified) = claim
                        .with(|claim| (claim.code_verified, claim.cloudflare_verified));
                    (code_verified && !cloudflare_verified)
                        .then(|| {
                            if stalled.get() {
                                view! {
                                    <p class="mt-2 text-sm text-muted">
                                        "We stopped watching. Start over below — it’s quick."
                                    </p>
                                }
                                    .into_any()
                            } else {
                                view! {
                                    <p class="mt-2 text-sm text-faint">
                                        "Waiting on Cloudflare. Check spam."
                                    </p>
                                }
                                    .into_any()
                            }
                        })
                }}

                {move || {
                    error
                        .get()
                        .map(|message| {
                            view! {
                                <div class="mt-5">
                                    <Callout tone=CalloutTone::Bad title=message />
                                </div>
                            }
                        })
                }}

                <StartOver on_restart=on_restart />
            </div>

            <PendingDashboard
                handle=Signal::derive(move || handle.get())
                destination=Signal::derive(move || claim.with(|claim| claim.destination.clone()))
            />
        </main>
    }
}

/// One tick of the poll: asks the server, and acts on the verdict. Free of
/// the component so the decision it makes reads on its own.
async fn poll_once(
    handle: String,
    claim: Signal<ClaimProgress>,
    in_flight: StoredValue<Option<AbortController>, LocalStorage>,
    apply: Callback<ClaimState>,
    on_restart: Callback<()>,
) {
    abort_in_flight(in_flight);
    let controller = AbortController::new().ok();
    in_flight.set_value(controller.clone());
    let signal = controller.as_ref().map(AbortController::signal);

    let answer = api::get_claim_state(&handle, signal.as_ref()).await;

    // The claim this tick asked about may have been restarted or replaced
    // while the answer was on its way.
    if claim.try_with_untracked(|claim| claim.handle != handle) != Some(false) {
        return;
    }
    // A poll that fails is a poll that runs again in four seconds.
    let Ok(response) = answer else {
        return;
    };
    match poll_verdict(response.status) {
        // The server is certain there is nothing here to wait for: the code
        // timed out, or the claim was promoted and cleared. The strip must not
        // outlive it.
        PollVerdict::Gone => on_restart.run(()),
        // Anything else that is not an answer says nothing about the claim,
        // and forgetting one on a wobble spends one of the five a wallet gets
        // in an hour.
        PollVerdict::Wait => {}
        PollVerdict::Read => {
            if let Ok(state) = serde_json::from_str::<ClaimState>(&response.body) {
                apply.run(state);
            }
        }
    }
}

fn abort_in_flight(in_flight: StoredValue<Option<AbortController>, LocalStorage>) {
    if let Some(Some(controller)) = in_flight.try_get_value() {
        controller.abort();
    }
}

/// Sends the code with the wallet proof. The code says whoever holds this
/// mailbox agreed; it does not say who is claiming. The wallet the claim was
/// started with says that, and both are needed: it is the wallet the inbox's
/// earnings accrue to.
async fn confirm_code(
    privy: crate::bridge::privy::Privy,
    wallet: Address,
    handle: &str,
    entered: &str,
) -> Result<ClaimState, String> {
    let io = BrowserProofIo::new(privy);
    let token = privy.identity_token().get_untracked();
    let proof = wallet_proof(&io, token.as_deref(), wallet, |at| {
        confirm_statement(handle, &wallet.to_string(), at)
    })
    .await
    .map_err(|error| error.to_string())?;
    api::post_confirmation(handle, entered.trim(), proof)
        .await
        .map_err(|error| error.to_string())
}

/// The way out, from wherever the claim has got to.
///
/// Every route back to the form used to be gated: the code field on an
/// unverified code, one restart on a stalled Cloudflare, another on a 410 or
/// 429. A claim sent to a mistyped address has none of those (its code lands
/// somewhere its owner cannot read, so the code never verifies) and its owner
/// had no way back at all short of clearing site data. Losing a pending claim
/// costs one of five tries in an hour; being held in one forever costs the
/// account. So this is rendered in every state, as one control.
#[component]
fn StartOver(on_restart: Callback<()>) -> impl IntoView {
    view! {
        <div class="mt-7 border-t border-line pt-4">
            <button
                type="button"
                on:click=move |_| on_restart.run(())
                class=format!("{QUIET_BUTTON} -ml-3")
            >
                "Start over"
            </button>
            <p class="mt-1 px-1 text-xs text-faint">
                "Wrong address, or no code arrived? This forgets the claim and takes you back to the form."
            </p>
        </div>
    }
}

/// The dashboard this claim is turning into, drawn from what the claim already
/// knows. Not the inbox panel: there is no inbox row yet and nothing on chain
/// to read for one, so there is nothing here to press either. Hidden from
/// screen readers because the strip above it has just said all of it.
#[component]
fn PendingDashboard(
    #[prop(into)] handle: Signal<String>,
    #[prop(into)] destination: Signal<String>,
) -> impl IntoView {
    view! {
        <div class="mt-10 opacity-50" aria-hidden="true">
            <div class="flex flex-col gap-10 sm:flex-row sm:items-start sm:justify-between">
                <div>
                    <p class="text-[11px] font-semibold uppercase tracking-[0.2em] text-faint">
                        "Pending"
                    </p>
                    <p class="mt-4 font-mono text-2xl break-all text-fg sm:text-[1.7rem]">
                        {move || postage_address(&handle.get())}
                    </p>
                    <p class="mt-2 text-[15px] text-muted">
                        "→ " <span class="font-mono text-fg">{move || destination.get()}</span>
                    </p>
                </div>
                <div class="hidden shrink-0 sm:block">
                    {move || {
                        view! {
                            <AddressCard
                                handle=postage_address(&handle.get())
                                price="—"
                                caption="Yours the moment Cloudflare answers"
                            />
                        }
                    }}
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The `ready` rule (ClaimStrip.test.ts, five tests).

    #[test]
    fn ready_is_false_for_a_handle_shorter_than_the_servers_minimum() {
        assert!(!claim_setup_ready("a", "user@example.com"));
    }

    #[test]
    fn ready_is_false_for_a_handle_starting_with_punctuation_which_the_servers_shape_rule_rejects()
    {
        assert!(!claim_setup_ready("-demo", "user@example.com"));
    }

    #[test]
    fn ready_is_false_for_a_handle_with_two_dots_running_together_which_the_server_rejects_separately()
     {
        assert!(!claim_setup_ready("demo..user", "user@example.com"));
    }

    #[test]
    fn ready_is_true_for_a_handle_and_destination_the_server_would_accept() {
        assert!(claim_setup_ready("demo.user-1", "user@example.com"));
    }

    #[test]
    fn ready_still_requires_an_at_sign_in_the_destination_even_once_the_handle_is_valid() {
        assert!(!claim_setup_ready("demo", "not-an-email"));
    }

    // The code field's send rule (four tests).

    #[test]
    fn the_code_field_sends_itself_on_the_sixth_digit_and_not_before() {
        assert!(!ready_to_submit("12345", false, false, None));
        assert!(ready_to_submit("123456", false, false, None));
    }

    #[test]
    fn a_seventh_keystroke_does_not_re_send_the_six_digits_already_sent() {
        assert!(!ready_to_submit("123456", false, false, Some("123456")));
        assert!(ready_to_submit("123457", false, false, Some("123456")));
    }

    #[test]
    fn the_code_field_sends_nothing_while_an_attempt_is_already_in_flight() {
        assert!(!ready_to_submit("123456", true, false, None));
    }

    #[test]
    fn the_code_field_stops_sending_itself_once_an_attempt_has_come_back_wrong() {
        assert!(!ready_to_submit("123456", false, true, None));
    }

    #[test]
    fn typed_text_keeps_only_the_first_six_digits() {
        assert_eq!(code_digits("12a34-56 78"), "123456");
        assert_eq!(code_digits("abc"), "");
    }

    #[test]
    fn the_destination_the_session_already_verified_needs_no_code() {
        assert!(!needs_code("me@example.com", Some(" Me@Example.com ")));
        assert!(needs_code("other@example.com", Some("me@example.com")));
        assert!(needs_code("me@example.com", None));
    }
}

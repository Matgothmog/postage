//! The owner's dashboard: what the address has earned, what it charges, and
//! the two buttons that act on the chain. Replaces
//! `web/src/app/InboxPanel.tsx`.
//!
//! The numbers are read from the escrow contract in the browser (`chain`), and
//! the two writes (`setFloorPrice`, `claimEarnings`) go through Privy's
//! transaction screen, encoded with the contract's own bindings.

use alloy_primitives::{Address, Bytes, U256};
use alloy_sol_types::SolCall;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use postage_core::contracts::{POSTAGE_ESCROW, PostageEscrow, USDC_DECIMALS};
use postage_core::format::{format_usdc, parse_usdc, short_address};
use postage_core::handle::postage_address;

use crate::api::Inbox;
use crate::bridge::privy::TransactionRequest;
use crate::chain::{Standing, read_standing};
use crate::chrome::{
    AddressCard, Callout, CalloutTone, FIELD, PRIMARY_BUTTON, QUIET_BUTTON, SECONDARY_BUTTON,
};
use crate::privy_context::use_privy;

/// The one-click price options. `amount` is the decimal string `parse_usdc`
/// expects, never a pre-formatted label, so a chip's active state compares
/// amounts instead of strings.
const PRICE_PRESETS: [(&str, &str); 4] =
    [("1¢", "0.01"), ("5¢", "0.05"), ("25¢", "0.25"), ("$1", "1")];

const PRICE_CHIP_BASE: &str = "rounded-full border px-4 py-2 text-sm font-medium transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-hi focus-visible:ring-offset-2 focus-visible:ring-offset-bg disabled:opacity-40";
const PRICE_CHIP_ACTIVE: &str = "border-accent bg-accent text-on-accent";
const PRICE_CHIP_INACTIVE: &str = "border-line-strong bg-surface-2 text-fg hover:border-accent-hi";

/// What the panel is doing on the chain right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Busy {
    Price,
    Claim,
}

/// `formatUnits(amount, 18)`: whole units, the fraction trimmed of trailing
/// zeros, no decimal point when there is none. What the custom field is
/// pre-filled with.
pub fn format_units(amount: u128) -> String {
    let unit = 10_u128.pow(USDC_DECIMALS);
    let (whole, fraction) = (amount / unit, amount % unit);
    if fraction == 0 {
        return whole.to_string();
    }
    let fraction = format!("{fraction:0width$}", width = USDC_DECIMALS as usize);
    format!("{whole}.{}", fraction.trim_end_matches('0'))
}

/// The preset (as its decimal string) whose amount is the owner's chosen
/// floor, if any. A defaulted floor is not a chosen price, so no chip,
/// preset or Custom, should render as active for it.
pub fn active_preset(standing: Option<&Standing>) -> Option<&'static str> {
    let chosen = standing.filter(|standing| standing.chosen)?.floor;
    PRICE_PRESETS
        .iter()
        .map(|(_, amount)| *amount)
        .find(|amount| parse_usdc(amount) == Ok(chosen))
}

/// The call data for the panel's two actions.
fn encode_price_call(amount: &str) -> Result<Bytes, String> {
    let amount = parse_usdc(amount).map_err(|error| error.to_string())?;
    let call = PostageEscrow::setFloorPriceCall {
        amount: U256::from(amount),
    };
    Ok(Bytes::from(call.abi_encode()))
}

fn encode_claim_call(owner: Address) -> Bytes {
    Bytes::from(PostageEscrow::claimEarningsCall { to: owner }.abi_encode())
}

#[component]
pub fn InboxPanel(inbox: Inbox, wallet: Address) -> impl IntoView {
    let privy = use_privy();
    let standing = RwSignal::new(None::<Standing>);
    let draft = RwSignal::new(String::new());
    let custom_open = RwSignal::new(false);
    let busy = RwSignal::new(None::<Busy>);
    let error = RwSignal::new(None::<String>);

    let apply = move |next: Standing| {
        standing.try_set(Some(next));
        draft.try_set(format_units(next.floor));
    };

    let load = move || {
        error.set(None);
        spawn_local(async move {
            match read_standing(wallet).await {
                Ok(next) => apply(next),
                Err(cause) => {
                    error.try_set(Some(cause.to_string()));
                }
            }
        });
    };
    // Reads once on mount; the signals die with the panel, so a late answer
    // for an unmounted panel lands nowhere.
    load();

    // `amount` overrides the draft for the one-click preset chips; the custom
    // field's "Set" button falls back to the draft. Either way this is the
    // same setFloorPrice call, same units, same encoding.
    let send = move |action: Busy, amount: Option<String>| {
        busy.set(Some(action));
        error.set(None);
        let draft_text = draft.get_untracked();
        spawn_local(async move {
            let outcome: Result<Standing, String> = async {
                let data = match action {
                    Busy::Price => encode_price_call(&amount.unwrap_or(draft_text))?,
                    Busy::Claim => encode_claim_call(wallet),
                };
                let tx = TransactionRequest {
                    to: POSTAGE_ESCROW,
                    data,
                    value: None,
                };
                privy
                    .send_transaction(&tx)
                    .await
                    .map_err(|cause| match cause {
                        crate::bridge::BridgeError::Sdk { message, .. } => message,
                        other => other.to_string(),
                    })?;
                read_standing(wallet)
                    .await
                    .map_err(|cause| cause.to_string())
            }
            .await;
            match outcome {
                Ok(next) => apply(next),
                Err(message) => {
                    error.try_set(Some(message));
                }
            }
            busy.try_set(None);
        });
    };

    let chosen_preset = move || active_preset(standing.get().as_ref());
    let chosen_custom =
        move || standing.get().is_some_and(|standing| standing.chosen) && chosen_preset().is_none();
    let custom_shown = move || custom_open.get() || chosen_custom();

    let price = move || {
        standing
            .get()
            .map_or("—".to_owned(), |s| format_usdc(s.floor))
    };
    let earned = move || {
        standing
            .get()
            .map_or("—".to_owned(), |standing| format_usdc(standing.earned))
    };

    let address = postage_address(&inbox.handle);
    let card_address = address.clone();

    view! {
        <main class="rise mx-auto w-full max-w-3xl px-6 py-14">
            <div class="flex flex-col gap-10 sm:flex-row sm:items-start sm:justify-between">
                <div>
                    <p class="text-[11px] font-semibold uppercase tracking-[0.2em] text-accent">
                        "Live"
                    </p>
                    <h1 class="mt-4 font-mono text-2xl break-all text-fg sm:text-[1.7rem]">
                        {address}
                    </h1>
                    <p class="mt-2 text-[15px] text-muted">
                        "→ " <span class="font-mono text-fg">{inbox.destination}</span>
                    </p>
                    <p class="mt-1 text-xs text-faint">
                        {format!("Paid into {}", short_address(&wallet.to_string()))}
                    </p>
                </div>
                <div class="hidden shrink-0 sm:block">
                    {move || {
                        view! {
                            <AddressCard
                                handle=card_address.clone()
                                price=price()
                                caption="Hand this out instead of your own"
                            />
                        }
                    }}
                </div>
            </div>

            <div class="mt-12 grid gap-4 sm:grid-cols-2">
                <section
                    class="rounded-2xl border border-line-strong bg-surface p-6"
                    aria-busy=move || (standing.get().is_none() && error.get().is_none()).to_string()
                >
                    <p class="text-xs uppercase tracking-[0.14em] text-faint">"Earned"</p>
                    <p class="mt-2 font-mono text-3xl tabular-nums text-fg" data-testid="earned">
                        {earned}
                    </p>
                    <button
                        type="button"
                        on:click=move |_| send(Busy::Claim, None)
                        disabled=move || {
                            busy.get().is_some()
                                || standing.get().is_none_or(|standing| standing.earned == 0)
                        }
                        class=format!("{PRIMARY_BUTTON} mt-5 w-full")
                    >
                        {move || if busy.get() == Some(Busy::Claim) { "Cashing out…" } else { "Cash out" }}
                    </button>
                    <p class="mt-3 text-xs leading-relaxed text-faint">
                        "Only machines pay. Humans are free."
                    </p>
                </section>

                <section class="rounded-2xl border border-line-strong bg-surface p-6">
                    <p class="text-xs uppercase tracking-[0.14em] text-faint">"Your price"</p>
                    <div class="mt-2 flex flex-wrap gap-2">
                        {PRICE_PRESETS
                            .into_iter()
                            .map(|(label, amount)| {
                                view! {
                                    <button
                                        type="button"
                                        on:click=move |_| {
                                            custom_open.set(false);
                                            send(Busy::Price, Some(amount.to_owned()));
                                        }
                                        disabled=move || busy.get().is_some()
                                        class=move || {
                                            let tone = if chosen_preset() == Some(amount) {
                                                PRICE_CHIP_ACTIVE
                                            } else {
                                                PRICE_CHIP_INACTIVE
                                            };
                                            format!("{PRICE_CHIP_BASE} {tone}")
                                        }
                                    >
                                        {label}
                                    </button>
                                }
                            })
                            .collect_view()}
                        <button
                            type="button"
                            on:click=move |_| custom_open.update(|open| *open = !*open)
                            disabled=move || busy.get().is_some()
                            class=move || {
                                let tone = if custom_shown() {
                                    PRICE_CHIP_ACTIVE
                                } else {
                                    PRICE_CHIP_INACTIVE
                                };
                                format!("{PRICE_CHIP_BASE} {tone}")
                            }
                        >
                            "Custom"
                        </button>
                    </div>
                    {move || {
                        custom_shown()
                            .then(|| {
                                view! {
                                    <div class="mt-3 flex gap-2">
                                        <input
                                            prop:value=move || draft.get()
                                            on:input=move |event| draft.set(event_target_value(&event))
                                            inputmode="decimal"
                                            placeholder="0.01"
                                            aria-label="Custom price"
                                            class=format!("{FIELD} font-mono")
                                        />
                                        <button
                                            type="button"
                                            on:click=move |_| send(Busy::Price, None)
                                            disabled=move || {
                                                busy.get().is_some() || draft.get().trim().is_empty()
                                            }
                                            class=SECONDARY_BUTTON
                                        >
                                            {move || if busy.get() == Some(Busy::Price) { "Saving" } else { "Set" }}
                                        </button>
                                    </div>
                                }
                            })
                    }}
                    <p class="mt-3 text-xs leading-relaxed text-faint">
                        {move || match standing.get() {
                            Some(standing) if !standing.chosen => {
                                format!("Charging {} by default.", format_usdc(standing.floor))
                            }
                            _ => "Bots pay this. Phishing pays 10×, and still bounces.".to_owned(),
                        }}
                    </p>
                </section>
            </div>

            {move || {
                error
                    .get()
                    .map(|message| {
                        let unread = standing.get_untracked().is_none();
                        view! {
                            <div class="mt-6">
                                <Callout tone=CalloutTone::Bad title=message>
                                    {unread
                                        .then(|| {
                                            view! {
                                                <button
                                                    type="button"
                                                    on:click=move |_| load()
                                                    class=format!("{QUIET_BUTTON} -ml-3 mt-1 text-bad")
                                                >
                                                    "Try again"
                                                </button>
                                            }
                                        })}
                                </Callout>
                            </div>
                        }
                    })
            }}

            <p class="mt-10 text-sm text-muted">
                "Every cent is public. "
                <A
                    href="/network"
                    attr:class="text-fg underline decoration-line-strong underline-offset-4 hover:decoration-fg"
                >
                    "See the ledger →"
                </A>
            </p>
        </main>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENT: u128 = 10_u128.pow(16);

    fn standing(floor: u128, chosen: bool) -> Standing {
        Standing {
            earned: 0,
            floor,
            chosen,
        }
    }

    #[test]
    fn format_units_trims_trailing_zeros_and_drops_a_bare_point() {
        assert_eq!(format_units(CENT), "0.01");
        assert_eq!(format_units(100 * CENT), "1");
        assert_eq!(format_units(0), "0");
        assert_eq!(format_units(1), "0.000000000000000001");
        assert_eq!(format_units(250 * CENT + 5 * CENT / 10), "2.505");
    }

    #[test]
    fn a_chosen_floor_that_equals_a_preset_lights_that_chip() {
        assert_eq!(active_preset(Some(&standing(CENT, true))), Some("0.01"));
        assert_eq!(
            active_preset(Some(&standing(25 * CENT, true))),
            Some("0.25")
        );
        assert_eq!(active_preset(Some(&standing(100 * CENT, true))), Some("1"));
    }

    #[test]
    fn a_defaulted_floor_is_not_a_chosen_price_so_no_chip_is_active() {
        assert_eq!(active_preset(Some(&standing(CENT, false))), None);
        assert_eq!(active_preset(None), None);
    }

    #[test]
    fn a_chosen_floor_off_the_presets_lights_none_of_them() {
        assert_eq!(active_preset(Some(&standing(7 * CENT, true))), None);
    }

    #[test]
    fn set_floor_price_encodes_the_parsed_amount_under_its_selector() {
        let data = encode_price_call("0.05").unwrap();
        // setFloorPrice(uint256)
        assert_eq!(data[..4], PostageEscrow::setFloorPriceCall::SELECTOR);
        assert_eq!(U256::from_be_slice(&data[4..]), U256::from(5 * CENT));
        assert_eq!(data.len(), 4 + 32);
    }

    #[test]
    fn a_price_that_is_not_a_number_is_a_message_not_a_transaction() {
        assert!(encode_price_call("free").is_err());
        assert!(encode_price_call("").is_err());
    }

    #[test]
    fn claim_earnings_encodes_the_owner_as_the_recipient() {
        let owner = Address::repeat_byte(0xab);
        let data = encode_claim_call(owner);
        assert_eq!(data[..4], PostageEscrow::claimEarningsCall::SELECTOR);
        assert_eq!(&data[16..], owner.as_slice());
    }
}

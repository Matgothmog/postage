use postage_shared::Tier;
use serde::Deserialize;

const ONE: f64 = 10_000.0;
const CEILING: f64 = 100_000.0;
const YEAR_SECONDS: i64 = 365 * 24 * 60 * 60;

/// What the subgraphs know about a sender, reduced to what `quote` reads.
/// Anything nothing reads is a query nobody should pay for.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SenderSignals {
    pub paid_count: u64,
    pub spam_reports: u64,
    pub spam_rate: f64,
    pub ens_names: u64,
    /// Epoch seconds. `Some(0)` is a real timestamp, not the absence of one.
    pub oldest_ens_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    pub amount: u128,
    pub floor: u128,
    pub multiplier_bps: u32,
    pub free: bool,
    pub reasons: Vec<String>,
}

/// What each verdict costs before reputation is considered, in basis points of
/// the inbox's floor price.
const fn tier_bps(tier: Tier) -> f64 {
    match tier {
        // Reads as written by a person, but nobody proved it. We assume a
        // machine and charge the floor.
        Tier::Human | Tier::Commercial => ONE,
        // Never charged, because it is never held. `quote` returns before it
        // reads this; the table is only total.
        Tier::Important => 0.0,
        // Deliberately punitive. Dangerous mail is blocked either way; this is
        // what a sender pays if they have a wallet attached.
        Tier::Dangerous => 10.0 * ONE,
    }
}

/// JavaScript's `Math.round`: halves go toward positive infinity, so the
/// multiplier math prices identically to the TypeScript it replaces.
fn js_round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// Turns a verdict and what The Graph knows about a sender into a price.
///
/// The tier sets the base and reputation moves it. A quote can never fall
/// below the inbox's floor, because the escrow enforces that and would revert.
/// `now` is epoch seconds, passed in because core has no clock.
pub fn quote(
    floor: u128,
    tier: Tier,
    signals: Option<&SenderSignals>,
    degraded: bool,
    now: i64,
) -> Quote {
    if tier == Tier::Important {
        return Quote {
            amount: 0,
            floor,
            multiplier_bps: 0,
            free: true,
            reasons: vec!["Something they are waiting for".to_owned()],
        };
    }

    let mut reasons: Vec<String> = Vec::new();
    let mut bps = tier_bps(tier);

    reasons.push(
        match tier {
            Tier::Dangerous => "Reads as an attempt to deceive",
            Tier::Human => "Reads human, nobody proved it",
            _ => "Automated mail nobody asked for",
        }
        .to_owned(),
    );

    // A header-only verdict is not confident enough to charge punitively.
    if degraded && tier == Tier::Dangerous {
        bps = 2.0 * ONE;
        reasons.push("Priced down: headers only".to_owned());
    }

    if let Some(signals) = signals {
        apply_signals(&mut bps, &mut reasons, signals, now);
    }

    if bps < ONE {
        reasons.push("Already at this inbox's floor".to_owned());
    }
    let bps = bps.clamp(ONE, CEILING);
    let multiplier_bps = bps as u32;

    Quote {
        amount: scale_by_bps(floor, multiplier_bps),
        floor,
        multiplier_bps,
        free: false,
        reasons,
    }
}

fn apply_signals(bps: &mut f64, reasons: &mut Vec<String>, signals: &SenderSignals, now: i64) {
    if signals.paid_count > 0 && signals.spam_rate > 0.0 {
        *bps += js_round(signals.spam_rate * 4.0 * ONE);
        reasons.push(format!(
            "Spam-reported {} of {} times here",
            signals.spam_reports, signals.paid_count
        ));
    }
    if signals.paid_count >= 3 && signals.spam_rate < 0.2 {
        *bps = js_round(*bps * 0.5);
        reasons.push(format!(
            "Good history here: {} messages",
            signals.paid_count
        ));
    }
    if signals.ens_names > 0 {
        *bps = js_round(*bps * 0.7);
        let plural = if signals.ens_names == 1 { "" } else { "s" };
        reasons.push(format!("Holds {} ENS name{plural}", signals.ens_names));

        let age = signals
            .oldest_ens_at
            .map_or(0, |registered| now.saturating_sub(registered));
        if age > YEAR_SECONDS {
            *bps = js_round(*bps * 0.8);
            reasons.push(format!(
                "Oldest registered {} years ago",
                age / YEAR_SECONDS
            ));
        }
    }
}

/// `floor * bps / 10_000` with integer division, split so a very large floor
/// cannot overflow the intermediate product.
fn scale_by_bps(floor: u128, bps: u32) -> u128 {
    let bps = u128::from(bps);
    let one = ONE as u128;
    (floor / one).saturating_mul(bps) + (floor % one) * bps / one
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const FLOOR: u128 = 1000;
    const NOW: i64 = 1_800_000_000;
    const YEAR: i64 = YEAR_SECONDS;

    fn two_years_ago() -> i64 {
        NOW - 2 * YEAR
    }

    /// Every field defaults to "no signal at all" so a test only has to name
    /// the ones it cares about.
    fn signals(edit: impl FnOnce(&mut SenderSignals)) -> SenderSignals {
        let mut base = SenderSignals {
            paid_count: 0,
            spam_reports: 0,
            spam_rate: 0.0,
            ens_names: 0,
            oldest_ens_at: None,
        };
        edit(&mut base);
        base
    }

    fn price(tier: Tier, s: Option<&SenderSignals>, degraded: bool) -> Quote {
        quote(FLOOR, tier, s, degraded, NOW)
    }

    #[test]
    fn important_mail_is_free_and_ignores_every_signal_that_would_otherwise_raise_its_price() {
        let s = signals(|s| {
            s.spam_reports = 50;
            s.spam_rate = 1.0;
        });
        let result = price(Tier::Important, Some(&s), true);

        assert!(result.free);
        assert_eq!(result.amount, 0);
        assert_eq!(result.multiplier_bps, 0);
        assert_eq!(result.floor, FLOOR);
    }

    #[test]
    fn human_and_commercial_mail_price_identically_at_the_floor_when_nothing_is_known() {
        let human = price(Tier::Human, None, false);
        let commercial = price(Tier::Commercial, None, false);

        assert_eq!(human.amount, FLOOR);
        assert_eq!(commercial.amount, FLOOR);
        assert!(!human.free);
    }

    #[test]
    fn dangerous_mail_is_priced_ten_times_the_floor_by_default() {
        assert_eq!(price(Tier::Dangerous, None, false).amount, 10_000);
    }

    #[test]
    fn a_degraded_verdict_downgrades_dangerous_mail_from_ten_times_the_floor_to_two_times() {
        assert_eq!(price(Tier::Dangerous, None, true).amount, 2000);
    }

    #[test]
    fn the_header_only_downgrade_applies_only_to_the_dangerous_tier() {
        assert_eq!(price(Tier::Human, None, true).amount, FLOOR);
    }

    #[test]
    fn a_sender_with_no_reported_spam_adds_no_surcharge() {
        let s = signals(|s| s.paid_count = 1);
        assert_eq!(price(Tier::Human, Some(&s), false).amount, FLOOR);
    }

    #[test]
    fn a_spam_rate_on_past_messages_adds_a_surcharge_proportional_to_that_rate() {
        let s = signals(|s| {
            s.paid_count = 1;
            s.spam_reports = 1;
            s.spam_rate = 0.1;
        });
        assert_eq!(price(Tier::Human, Some(&s), false).amount, 1400);
    }

    #[test]
    fn fewer_than_three_paid_messages_earns_no_loyalty_discount_however_clean_the_record() {
        let s = signals(|s| s.paid_count = 2);
        assert_eq!(price(Tier::Dangerous, Some(&s), false).amount, 10_000);
    }

    #[test]
    fn three_or_more_well_received_messages_earn_a_loyalty_discount() {
        let s = signals(|s| s.paid_count = 3);
        assert_eq!(price(Tier::Dangerous, Some(&s), false).amount, 5000);
    }

    #[test]
    fn a_spam_rate_exactly_at_the_loyalty_threshold_forfeits_the_discount_a_rate_just_under_keeps()
    {
        let just_under = signals(|s| {
            s.paid_count = 3;
            s.spam_rate = 0.19;
        });
        let at_threshold = signals(|s| {
            s.paid_count = 3;
            s.spam_rate = 0.2;
        });

        assert_eq!(
            price(Tier::Dangerous, Some(&just_under), false).amount,
            5380
        );
        assert_eq!(
            price(Tier::Dangerous, Some(&at_threshold), false).amount,
            10_000
        );
    }

    #[test]
    fn an_ens_name_earns_its_own_discount() {
        let s = signals(|s| {
            s.ens_names = 1;
            s.oldest_ens_at = Some(NOW - 10);
        });
        assert_eq!(price(Tier::Dangerous, Some(&s), false).amount, 7000);
    }

    #[test]
    fn an_ens_name_registered_over_a_year_ago_earns_a_deeper_discount_than_a_fresh_one() {
        let s = signals(|s| {
            s.ens_names = 1;
            s.oldest_ens_at = Some(two_years_ago());
        });
        assert_eq!(price(Tier::Dangerous, Some(&s), false).amount, 5600);
    }

    #[test]
    fn the_loyalty_and_ens_discounts_compound_rather_than_override_each_other() {
        let loyalty = signals(|s| s.paid_count = 3);
        let ens = signals(|s| {
            s.ens_names = 1;
            s.oldest_ens_at = Some(NOW - 10);
        });
        let both = signals(|s| {
            s.paid_count = 3;
            s.ens_names = 1;
            s.oldest_ens_at = Some(NOW - 10);
        });

        let loyalty_only = price(Tier::Dangerous, Some(&loyalty), false);
        let ens_only = price(Tier::Dangerous, Some(&ens), false);
        let stacked = price(Tier::Dangerous, Some(&both), false);

        assert_eq!(stacked.amount, 3500);
        assert!(stacked.amount < loyalty_only.amount && stacked.amount < ens_only.amount);
    }

    #[test]
    fn combined_discounts_can_never_cut_a_price_below_the_inboxs_floor() {
        let s = signals(|s| {
            s.paid_count = 3;
            s.ens_names = 1;
            s.oldest_ens_at = Some(two_years_ago());
        });
        let result = price(Tier::Human, Some(&s), false);

        assert_eq!(result.amount, FLOOR);
        assert_eq!(result.multiplier_bps, 10_000);
    }

    #[test]
    fn a_spam_surcharge_can_never_push_a_price_past_the_inboxs_ceiling() {
        let high = signals(|s| {
            s.paid_count = 1;
            s.spam_rate = 1.0;
        });
        let extreme = signals(|s| {
            s.paid_count = 1;
            s.spam_rate = 5.0;
        });

        assert_eq!(price(Tier::Dangerous, Some(&high), false).amount, 10_000);
        assert_eq!(price(Tier::Dangerous, Some(&extreme), false).amount, 10_000);
    }

    #[test]
    fn a_zero_floor_prices_every_tier_at_zero_but_the_quote_is_still_not_free() {
        let result = quote(0, Tier::Commercial, None, false, NOW);

        assert_eq!(result.amount, 0);
        assert!(!result.free);
    }

    #[test]
    fn an_oldest_ens_at_timestamp_of_zero_earns_the_age_discount_not_null() {
        let with_zero = signals(|s| {
            s.ens_names = 1;
            s.oldest_ens_at = Some(0);
        });
        let with_null = signals(|s| s.ens_names = 1);

        assert_eq!(price(Tier::Dangerous, Some(&with_zero), false).amount, 5600);
        assert_eq!(price(Tier::Dangerous, Some(&with_null), false).amount, 7000);
    }

    fn big(value: &Value) -> u128 {
        value.as_str().unwrap().parse().unwrap()
    }

    #[test]
    fn every_golden_vector_matches_the_typescript_output() {
        let golden: Value =
            serde_json::from_str(include_str!("../../../fixtures/golden/pricing.json")).unwrap();
        let now = golden["config"]["now"].as_i64().unwrap();
        let cases = golden["cases"].as_array().unwrap();
        assert!(!cases.is_empty());

        for case in cases {
            let name = case["name"].as_str().unwrap();
            let input = &case["input"];
            let tier: Tier = serde_json::from_value(input["tier"].clone()).unwrap();
            let parsed: Option<SenderSignals> =
                serde_json::from_value(input["signals"].clone()).unwrap();
            let degraded = input["degraded"].as_bool().unwrap();

            let got = quote(big(&input["floor"]), tier, parsed.as_ref(), degraded, now);

            let out = &case["output"];
            assert_eq!(got.amount, big(&out["amount"]), "{name}: amount");
            assert_eq!(got.floor, big(&out["floor"]), "{name}: floor");
            assert_eq!(
                u64::from(got.multiplier_bps),
                out["multiplierBps"].as_u64().unwrap(),
                "{name}: multiplierBps"
            );
            assert_eq!(got.free, out["free"].as_bool().unwrap(), "{name}: free");
            let reasons: Vec<&str> = out["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r.as_str().unwrap())
                .collect();
            assert_eq!(got.reasons, reasons, "{name}: reasons");
        }
    }
}

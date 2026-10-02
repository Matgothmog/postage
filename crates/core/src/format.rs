use thiserror::Error;

/// Decimals of the token the escrow is priced in (`USDC_DECIMALS` in the web
/// app's contract constants).
pub const USDC_DECIMALS: u32 = 18;

const UNIT: u128 = 10u128.pow(USDC_DECIMALS);

/// Why a user-typed amount could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseUsdcError {
    #[error("amount is empty")]
    Empty,
    #[error("amount {0:?} is not a non-negative decimal number")]
    Invalid(String),
    #[error("amount {0:?} is too large")]
    TooLarge(String),
}

/// `toFixed(2)` for a non-negative value, rounding exact ties up the way
/// JavaScript does (Rust rounds them to even). A tie at two decimals is only
/// representable when the value is an odd multiple of 1/8.
fn to_fixed_two(value: f64) -> String {
    let eighths = value * 8.0;
    let is_exact_tie = eighths.fract() == 0.0 && eighths % 2.0 == 1.0;
    if is_exact_tie {
        return format!("{:.2}", (value * 100.0).round() / 100.0);
    }
    format!("{value:.2}")
}

/// The amount as a float of whole units, parsed from its exact decimal string
/// so the result is the nearest double, as `Number(formatUnits(..))` gives.
fn units_as_float(amount: u128) -> f64 {
    let text = format!("{}.{:018}", amount / UNIT, amount % UNIT);
    text.parse().unwrap_or(f64::INFINITY)
}

/// Postage is small enough that the useful unit is cents, not dollars.
pub fn format_usdc(amount: u128) -> String {
    let value = units_as_float(amount);
    if value == 0.0 {
        return "free".to_owned();
    }
    if value < 0.01 {
        return format!("{}c", to_fixed_two(value * 100.0));
    }
    format!("${}", to_fixed_two(value))
}

/// Reads a decimal amount such as `"1.5"` into base units. Digits beyond the
/// token's precision round half up, as viem's `parseUnits` does.
pub fn parse_usdc(input: &str) -> Result<u128, ParseUsdcError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ParseUsdcError::Empty);
    }
    let invalid = || ParseUsdcError::Invalid(trimmed.to_owned());
    let too_large = || ParseUsdcError::TooLarge(trimmed.to_owned());

    let (whole, fraction) = trimmed.split_once('.').unwrap_or((trimmed, ""));
    let all_digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if (whole.is_empty() && fraction.is_empty()) || !all_digits(whole) || !all_digits(fraction) {
        return Err(invalid());
    }

    let precision = USDC_DECIMALS as usize;
    let (kept, dropped) = fraction.split_at(fraction.len().min(precision));
    let padded = format!("{kept:0<precision$}");
    let whole_units: u128 = if whole.is_empty() {
        0
    } else {
        whole.parse().map_err(|_| too_large())?
    };
    let fraction_units: u128 = padded.parse().map_err(|_| invalid())?;

    let rounds_up = dropped.starts_with(|c: char| c >= '5');
    whole_units
        .checked_mul(UNIT)
        .and_then(|base| base.checked_add(fraction_units))
        .and_then(|total| total.checked_add(u128::from(rounds_up)))
        .ok_or_else(too_large)
}

/// `0x1234...abcd`: the first six and last four characters.
pub fn short_address(address: &str) -> String {
    let characters: Vec<char> = address.chars().collect();
    let head: String = characters.iter().take(6).collect();
    let tail: String = characters[characters.len().saturating_sub(4)..]
        .iter()
        .collect();
    format!("{head}...{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENT: u128 = UNIT / 100;

    #[test]
    fn zero_is_free() {
        assert_eq!(format_usdc(0), "free");
    }

    #[test]
    fn whole_dollars_show_two_decimals() {
        assert_eq!(format_usdc(UNIT), "$1.00");
        assert_eq!(format_usdc(12 * UNIT + 34 * CENT), "$12.34");
    }

    #[test]
    fn one_cent_is_the_dollar_threshold() {
        assert_eq!(format_usdc(CENT), "$0.01");
    }

    #[test]
    fn below_one_cent_is_shown_in_cents() {
        assert_eq!(format_usdc(CENT / 2), "0.50c");
        assert_eq!(format_usdc(CENT / 10), "0.10c");
    }

    #[test]
    fn exact_ties_round_up_like_javascript() {
        // 0.125 USD is an exact tie at two decimals; JS toFixed gives 0.13.
        assert_eq!(format_usdc(UNIT / 8), "$0.13");
    }

    #[test]
    fn inexact_halves_follow_the_double_not_the_decimal() {
        // 1.005 is stored just below the tie, so JS toFixed gives 1.00.
        assert_eq!(format_usdc(UNIT + UNIT / 200), "$1.00");
    }

    #[test]
    fn parse_reads_whole_and_fractional_amounts() {
        assert_eq!(parse_usdc("1"), Ok(UNIT));
        assert_eq!(parse_usdc(" 0.25 "), Ok(UNIT / 4));
        assert_eq!(parse_usdc(".5"), Ok(UNIT / 2));
    }

    #[test]
    fn parse_rounds_digits_beyond_the_token_precision() {
        assert_eq!(parse_usdc("0.0000000000000000005"), Ok(1));
        assert_eq!(parse_usdc("0.0000000000000000004"), Ok(0));
    }

    #[test]
    fn parse_rejects_empty_and_malformed_input() {
        assert_eq!(parse_usdc("  "), Err(ParseUsdcError::Empty));
        assert!(matches!(parse_usdc("abc"), Err(ParseUsdcError::Invalid(_))));
        assert!(matches!(parse_usdc("-1"), Err(ParseUsdcError::Invalid(_))));
        assert!(matches!(
            parse_usdc("1.2.3"),
            Err(ParseUsdcError::Invalid(_))
        ));
        assert!(matches!(parse_usdc("."), Err(ParseUsdcError::Invalid(_))));
    }

    #[test]
    fn parse_rejects_amounts_beyond_the_integer_range() {
        assert!(matches!(
            parse_usdc("999999999999999999999999999999"),
            Err(ParseUsdcError::TooLarge(_))
        ));
    }

    #[test]
    fn short_address_keeps_the_first_six_and_last_four() {
        assert_eq!(
            short_address("0x1234567890abcdef1234567890abcdef12345678"),
            "0x1234...5678"
        );
    }

    #[test]
    fn short_address_of_a_short_string_overlaps_like_slice_does() {
        assert_eq!(short_address("abc"), "abc...abc");
    }
}

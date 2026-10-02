//! JavaScript's `String(number)`, for text that both a browser and this code
//! have to produce byte for byte.
//!
//! A signed statement carries its timestamp as text, and the TypeScript
//! verifier rendered whatever `Number()` made of a header with a template
//! literal. A signature over `Issued: 1757332800.5`, or over the decimal a
//! `0x...` header converts to, has to verify here exactly as it did there, so
//! the rendering follows ECMAScript's `Number::toString` rather than Rust's
//! `Display`, which writes `inf`, `-0` and never switches to an exponent.

/// Past this many integer digits JavaScript writes an exponent (`1e+21`).
const MAX_PLAIN_INTEGER_DIGITS: i32 = 21;

/// Down to this many leading fractional zeros it still writes a plain decimal
/// (`0.000001`); one more and it writes `1e-7`.
const MIN_PLAIN_FRACTION_EXPONENT: i32 = -6;

/// `String(value)` as JavaScript writes it.
pub fn to_js_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == 0.0 {
        // Both zeros: JavaScript writes negative zero as "0".
        return "0".to_owned();
    }
    if value.is_infinite() {
        let sign = if value < 0.0 { "-" } else { "" };
        return format!("{sign}Infinity");
    }
    if value < 0.0 {
        return format!("-{}", to_js_string(-value));
    }

    let (digits, point) = shortest_digits(value);
    place_point(&digits, point)
}

/// The shortest digit string that reads back as `value`, and where the decimal
/// point sits relative to its first digit (ECMAScript's `k` digits and `n`).
///
/// Rust's `{:e}` already prints the shortest round-tripping digits, the same
/// ones JavaScript chooses, so only the layout around them is left to do.
fn shortest_digits(value: f64) -> (String, i32) {
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .unwrap_or((scientific.as_str(), "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    // `{:e}` of a finite f64 always has an exponent in i32 range.
    let exponent: i32 = exponent.parse().unwrap_or(0);
    (digits, exponent + 1)
}

/// Number::toString's four layouts, chosen by where the point falls.
fn place_point(digits: &str, point: i32) -> String {
    let length = i32::try_from(digits.len()).unwrap_or(i32::MAX);

    if length <= point && point <= MAX_PLAIN_INTEGER_DIGITS {
        return format!("{digits}{}", zeros(point - length));
    }
    if 0 < point && point <= MAX_PLAIN_INTEGER_DIGITS {
        let (whole, fraction) = digits.split_at(index(point));
        return format!("{whole}.{fraction}");
    }
    if MIN_PLAIN_FRACTION_EXPONENT < point && point <= 0 {
        return format!("0.{}{digits}", zeros(-point));
    }

    let exponent = point - 1;
    let sign = if exponent < 0 { '-' } else { '+' };
    let magnitude = exponent.unsigned_abs();
    let (first, rest) = digits.split_at(1);
    if rest.is_empty() {
        format!("{first}e{sign}{magnitude}")
    } else {
        format!("{first}.{rest}e{sign}{magnitude}")
    }
}

fn zeros(count: i32) -> String {
    "0".repeat(index(count))
}

fn index(count: i32) -> usize {
    usize::try_from(count).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from Node's `String(x)` for each value.
    #[test]
    fn numbers_render_as_javascript_writes_them() {
        let cases: [(f64, &str); 21] = [
            (1_757_332_800.0, "1757332800"),
            (1_757_332_800.5, "1757332800.5"),
            (1e21, "1e+21"),
            (1e-7, "1e-7"),
            (123e-20, "1.23e-18"),
            (0.000_001, "0.000001"),
            (1.5e300, "1.5e+300"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (0.1 + 0.2, "0.30000000000000004"),
            (100.0, "100"),
            (1e20, "100000000000000000000"),
            (-1_757_332_800.25, "-1757332800.25"),
            (1_757_332_800.123, "1757332800.123"),
            (123_456_789_012_345_680_000.0, "123456789012345680000"),
            (0.000_001_5, "0.0000015"),
            (0.000_012_34, "0.00001234"),
            (-1e-7, "-1e-7"),
            (9_007_199_254_740_992.0, "9007199254740992"),
            (4.35, "4.35"),
            (12.0, "12"),
        ];
        for (value, expected) in cases {
            assert_eq!(to_js_string(value), expected, "{value:e}");
        }
    }

    #[test]
    fn the_special_values_are_spelled_as_javascript_spells_them() {
        assert_eq!(to_js_string(f64::NAN), "NaN");
        assert_eq!(to_js_string(f64::INFINITY), "Infinity");
        assert_eq!(to_js_string(f64::NEG_INFINITY), "-Infinity");
        assert_eq!(to_js_string(0.0), "0");
        assert_eq!(to_js_string(-0.0), "0");
    }
}

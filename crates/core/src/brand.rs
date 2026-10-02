//! Palette for the challenge email. The email is a runtime-built HTML string
//! that never sees the app stylesheet, so its colors live here once instead of
//! as hex literals in the markup.
//!
//! Deliberately a LIGHT palette: Gmail on Android inverts dark email bodies and
//! some Outlook builds drop CSS backgrounds with no `bgcolor` attribute. `ACCENT`
//! is `#1D4ED8` (the app's `--accent-lo`), not `#3B82F6`: on white the latter is
//! 3.68:1 and fails WCAG AA for text, while `#1D4ED8` is 6.3:1, as is white text
//! on an `#1D4ED8` fill. If the app palette moves, re-check these by hand.

/// Page background.
pub const BG: &str = "#F5F8FF";
/// The message panel.
pub const CARD: &str = "#FFFFFF";
/// Body text.
pub const INK: &str = "#0F172A";
/// Secondary text.
pub const INK_SOFT: &str = "#475569";
/// Buttons, links.
pub const ACCENT: &str = "#1D4ED8";
/// Rules and borders.
pub const LINE: &str = "#DBE4F0";
/// Label text on an accent-filled button.
pub const ON_ACCENT: &str = "#FFFFFF";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_color_is_an_uppercase_six_digit_hex() {
        for color in [BG, CARD, INK, INK_SOFT, ACCENT, LINE, ON_ACCENT] {
            assert_eq!(color.len(), 7, "{color}");
            assert!(color.starts_with('#'), "{color}");
            assert!(
                color[1..]
                    .chars()
                    .all(|c| matches!(c, '0'..='9' | 'A'..='F')),
                "{color}"
            );
        }
    }

    #[test]
    fn accent_is_the_wcag_safe_blue() {
        assert_eq!(ACCENT, "#1D4ED8");
    }
}

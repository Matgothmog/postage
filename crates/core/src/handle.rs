/// The domain the gateway receives mail for, and the things anyone needs to do
/// with it.
pub const MAIL_DOMAIN: &str = "usepostage.com";

pub fn postage_address(handle: &str) -> String {
    format!("{}@{MAIL_DOMAIN}", handle.to_lowercase())
}

/// The handle an address points at, or "" if it names no local part.
pub fn handle_of(address: &str) -> String {
    address.split('@').next().unwrap_or_default().to_lowercase()
}

/// Whether this address is one of ours. A destination that is means mail
/// forwarded to it comes straight back, and every lap spends a classify call, a
/// chain read and a challenge row.
///
/// Subdomains count too, and so does the fully qualified form with trailing
/// dots (`you@usepostage.com.`), which resolves to the same host.
pub fn is_ours(address: &str) -> bool {
    let address = address.trim_end_matches('.').to_lowercase();
    let Some((_, domain)) = address.rsplit_once('@') else {
        return false;
    };
    domain == MAIL_DOMAIN
        || domain
            .strip_suffix(MAIL_DOMAIN)
            .is_some_and(|prefix| prefix.ends_with('.') && prefix.len() > 1)
}

pub const HANDLE_MIN_LENGTH: usize = 2;
pub const HANDLE_MAX_LENGTH: usize = 31;

fn is_handle_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

fn is_punctuation(c: char) -> bool {
    matches!(c, '.' | '_' | '-')
}

/// The shape a handle must have once lowercased: starts and ends on a letter
/// or digit, with only letters, digits, dot, underscore and dash between.
/// Length and the repeated-dot rule are checked separately, since the server
/// reports each as its own error message.
pub fn has_handle_shape(handle: &str) -> bool {
    let is_lower_alnum = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let (Some(first), Some(last)) = (handle.chars().next(), handle.chars().next_back()) else {
        return false;
    };
    is_lower_alnum(first)
        && is_lower_alnum(last)
        && handle
            .chars()
            .all(|c| is_lower_alnum(c) || is_punctuation(c))
}

pub fn has_repeated_dot(handle: &str) -> bool {
    handle.contains("..")
}

/// The single predicate a handle must pass, all rules together.
pub fn is_valid_handle(handle: &str) -> bool {
    let length = handle.chars().count();
    (HANDLE_MIN_LENGTH..=HANDLE_MAX_LENGTH).contains(&length)
        && has_handle_shape(handle)
        && !has_repeated_dot(handle)
}

/// Strips a raw string down to the characters a handle may contain, leaving
/// case untouched: what the claim form's live input filter needs.
pub fn keep_handle_chars(raw: &str) -> String {
    raw.chars().filter(|&c| is_handle_char(c)).collect()
}

fn collapse_dot_runs(value: &str) -> String {
    let mut collapsed = String::with_capacity(value.len());
    for c in value.chars() {
        if c == '.' && collapsed.ends_with('.') {
            continue;
        }
        collapsed.push(c);
    }
    collapsed
}

/// Turns free text (typically an email's local part) into a handle that
/// already satisfies [`is_valid_handle`]: lowercases, drops characters outside
/// the charset, collapses runs of dots, and trims punctuation off both ends,
/// including after the length cap, since truncating alone can land the cut on
/// a dot or dash.
pub fn normalize_handle(raw: &str) -> String {
    let cleaned = collapse_dot_runs(&keep_handle_chars(&raw.to_lowercase()));
    let cleaned = cleaned.trim_matches(is_punctuation);
    // Everything left is ASCII, so cutting at a character count is safe.
    let capped: String = cleaned.chars().take(HANDLE_MAX_LENGTH).collect();
    let truncated = capped.trim_matches(is_punctuation);
    if truncated.chars().count() >= HANDLE_MIN_LENGTH {
        truncated.to_owned()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_on_our_own_domain_is_recognised_as_ours() {
        assert!(is_ours(&format!("someone@{MAIL_DOMAIN}")));
    }

    #[test]
    fn an_address_on_any_other_domain_is_not_ours() {
        assert!(!is_ours("someone@gmail.com"));
    }

    #[test]
    fn a_domain_that_merely_ends_with_ours_as_a_suffix_is_not_ours() {
        assert!(!is_ours(&format!("someone@evil-{MAIL_DOMAIN}")));
    }

    #[test]
    fn recognising_our_domain_is_case_insensitive() {
        assert!(is_ours(&format!("someone@{}", MAIL_DOMAIN.to_uppercase())));
    }

    #[test]
    fn subdomains_of_ours_are_ours() {
        assert!(is_ours("a@sub.usepostage.com"));
        assert!(is_ours("a@a.b.usepostage.com"));
    }

    #[test]
    fn trailing_dots_and_mixed_case_do_not_hide_our_domain() {
        assert!(is_ours("a@usepostage.com."));
        assert!(is_ours("a@usepostage.com.."));
        assert!(is_ours("a@Sub.UsePostage.COM."));
    }

    #[test]
    fn lookalike_domains_are_not_ours() {
        assert!(!is_ours("a@usepostage.com.evil.com"));
        assert!(!is_ours("a@xusepostage.com"));
        assert!(!is_ours("a@.usepostage.com"));
        assert!(!is_ours("a@usepostage.co"));
    }

    #[test]
    fn an_address_with_no_domain_at_all_is_not_ours() {
        assert!(!is_ours("not-an-address"));
    }

    #[test]
    fn postage_address_lowercases_the_handle_before_combining_it_with_the_domain() {
        assert_eq!(postage_address("Demo"), format!("demo@{MAIL_DOMAIN}"));
    }

    #[test]
    fn handle_of_reads_the_local_part_back_off_an_address_it_produced() {
        assert_eq!(handle_of(&postage_address("Demo")), "demo");
    }

    #[test]
    fn handle_of_reports_no_handle_for_an_address_with_no_local_part() {
        assert_eq!(handle_of(&format!("@{MAIL_DOMAIN}")), "");
    }

    #[test]
    fn a_handle_within_the_charset_and_length_is_valid() {
        assert!(is_valid_handle("demo.user-1"));
    }

    #[test]
    fn a_handle_shorter_than_the_minimum_is_invalid() {
        assert!(!is_valid_handle("a"));
    }

    #[test]
    fn a_handle_longer_than_the_maximum_is_invalid() {
        assert!(!is_valid_handle(&"a".repeat(HANDLE_MAX_LENGTH + 1)));
    }

    #[test]
    fn a_handle_starting_with_punctuation_is_invalid() {
        assert!(!is_valid_handle("-demo"));
    }

    #[test]
    fn a_handle_ending_with_punctuation_is_invalid() {
        assert!(!is_valid_handle("demo-"));
    }

    #[test]
    fn a_handle_with_a_character_outside_the_charset_is_invalid() {
        assert!(!is_valid_handle("demo user"));
    }

    #[test]
    fn a_handle_with_two_dots_running_together_is_invalid() {
        assert!(!is_valid_handle("demo..user"));
    }

    #[test]
    fn uppercase_is_not_part_of_the_valid_shape_callers_lowercase_first() {
        assert!(!is_valid_handle("Demo"));
    }

    #[test]
    fn has_repeated_dot_reports_two_adjacent_dots_anywhere_in_the_string() {
        assert!(has_repeated_dot("a..b"));
        assert!(!has_repeated_dot("a.b.c"));
    }

    #[test]
    fn keep_handle_chars_drops_characters_outside_the_handle_charset() {
        assert_eq!(keep_handle_chars("de mo!user@x"), "demouserx");
    }

    #[test]
    fn keep_handle_chars_preserves_case_unlike_the_final_validity_rule() {
        assert_eq!(keep_handle_chars("DemoUser"), "DemoUser");
    }

    #[test]
    fn normalize_handle_lowercases_and_strips_characters_outside_the_charset() {
        assert_eq!(normalize_handle("John.Doe+Promo"), "john.doepromo");
    }

    #[test]
    fn normalize_handle_collapses_runs_of_dots_into_one() {
        assert_eq!(normalize_handle("john...doe"), "john.doe");
    }

    #[test]
    fn normalize_handle_trims_punctuation_off_both_ends() {
        assert_eq!(normalize_handle("-.john.doe._-"), "john.doe");
    }

    #[test]
    fn normalize_handle_reports_no_suggestion_once_trimming_leaves_it_too_short() {
        assert_eq!(normalize_handle("."), "");
        assert_eq!(normalize_handle("a"), "");
    }

    #[test]
    fn normalize_handle_never_hands_back_more_than_the_maximum_length() {
        let result = normalize_handle(&"a".repeat(HANDLE_MAX_LENGTH + 10));
        assert!(result.len() <= HANDLE_MAX_LENGTH);
    }

    #[test]
    fn normalize_handle_re_trims_punctuation_exposed_by_truncating_to_the_length_cap() {
        let local = format!("{}-{}", "a".repeat(HANDLE_MAX_LENGTH - 1), "b".repeat(5));
        let result = normalize_handle(&local);
        assert_eq!(result, "a".repeat(HANDLE_MAX_LENGTH - 1));
        assert!(is_valid_handle(&result));
    }

    #[test]
    fn normalize_handle_output_always_satisfies_is_valid_handle_for_anything_it_accepts() {
        let samples = [
            "john.doe".to_owned(),
            "a".repeat(50),
            "-leading-and-trailing-".to_owned(),
            "dots....everywhere....".to_owned(),
            "MiXeD-CaSe.Name".to_owned(),
            format!("{}..", "a".repeat(29)),
        ];
        for sample in samples {
            let result = normalize_handle(&sample);
            if result.is_empty() {
                continue;
            }
            assert!(
                is_valid_handle(&result),
                "normalize_handle({sample:?}) -> {result:?}"
            );
        }
    }
}

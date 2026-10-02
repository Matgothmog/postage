//! What the receiving MTA concluded about the sender, read out of
//! `Authentication-Results` (replaces `authResults` in `worker/src/index.ts`).
//!
//! A sender may write this header themselves, and the runtime joins every copy
//! into one string with ", ", so a naive scan of that string can return
//! whichever copy happens to mention a method first. Cloudflare's own copy is
//! the one that means anything; the rest are the sender's claims about the
//! sender. The two cannot be told apart from here, so a method is believed only
//! when every copy agrees on it. A sender who adds `dmarc=pass` to a message
//! Cloudflare marked `dmarc=fail` contradicts it and comes away with nothing.
//!
//! What is left is a message for which Cloudflare recorded no result at all:
//! there is then nothing to disagree with, and a forged claim stands. That is
//! narrow, because a forged claim only buys anything if Cloudflare stated
//! neither dmarc nor spf. Closing it needs Cloudflare's authserv-id, which is
//! not established.
//!
//! A method name has to start a token, which is more than a word boundary asks
//! for. `policy.dmarc=none` states the domain's published policy and
//! `x-dkim=fail` is a vendor's own method: both are ordinary RFC 8601 syntax,
//! and both put a word boundary immediately in front of a method name. Only a
//! word character, a dot or a hyphen can put a name inside a larger token, so
//! those are what the byte in front of a match must not be.
//!
//! This is a scan rather than a parse: text inside a quoted `reason=` is read
//! like anything else. Reading too much can only add a value to the set, which
//! either agrees or loses the method, so it errs toward unknown.

use std::collections::BTreeSet;

/// The three results the gateway is told, each `None` when unknown.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthResults {
    pub spf: Option<String>,
    pub dkim: Option<String>,
    pub dmarc: Option<String>,
}

pub fn auth_results(header: Option<&str>) -> AuthResults {
    let header = header.unwrap_or("");
    AuthResults {
        spf: read_method(header, "spf"),
        dkim: read_method(header, "dkim"),
        dmarc: read_method(header, "dmarc"),
    }
}

/// JavaScript's `\w`: ASCII letters, digits and underscore, with no Unicode
/// mode, so a non-ASCII letter ends a value exactly where it did in the worker.
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn can_start_a_method(previous: Option<u8>) -> bool {
    match previous {
        Some(byte) => !(is_word_byte(byte) || byte == b'.' || byte == b'-'),
        None => true,
    }
}

fn read_method(header: &str, method: &str) -> Option<String> {
    // ASCII lowercasing keeps every byte offset, so positions found in the
    // lowered copy index the original too.
    let lowered = header.to_ascii_lowercase();
    let needle = format!("{method}=");
    let bytes = lowered.as_bytes();

    let mut claimed: BTreeSet<&str> = BTreeSet::new();
    let mut from = 0;
    while let Some(found) = lowered[from..].find(&needle) {
        let start = from + found;
        let value_start = start + needle.len();
        let previous = start.checked_sub(1).map(|index| bytes[index]);
        let value_len = bytes[value_start..]
            .iter()
            .take_while(|byte| is_word_byte(**byte))
            .count();
        if can_start_a_method(previous) && value_len > 0 {
            claimed.insert(&lowered[value_start..value_start + value_len]);
            from = value_start + value_len;
        } else {
            from = value_start;
        }
    }

    if claimed.len() == 1 {
        claimed.into_iter().next().map(str::to_owned)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLEAN: &str = "mx.cloudflare.net; dkim=pass header.d=example.com header.i=@example.com; \
         spf=pass smtp.mailfrom=sender@example.com; dmarc=pass header.from=example.com";

    fn results(spf: Option<&str>, dkim: Option<&str>, dmarc: Option<&str>) -> AuthResults {
        AuthResults {
            spf: spf.map(str::to_owned),
            dkim: dkim.map(str::to_owned),
            dmarc: dmarc.map(str::to_owned),
        }
    }

    #[test]
    fn a_single_header_from_the_receiving_mta_is_read_straight_through() {
        assert_eq!(
            auth_results(Some(CLEAN)),
            results(Some("pass"), Some("pass"), Some("pass"))
        );
    }

    #[test]
    fn results_are_read_case_insensitively_and_reported_lowercase() {
        assert_eq!(
            auth_results(Some("mx; SPF=Pass; DKIM=FAIL; DMARC=None")),
            results(Some("pass"), Some("fail"), Some("none"))
        );
    }

    /// The regression the TypeScript suite was written for: `policy.dmarc` is a
    /// registered RFC 8601 property, so an MTA stating the domain's published
    /// policy beside its verdict is writing ordinary header syntax.
    #[test]
    fn a_policy_property_does_not_contradict_the_result_it_belongs_to() {
        let found = auth_results(Some("mx; dmarc=pass policy.dmarc=none; spf=pass"));
        assert_eq!(found.dmarc.as_deref(), Some("pass"));
    }

    #[test]
    fn a_header_as_a_real_mta_writes_it_reads_all_three_methods() {
        let header = "mx.google.com; dkim=pass header.i=@example.com header.b=\"AbC\"; \
             spf=pass (google.com: domain of sender@example.com designates 1.2.3.4) \
             smtp.mailfrom=sender@example.com; \
             dmarc=pass (p=NONE sp=QUARANTINE dis=NONE) policy.dmarc=none header.from=example.com";
        assert_eq!(
            auth_results(Some(header)),
            results(Some("pass"), Some("pass"), Some("pass"))
        );
    }

    #[test]
    fn a_vendors_own_x_method_does_not_contradict_the_standard_one() {
        let found = auth_results(Some("mx; dkim=pass header.d=example.com; x-dkim=fail"));
        assert_eq!(found.dkim.as_deref(), Some("pass"));
    }

    #[test]
    fn a_property_whose_name_ends_in_a_method_name_is_not_read_as_a_result() {
        assert_eq!(
            auth_results(Some("mx; spf=pass smtp.mailfrom=a@b.com policy.spf=none")),
            results(Some("pass"), None, None)
        );
    }

    #[test]
    fn copies_that_agree_are_believed() {
        let found = auth_results(Some("mx1; dmarc=pass, mx2; dmarc=pass"));
        assert_eq!(found.dmarc.as_deref(), Some("pass"));
    }

    #[test]
    fn a_copy_contradicting_another_leaves_the_method_unknown() {
        assert_eq!(
            auth_results(Some("mx1; dmarc=pass; spf=pass, mx2; dmarc=fail")),
            results(Some("pass"), None, None)
        );
    }

    /// The gap the module docs describe, in executable form: with no result from
    /// the receiving MTA there is nothing for a forged claim to disagree with.
    /// Recorded so that closing it is a visible change here rather than a silent
    /// one.
    #[test]
    fn a_lone_claim_stands_when_no_other_copy_speaks_to_it() {
        let found = auth_results(Some("mx; dmarc=pass"));
        assert_eq!(found.dmarc.as_deref(), Some("pass"));
    }

    #[test]
    fn a_message_with_no_authentication_header_authenticates_nothing() {
        assert_eq!(auth_results(None), AuthResults::default());
    }

    #[test]
    fn a_header_naming_no_method_authenticates_nothing() {
        assert_eq!(
            auth_results(Some("mx.cloudflare.net; none")),
            AuthResults::default()
        );
    }

    #[test]
    fn a_truncated_method_with_no_result_is_not_a_result() {
        assert_eq!(
            auth_results(Some("mx; dmarc= ; spf=")),
            AuthResults::default()
        );
    }

    #[test]
    fn an_empty_header_authenticates_nothing() {
        assert_eq!(auth_results(Some("")), AuthResults::default());
    }
}

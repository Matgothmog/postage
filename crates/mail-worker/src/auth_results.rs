//! What Cloudflare's receiving MTA concluded about the sender, read out of the
//! `Authentication-Results` it stamped (replaces `authResults` in
//! `worker/src/index.ts`).
//!
//! A sender can write this header themselves, so a value is believed only from
//! a header whose authserv-id (RFC 8601: the first token before the first `;`)
//! is [`CLOUDFLARE_AUTHSERV_ID`], compared case-insensitively. A header naming
//! any other authserv-id is ignored entirely, wherever it sits, and a message
//! with no trusted header authenticates nothing: every result is unknown.
//!
//! The runtime hands over every copy joined with ", ", in message order, so the
//! joined string is first cut back into headers. A new header starts after a
//! ", " when what follows looks like `authserv-id;` (or `i=N; authserv-id;`
//! for ARC). A mistaken cut can only truncate a header, which loses results
//! rather than inventing them.
//!
//! Choice among trusted headers: the topmost `Authentication-Results` wins,
//! because a receiving MTA prepends its own and the topmost is the one closest
//! to us. Only when there is none is an `ARC-Authentication-Results` with
//! `i=1` and the same authserv-id used, since `i=1` is the hop Cloudflare
//! stamps on mail arriving with no earlier ARC set (`cloudflare.rs` appends
//! those after the plain headers).
//!
//! Confirmed on 2026-10-02 against real mail: Cloudflare stamps a plain
//! `Authentication-Results` with authserv-id `mx.cloudflare.net`, topmost in
//! the message the worker sees. The `ARC-Authentication-Results` fallback
//! stays a weaker source: if Cloudflare ever stamped only that header, a
//! sender could still write a plain one with this id and have it read first.
//! For Gmail senders Cloudflare's ARC set is `i=2`, so this fallback rarely
//! applies.
//!
//! Inside the chosen header this is a scan rather than a parse. A method name
//! has to start a token, which is more than a word boundary asks for.
//! `policy.dmarc=none` states the domain's published policy and `x-dkim=fail`
//! is a vendor's own method: both are ordinary RFC 8601 syntax. Only a word
//! character, a dot or a hyphen can put a name inside a larger token, so those
//! are what the byte in front of a match must not be. A method claimed with two
//! different values inside the header is treated as unknown.

use std::collections::BTreeSet;

/// Authserv-id Cloudflare's MX stamps on the results it records, confirmed
/// against live mail: see the module docs.
pub const CLOUDFLARE_AUTHSERV_ID: &str = "mx.cloudflare.net";

/// The three results the gateway is told, each `None` when unknown.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthResults {
    pub spf: Option<String>,
    pub dkim: Option<String>,
    pub dmarc: Option<String>,
}

pub fn auth_results(header: Option<&str>) -> AuthResults {
    let Some(body) = header.and_then(trusted_body) else {
        return AuthResults::default();
    };
    AuthResults {
        spf: read_method(body, "spf"),
        dkim: read_method(body, "dkim"),
        dmarc: read_method(body, "dmarc"),
    }
}

/// One header cut out of the joined string.
struct Segment<'a> {
    /// `Some(n)` for an ARC header carrying `i=n;`.
    arc_instance: Option<u32>,
    authserv_id: &'a str,
    /// Everything after the authserv-id's `;`.
    body: &'a str,
}

fn trusted_body(joined: &str) -> Option<&str> {
    let segments = split_headers(joined);
    let is_ours = |segment: &&Segment| {
        segment
            .authserv_id
            .eq_ignore_ascii_case(CLOUDFLARE_AUTHSERV_ID)
    };
    segments
        .iter()
        .filter(|segment| segment.arc_instance.is_none())
        .find(is_ours)
        .or_else(|| {
            segments
                .iter()
                .filter(|segment| segment.arc_instance == Some(1))
                .find(is_ours)
        })
        .map(|segment| segment.body)
}

fn split_headers(joined: &str) -> Vec<Segment<'_>> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut search_from = 0;
    while let Some(found) = joined[search_from..].find(", ") {
        let cut = search_from + found;
        let rest = cut + 2;
        if parse_segment(&joined[rest..]).is_some() {
            segments.extend(parse_segment(&joined[start..cut]));
            start = rest;
        }
        search_from = rest;
    }
    segments.extend(parse_segment(&joined[start..]));
    segments
}

/// Reads `[i=N;] authserv-id [version]; ...`, or `None` when the text does not
/// open like a header.
fn parse_segment(text: &str) -> Option<Segment<'_>> {
    let text = text.trim_start();
    let (arc_instance, after_instance) = match strip_arc_instance(text) {
        Some((instance, rest)) => (Some(instance), rest),
        None => (None, text),
    };
    let (head, body) = after_instance.split_once(';')?;
    let mut tokens = head.split_whitespace();
    let authserv_id = tokens.next()?;
    let version_ok = tokens
        .next()
        .is_none_or(|version| version.bytes().all(|byte| byte.is_ascii_digit()));
    let looks_like_an_id = authserv_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    if tokens.next().is_some() || !version_ok || !looks_like_an_id {
        return None;
    }
    Some(Segment {
        arc_instance,
        authserv_id,
        body,
    })
}

fn strip_arc_instance(text: &str) -> Option<(u32, &str)> {
    text.get(..2)
        .filter(|prefix| prefix.eq_ignore_ascii_case("i="))?;
    let (digits, after) = text[2..].split_once(';')?;
    let instance = digits.trim().parse::<u32>().ok()?;
    Some((instance, after))
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

    const FORGED_PASS: &str = "evil.example; dkim=pass; spf=pass; dmarc=pass";

    #[test]
    fn results_are_read_case_insensitively_and_reported_lowercase() {
        assert_eq!(
            auth_results(Some("mx.cloudflare.net; SPF=Pass; DKIM=FAIL; DMARC=None")),
            results(Some("pass"), Some("fail"), Some("none"))
        );
    }

    /// The regression the TypeScript suite was written for: `policy.dmarc` is a
    /// registered RFC 8601 property, so an MTA stating the domain's published
    /// policy beside its verdict is writing ordinary header syntax.
    #[test]
    fn a_policy_property_does_not_contradict_the_result_it_belongs_to() {
        let found = auth_results(Some(
            "mx.cloudflare.net; dmarc=pass policy.dmarc=none; spf=pass",
        ));
        assert_eq!(found.dmarc.as_deref(), Some("pass"));
    }

    #[test]
    fn a_header_with_comments_and_properties_reads_all_three_methods() {
        let header = "mx.cloudflare.net; dkim=pass header.i=@example.com header.b=\"AbC\"; \
             spf=pass (cloudflare.net: domain of sender@example.com designates 1.2.3.4) \
             smtp.mailfrom=sender@example.com; \
             dmarc=pass (p=NONE sp=QUARANTINE dis=NONE) policy.dmarc=none header.from=example.com";
        assert_eq!(
            auth_results(Some(header)),
            results(Some("pass"), Some("pass"), Some("pass"))
        );
    }

    #[test]
    fn a_vendors_own_x_method_does_not_contradict_the_standard_one() {
        let found = auth_results(Some(
            "mx.cloudflare.net; dkim=pass header.d=example.com; x-dkim=fail",
        ));
        assert_eq!(found.dkim.as_deref(), Some("pass"));
    }

    #[test]
    fn a_property_whose_name_ends_in_a_method_name_is_not_read_as_a_result() {
        assert_eq!(
            auth_results(Some(
                "mx.cloudflare.net; spf=pass smtp.mailfrom=a@b.com policy.spf=none"
            )),
            results(Some("pass"), None, None)
        );
    }

    #[test]
    fn a_method_claimed_two_ways_inside_the_trusted_header_is_unknown() {
        assert_eq!(
            auth_results(Some("mx.cloudflare.net; dmarc=pass; dmarc=fail; spf=pass")),
            results(Some("pass"), None, None)
        );
    }

    // Changed from `a_lone_claim_stands_when_no_other_copy_speaks_to_it`:
    // a claim nobody at Cloudflare made no longer stands.
    #[test]
    fn a_forged_claim_with_no_cloudflare_header_authenticates_nothing() {
        assert_eq!(auth_results(Some(FORGED_PASS)), AuthResults::default());
    }

    #[test]
    fn a_lone_claim_under_another_receivers_id_is_ignored() {
        assert_eq!(
            auth_results(Some("mx.google.com; dmarc=pass")),
            AuthResults::default()
        );
    }

    #[test]
    fn a_forged_pass_above_a_cloudflare_fail_does_not_override_it() {
        let joined = format!("{FORGED_PASS}, mx.cloudflare.net; spf=fail; dkim=fail; dmarc=fail");
        assert_eq!(
            auth_results(Some(&joined)),
            results(Some("fail"), Some("fail"), Some("fail"))
        );
    }

    #[test]
    fn a_forged_pass_below_a_cloudflare_fail_does_not_override_it() {
        let joined = format!("mx.cloudflare.net; spf=fail; dkim=fail; dmarc=fail, {FORGED_PASS}");
        assert_eq!(
            auth_results(Some(&joined)),
            results(Some("fail"), Some("fail"), Some("fail"))
        );
    }

    #[test]
    fn a_forged_header_beside_a_cloudflare_pass_changes_nothing() {
        let joined = format!("evil.example; dmarc=fail, {CLEAN}, other.example; spf=fail");
        assert_eq!(
            auth_results(Some(&joined)),
            results(Some("pass"), Some("pass"), Some("pass"))
        );
    }

    #[test]
    fn the_authserv_id_is_matched_case_insensitively() {
        for id in ["MX.CLOUDFLARE.NET", "Mx.Cloudflare.Net"] {
            let found = auth_results(Some(&format!("{id}; dmarc=pass")));
            assert_eq!(found.dmarc.as_deref(), Some("pass"), "{id}");
        }
    }

    #[test]
    fn an_authserv_id_version_is_allowed() {
        let found = auth_results(Some("mx.cloudflare.net 1; dmarc=pass"));
        assert_eq!(found.dmarc.as_deref(), Some("pass"));
    }

    #[test]
    fn an_id_that_merely_contains_cloudflares_is_not_cloudflares() {
        for id in [
            "mx.cloudflare.net.evil.example",
            "evil-mx.cloudflare.net",
            "xmx.cloudflare.net",
        ] {
            let found = auth_results(Some(&format!("{id}; dmarc=pass")));
            assert_eq!(found, AuthResults::default(), "{id}");
        }
    }

    #[test]
    fn with_several_cloudflare_headers_the_topmost_wins() {
        let joined = "mx.cloudflare.net; dmarc=fail, mx.cloudflare.net; dmarc=pass";
        assert_eq!(auth_results(Some(joined)).dmarc.as_deref(), Some("fail"));
    }

    #[test]
    fn a_trusted_header_does_not_borrow_results_from_an_untrusted_one() {
        let joined = "mx.cloudflare.net; spf=pass, evil.example; dmarc=pass; dkim=pass";
        assert_eq!(
            auth_results(Some(joined)),
            results(Some("pass"), None, None)
        );
    }

    #[test]
    fn arc_results_with_instance_one_and_cloudflares_id_are_used_when_no_plain_header_is() {
        let joined = "i=1; mx.cloudflare.net; dkim=pass header.d=example.com; dmarc=pass; spf=none";
        assert_eq!(
            auth_results(Some(joined)),
            results(Some("none"), Some("pass"), Some("pass"))
        );
    }

    #[test]
    fn a_plain_cloudflare_header_outranks_arc_results() {
        let joined = "mx.cloudflare.net; dmarc=fail, i=1; mx.cloudflare.net; dmarc=pass";
        assert_eq!(auth_results(Some(joined)).dmarc.as_deref(), Some("fail"));
    }

    #[test]
    fn arc_results_from_a_later_instance_or_another_id_are_ignored() {
        for joined in [
            "i=2; mx.cloudflare.net; dmarc=pass",
            "i=1; mx.google.com; dmarc=pass",
            "i=x; mx.cloudflare.net; dmarc=pass",
        ] {
            assert_eq!(
                auth_results(Some(joined)),
                AuthResults::default(),
                "{joined}"
            );
        }
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
            auth_results(Some("mx.cloudflare.net; dmarc= ; spf=")),
            AuthResults::default()
        );
    }

    #[test]
    fn an_empty_header_authenticates_nothing() {
        assert_eq!(auth_results(Some("")), AuthResults::default());
    }
}

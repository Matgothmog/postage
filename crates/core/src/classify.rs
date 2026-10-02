//! Mail classification, minus the model call: the facts a verdict is judged
//! on, the header-only fallback, the shaping applied to whatever the model
//! says, and the text sent to it. The network half lives in the server crate.

use std::collections::HashSet;
use std::fmt::Display;
use std::sync::LazyLock;

use postage_shared::Tier;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Everything the classifier is told about one inbound message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailFacts {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    /// Results the receiving MTA already computed: "pass", "fail", "none".
    pub spf: Option<String>,
    pub dkim: Option<String>,
    pub dmarc: Option<String>,
    pub urls: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub tier: Tier,
    pub confidence: f64,
    pub reasons: Vec<String>,
    /// True when the model was unreachable and this came from headers alone. A
    /// degraded verdict is never allowed to charge at the dangerous tier.
    pub degraded: bool,
}

/// What the model is asked to return, before `tidy` and the `degraded` flag.
/// Unknown keys are ignored, as the zod object it stands in for strips them.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelVerdict {
    pub tier: Tier,
    pub confidence: f64,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerdictParseError {
    #[error("Failed to parse structured output as JSON: {0}")]
    NotJson(String),
    #[error("Failed to parse structured output: {0}")]
    Invalid(String),
}

impl ModelVerdict {
    /// Reads the model's text block: JSON first, then the verdict's shape.
    pub fn parse(text: &str) -> Result<Self, VerdictParseError> {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|error| VerdictParseError::NotJson(error.to_string()))?;
        serde_json::from_value(value).map_err(|error| VerdictParseError::Invalid(error.to_string()))
    }

    /// The verdict this answer becomes: reasons shaped, not degraded.
    pub fn into_verdict(self) -> Verdict {
        Verdict {
            tier: self.tier,
            confidence: self.confidence,
            reasons: tidy(&self.reasons),
            degraded: false,
        }
    }
}

pub const SYSTEM_PROMPT: &str = r#"You classify inbound email for a gateway that charges senders.

Decide which one of four things a message is:

- human: written by a person to this specific recipient. Personal or
  professional correspondence, a reply, an introduction.
- important: automated but the recipient needs it now. One-time codes, password
  resets, receipts, shipping and booking confirmations, security alerts.
- commercial: automated and legitimate but not urgent. Newsletters, marketing,
  product announcements, digests, notifications the recipient opted into.
- dangerous: trying to deceive. Credential phishing, impersonation of a brand or
  person, payment redirection, malware links, extortion.

Weigh the authentication results heavily. A message claiming to be from a bank
with dmarc=fail is far more suspect than the same text with dmarc=pass. Judge
the links by where they actually point, not by their anchor text.

Err toward "important" over "commercial" when a person would be harmed by a
delay, and toward "commercial" over "dangerous" when a message is merely
unwanted. Blocking real mail costs the user more than letting a newsletter
through.

Give two or three reasons. Each is at most eight words, a fragment, no closing
period, concrete about what you actually saw — "sent from a bulk mail
platform", "link text and destination disagree", "DMARC failed for a bank
domain". No hedging, no jargon, and never restate the tier name."#;

const BODY_UTF16_CAP: usize = 12_000;
const LINK_CAP: usize = 20;

/// The user turn: headers, authentication results, links, then the body cut
/// to the first 12,000 UTF-16 code units (JavaScript's `slice` unit, so the
/// cut falls where the TypeScript gateway's did).
pub fn user_content(mail: &MailFacts) -> String {
    let unknown = |result: &Option<String>| result.as_deref().unwrap_or("unknown").to_owned();
    let links = if mail.urls.is_empty() {
        "Links: none".to_owned()
    } else {
        let shown: Vec<&str> = mail
            .urls
            .iter()
            .take(LINK_CAP)
            .map(String::as_str)
            .collect();
        format!("Links: {}", shown.join(", "))
    };
    [
        format!("From: {}", mail.from),
        format!("To: {}", mail.to),
        format!("Subject: {}", mail.subject),
        format!(
            "SPF: {}  DKIM: {}  DMARC: {}",
            unknown(&mail.spf),
            unknown(&mail.dkim),
            unknown(&mail.dmarc)
        ),
        links,
        String::new(),
        "Body:".to_owned(),
        truncate_utf16(&mail.body, BODY_UTF16_CAP),
    ]
    .join("\n")
}

/// The longest prefix of whole characters that fits in `cap` UTF-16 units. A
/// JavaScript slice can end between the halves of a surrogate pair; that half
/// character cannot exist in a Rust string, so it is dropped here.
fn truncate_utf16(text: &str, cap: usize) -> String {
    let mut units = 0;
    let mut kept = String::new();
    for character in text.chars() {
        units += character.len_utf16();
        if units > cap {
            break;
        }
        kept.push(character);
    }
    kept
}

/// JavaScript's `\s` and `trim()` set: the Unicode space separators, the line
/// terminators and the BOM. Rust's `is_whitespace` differs on exactly two
/// characters: it counts U+0085 and not U+FEFF.
fn is_js_whitespace(character: char) -> bool {
    character == '\u{feff}' || (character.is_whitespace() && character != '\u{85}')
}

/// A credential is a long run of key-charset characters with no spaces:
/// `sk-ant-...`, a bearer token, a signed URL's `key=` value. The
/// auth-resolution failure the header fallback exists for never carries one
/// (it names which setting is missing, not its value), but nothing guarantees
/// the next error won't quote a request that did, so anything shaped like one
/// is blanked out before the reason reaches a log.
const KEY_SHAPED_MIN_RUN: usize = 20;

pub fn sanitized_reason(cause: &impl Display) -> String {
    let message = cause.to_string();
    let is_key_character = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let mut out = String::with_capacity(message.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() >= KEY_SHAPED_MIN_RUN {
            out.push_str("[redacted]");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for character in message.chars() {
        if is_key_character(character) {
            run.push(character);
        } else {
            flush(&mut run, &mut out);
            out.push(character);
        }
    }
    flush(&mut run, &mut out);
    out
}

const REASON_WORD_CAP: usize = 8;

/// Shapes the model's own reasons before they leave this module: trim, drop
/// one trailing period, collapse every run of whitespace (newlines included)
/// and cap at eight words.
///
/// These are model output shaped by a stranger's email. The collapse is the
/// part that earns its place: a plain-text body is exactly its own bytes, so a
/// reason that kept its newlines could write its own lines into a message sent
/// under our name wherever it is ever printed.
pub fn tidy(reasons: &[String]) -> Vec<String> {
    reasons.iter().map(|reason| tidy_one(reason)).collect()
}

fn tidy_one(reason: &str) -> String {
    let trimmed = reason.trim_matches(is_js_whitespace);
    let without_period = trimmed.strip_suffix('.').unwrap_or(trimmed);
    without_period
        .split(is_js_whitespace)
        .filter(|word| !word.is_empty())
        .take(REASON_WORD_CAP)
        .collect::<Vec<_>>()
        .join(" ")
}

// ASCII-only case folding and word boundaries, as a JavaScript regex without
// the `u` flag has them.
static TRANSACTIONAL: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        r"(?i-u)\b(verification code|one[- ]time|otp|password reset|reset your|confirm your|receipt|invoice|order (confirmed|shipped)|security alert|sign[- ]in attempt)\b",
    )
});
static MARKETING: LazyLock<Regex> = LazyLock::new(|| {
    compile(r"(?i-u)\b(unsubscribe|newsletter|deal|% off|sale|webinar|new features?)\b")
});
static LURE: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        r"(?i-u)\b(verify your account|suspended|unusual activity|click here|confirm your identity|wire transfer|gift card)\b",
    )
});

/// The patterns are literals fixed at compile time, so a failure here is a typo
/// that the first test run catches.
#[allow(clippy::expect_used)]
fn compile(pattern: &str) -> Regex {
    Regex::new(pattern).expect("fixed pattern is valid")
}

/// Header-only fallback. Deliberately conservative: it can flag something as
/// suspicious but never as dangerous, because a wrong dangerous verdict blocks
/// mail and charges for it.
pub fn classify_from_headers(mail: &MailFacts) -> Verdict {
    let degraded = |tier: Tier, confidence: f64, reasons: Vec<&str>| Verdict {
        tier,
        confidence,
        reasons: reasons.into_iter().map(str::to_owned).collect(),
        degraded: true,
    };
    let mut reasons = Vec::new();
    let is = |value: &Option<String>, expected: &str| value.as_deref() == Some(expected);
    let auth_failed = is(&mail.dmarc, "fail") || (is(&mail.spf, "fail") && !is(&mail.dkim, "pass"));

    if auth_failed {
        reasons.push("Domain failed its own auth check");
    }

    if TRANSACTIONAL.is_match(&mail.subject) && !auth_failed {
        reasons.push("Looks like a code or a receipt");
        return degraded(Tier::Important, 0.5, reasons);
    }

    let subject_and_body = format!("{} {}", mail.subject, mail.body);

    if auth_failed && LURE.is_match(&subject_and_body) {
        reasons.push("Pressure language typical of phishing");
        // Still not "dangerous": a degraded verdict must not charge the top tier.
        return degraded(Tier::Commercial, 0.3, reasons);
    }

    reasons.push(if MARKETING.is_match(&subject_and_body) {
        "Reads as bulk marketing"
    } else {
        "Judged on headers only"
    });

    degraded(Tier::Commercial, 0.3, reasons)
}

/// Every `http(s)://` run up to whitespace, `<`, `>`, quotes or `)`, each
/// reported once, in the order it first appears.
pub fn extract_urls(body: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    let mut at = 0;
    while at < body.len() {
        let rest = &body[at..];
        let length = url_length_at(rest);
        if length > 0 {
            let url = &rest[..length];
            if seen.insert(url) {
                found.push(url.to_owned());
            }
            at += length;
        } else {
            at += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    found
}

/// Byte length of the URL starting exactly at `text`, or 0 when none does.
fn url_length_at(text: &str) -> usize {
    let scheme = ["https://", "http://"].into_iter().find(|scheme| {
        text.get(..scheme.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(scheme))
    });
    let Some(scheme) = scheme else { return 0 };
    let tail = text[scheme.len()..]
        .find(|c: char| is_js_whitespace(c) || matches!(c, '<' | '>' | '"' | '\'' | ')'))
        .unwrap_or(text.len() - scheme.len());
    if tail == 0 { 0 } else { scheme.len() + tail }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(overrides: impl FnOnce(&mut MailFacts)) -> MailFacts {
        let mut facts = MailFacts {
            from: "someone@example.com".into(),
            to: "demo@usepostage.com".into(),
            subject: String::new(),
            body: String::new(),
            spf: Some("pass".into()),
            dkim: Some("pass".into()),
            dmarc: Some("pass".into()),
            urls: vec![],
        };
        overrides(&mut facts);
        facts
    }

    fn fail(value: &str) -> Option<String> {
        Some(value.to_owned())
    }

    /// The rule the whole degraded path exists to keep: a model outage must
    /// never let mail escalate to the tier that blocks delivery and charges for
    /// it.
    #[test]
    fn a_degraded_verdict_is_never_the_dangerous_tier_even_for_classic_phishing_language() {
        let verdict = classify_from_headers(&mail(|m| {
            m.spf = fail("fail");
            m.dkim = fail("fail");
            m.dmarc = fail("fail");
            m.subject = "Your account is suspended".into();
            m.body = "Click here to verify your account and confirm your identity or it stays suspended.".into();
        }));

        assert_ne!(verdict.tier, Tier::Dangerous);
        assert!(verdict.degraded);
    }

    #[test]
    fn failed_authentication_alone_with_no_lure_language_still_never_reaches_dangerous() {
        let verdict = classify_from_headers(&mail(|m| {
            m.spf = fail("fail");
            m.dkim = fail("fail");
            m.dmarc = fail("fail");
        }));
        assert_ne!(verdict.tier, Tier::Dangerous);
    }

    #[test]
    fn a_wire_transfer_lure_over_a_failed_auth_domain_is_degraded_to_commercial_not_dangerous() {
        let verdict = classify_from_headers(&mail(|m| {
            m.dmarc = fail("fail");
            m.subject = "Urgent wire transfer needed".into();
            m.body = "Please process this gift card payment immediately.".into();
        }));

        assert_eq!(verdict.tier, Tier::Commercial);
        assert!(verdict.degraded);
    }

    #[test]
    fn every_degraded_verdict_whatever_its_tier_says_so() {
        let cases = [
            mail(|_| {}),
            mail(|m| m.subject = "one-time verification code".into()),
            mail(|m| m.body = "unsubscribe from this newsletter".into()),
            mail(|m| {
                m.dmarc = fail("fail");
                m.body = "click here to verify your account".into();
            }),
        ];
        for facts in &cases {
            assert!(classify_from_headers(facts).degraded);
        }
    }

    #[test]
    fn a_transactional_subject_with_clean_authentication_reads_as_important() {
        let verdict = classify_from_headers(&mail(|m| {
            m.subject = "Your one-time verification code".into();
        }));
        assert_eq!(verdict.tier, Tier::Important);
    }

    /// Authentication failure overrides the transactional read: a
    /// password-reset subject is exactly what a spoofed sender would pick.
    #[test]
    fn a_transactional_subject_over_failed_authentication_does_not_get_importance() {
        let verdict = classify_from_headers(&mail(|m| {
            m.dmarc = fail("fail");
            m.subject = "Your one-time verification code".into();
        }));
        assert_ne!(verdict.tier, Tier::Important);
    }

    #[test]
    fn bulk_marketing_language_reads_as_commercial() {
        let verdict = classify_from_headers(&mail(|m| {
            m.body = "50% off this week, unsubscribe anytime".into();
        }));
        assert_eq!(verdict.tier, Tier::Commercial);
    }

    #[test]
    fn mail_matching_none_of_the_header_patterns_still_classifies_as_commercial() {
        let verdict = classify_from_headers(&mail(|m| {
            m.subject = "hi".into();
            m.body = "just saying hello".into();
        }));
        assert_eq!(verdict.tier, Tier::Commercial);
        assert!(
            !verdict.reasons.is_empty(),
            "a verdict with no reason given explains nothing to the user"
        );
    }

    #[test]
    fn no_links_in_the_body_yields_no_urls() {
        assert_eq!(extract_urls("nothing to see here"), Vec::<String>::new());
    }

    #[test]
    fn an_empty_body_yields_no_urls() {
        assert_eq!(extract_urls(""), Vec::<String>::new());
    }

    #[test]
    fn a_link_is_extracted_without_markdown_parentheses_or_surrounding_quotes() {
        let found =
            extract_urls(r#"See (http://example.com/foo) and "http://example.com/bar" now"#);
        assert_eq!(found, ["http://example.com/foo", "http://example.com/bar"]);
    }

    #[test]
    fn the_same_link_repeated_in_the_body_is_reported_once() {
        let found =
            extract_urls("http://dup.example.com then again http://dup.example.com later too");
        assert_eq!(found, ["http://dup.example.com"]);
    }

    #[test]
    fn distinct_links_are_all_kept_in_the_order_they_first_appear() {
        let found = extract_urls(
            "first http://a.example.com then http://b.example.com then http://a.example.com",
        );
        assert_eq!(found, ["http://a.example.com", "http://b.example.com"]);
    }

    #[test]
    fn a_scheme_with_nothing_after_it_is_not_a_link_and_the_scan_goes_on() {
        let found = extract_urls("http:// then HTTPS://Caps.example/Path é http://ok.example");
        assert_eq!(found, ["HTTPS://Caps.example/Path", "http://ok.example"]);
    }

    #[test]
    fn sanitized_reason_reads_an_errors_own_message() {
        let error = std::io::Error::other("model unavailable");
        assert_eq!(sanitized_reason(&error), "model unavailable");
    }

    #[test]
    fn sanitized_reason_stringifies_a_thrown_value_that_is_not_an_error() {
        assert_eq!(sanitized_reason(&"timeout"), "timeout");
    }

    #[test]
    fn sanitized_reason_leaves_the_real_auth_resolution_failure_readable() {
        // Names the missing setting, never a value, so nothing in it should be
        // blanked out.
        let message = "ANTHROPIC_API_KEY is not set";
        assert_eq!(sanitized_reason(&message), message);
    }

    #[test]
    fn sanitized_reason_blanks_out_anything_shaped_like_a_credential() {
        let secret = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789";
        let reason = sanitized_reason(&format!("401 invalid x-api-key {secret}"));

        assert!(!reason.contains(secret));
        assert!(reason.contains("[redacted]"));
    }

    #[test]
    fn sanitized_reason_keeps_runs_one_short_of_the_threshold() {
        let nineteen = "a".repeat(19);
        let twenty = "a".repeat(20);
        assert_eq!(sanitized_reason(&nineteen), nineteen);
        assert_eq!(sanitized_reason(&twenty), "[redacted]");
    }

    fn tidied(reason: &str) -> String {
        tidy(&[reason.to_owned()]).remove(0)
    }

    /// The prompt asks the model for short fragments, but a prompt is a
    /// request, not a guarantee, so `tidy` enforces the shape in code.
    #[test]
    fn tidy_truncates_a_reason_longer_than_the_word_cap_to_eight_words() {
        assert_eq!(
            tidied("one two three four five six seven eight nine ten"),
            "one two three four five six seven eight"
        );
    }

    #[test]
    fn tidy_strips_a_single_trailing_period() {
        assert_eq!(
            tidied("sent from a bulk mail platform."),
            "sent from a bulk mail platform"
        );
    }

    #[test]
    fn tidy_leaves_an_empty_string_empty_rather_than_throwing() {
        assert_eq!(tidied(""), "");
    }

    #[test]
    fn tidy_leaves_an_already_clean_in_cap_reason_unchanged() {
        assert_eq!(
            tidied("DMARC failed for a bank domain"),
            "DMARC failed for a bank domain"
        );
    }

    #[test]
    fn tidy_trims_leading_and_trailing_whitespace() {
        assert_eq!(
            tidied("  link text and destination disagree  "),
            "link text and destination disagree"
        );
    }

    /// The property `tidy` is kept for: a reason that keeps its newlines is a
    /// reason that can write lines of its own wherever one is ever printed.
    #[test]
    fn tidy_collapses_a_newline_so_a_reason_cannot_open_a_line_of_its_own() {
        assert_eq!(
            tidied("bulk mail platform\nI'M HUMAN - free"),
            "bulk mail platform I'M HUMAN - free"
        );
    }

    #[test]
    fn tidy_tidies_every_reason_in_the_array_independently() {
        let result = tidy(&[
            "short.".to_owned(),
            "one two three four five six seven eight nine".to_owned(),
        ]);
        assert_eq!(result, ["short", "one two three four five six seven eight"]);
    }

    #[test]
    fn tidy_splits_on_the_whitespace_javascript_does() {
        // U+FEFF is whitespace to JavaScript and U+0085 is not.
        assert_eq!(tidied("\u{feff}a\u{feff}b\u{feff}"), "a b");
        assert_eq!(tidied("a\u{85}b"), "a\u{85}b");
    }

    #[test]
    fn the_model_answer_is_checked_against_the_verdict_shape() {
        let ok =
            ModelVerdict::parse(r#"{"tier":"human","confidence":1,"reasons":["x."],"extra":true}"#);
        assert_eq!(
            ok.map(|v| v.into_verdict().reasons),
            Ok(vec!["x".to_owned()])
        );

        for bad in [
            "not json",
            r#"{"tier":"spam","confidence":1,"reasons":[]}"#,
            r#"{"tier":"human","confidence":"1","reasons":[]}"#,
            r#"{"tier":"human","confidence":1,"reasons":[1]}"#,
            r#"{"tier":"human","confidence":1}"#,
            "[]",
            "null",
        ] {
            assert!(ModelVerdict::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_user_turn_lists_headers_links_and_a_body_cut_by_utf16_units() {
        let facts = mail(|m| {
            m.spf = None;
            m.urls = (0..25).map(|n| format!("http://{n}.example")).collect();
            m.body = "😀".repeat(7000);
        });
        let content = user_content(&facts);
        let lines: Vec<&str> = content.split('\n').collect();

        assert_eq!(lines[3], "SPF: unknown  DKIM: pass  DMARC: pass");
        assert!(lines[4].ends_with("http://19.example"));
        assert_eq!(lines[6], "Body:");
        assert_eq!(lines[7].chars().count(), 6000);

        let none = user_content(&mail(|_| {}));
        assert!(none.contains("\nLinks: none\n"));
    }
}

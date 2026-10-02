//! The one notice a held sender receives, as subject, HTML and plain text.
//!
//! Pure string templating: the caller supplies `now` because core has no clock.

use postage_shared::GatewayNotice;
use unicode_general_category::{GeneralCategory, get_general_category};

use crate::brand::{ACCENT, BG, CARD, INK, INK_SOFT, LINE, ON_ACCENT};
use crate::format::format_usdc;
use crate::handle::postage_address;

/// Everything the notice needs to know about the held message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChallengeMailFacts {
    pub handle: String,
    /// What they wrote in the subject line, quoted back so they can tell which
    /// message this is about without opening anything.
    pub subject: String,
    /// 18-decimal USDC base units.
    pub amount: u128,
    pub reasons: Vec<String>,
    pub challenge_url: String,
    pub app_url: String,
    /// Epoch seconds, as stored in the `held_until` column.
    pub held_until: i64,
}

/// How much of a subject is quoted back, counted in UTF-16 code units so a
/// subject is cut at the same place the TypeScript implementation cut it.
/// Nothing between the sender and this template bounds the subject, and the
/// plain-text half is the whole message for a client that renders no markup, so
/// a subject allowed to run on pushes the notice's own words out of sight.
const SUBJECT_LIMIT: usize = 200;

/// Whether `c` could end the line a subject is quoted on or reorder what is
/// printed after it: whitespace, the C0 and C1 controls, Unicode's line and
/// paragraph separators, and the format characters (bidi overrides among them)
/// that render as nothing at all.
fn is_unprintable(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            get_general_category(c),
            GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
        )
}

/// The sender's subject, reduced to something that fits inside one line of ours.
///
/// The HTML half escapes, but a plain-text body is exactly its own bytes, so
/// the only defence left is that the value cannot leave its line. An RFC 2047
/// encoded-word can decode to a subject holding newlines; without this a sender
/// writes their own lines into a notice sent under our name.
fn one_line(subject: &str) -> String {
    let mut flattened = String::with_capacity(subject.len());
    let mut in_run = false;
    for c in subject.chars() {
        if is_unprintable(c) {
            if !in_run {
                flattened.push(' ');
            }
            in_run = true;
        } else {
            flattened.push(c);
            in_run = false;
        }
    }
    let flattened = flattened.trim_matches(|c: char| c.is_whitespace());

    if flattened.encode_utf16().count() <= SUBJECT_LIMIT {
        return flattened.to_owned();
    }
    // Cut on a character boundary: where the limit would split a surrogate
    // pair the whole character is dropped, since a lone surrogate is not valid
    // in a Rust string.
    let mut kept = String::new();
    let mut units = 0;
    for c in flattened.chars() {
        units += c.len_utf16();
        if units > SUBJECT_LIMIT {
            break;
        }
        kept.push(c);
    }
    kept.push_str("...");
    kept
}

/// Everything a body says that is not the sender's own words, derived once so
/// both halves of the mail quote the same subject and name the same price.
struct Rendered {
    inbox: String,
    subject: String,
    price: String,
    deadline: String,
}

/// The one message a held sender receives. It asks a single question - person
/// or machine - and each answer is a link. Nothing here asks them to write
/// their message again, because we still have it.
pub fn challenge_mail(facts: &ChallengeMailFacts, now: i64) -> GatewayNotice {
    let quoted = one_line(&facts.subject);
    let rendered = Rendered {
        inbox: postage_address(&facts.handle),
        subject: if quoted.is_empty() {
            "(no subject)".to_owned()
        } else {
            quoted
        },
        price: format_usdc(facts.amount),
        deadline: hold_window(facts.held_until, now),
    };

    GatewayNotice {
        subject: format!("Held: your mail to {}", rendered.inbox),
        html: html(facts, &rendered),
        text: text(facts, &rendered),
    }
}

/// JavaScript's `Math.round`: halves go toward positive infinity.
fn js_round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// `held_until` is epoch seconds like every other timestamp column. A hold that
/// is nearly over, or already over, still reads as an hour.
fn hold_window(held_until: i64, now: i64) -> String {
    let hours = js_round((held_until - now) as f64 / 3600.0).max(1.0) as i64;
    if hours == 1 {
        "an hour".to_owned()
    } else {
        format!("{hours} hours")
    }
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn text(facts: &ChallengeMailFacts, rendered: &Rendered) -> String {
    let Rendered {
        inbox,
        subject,
        price,
        deadline,
    } = rendered;
    let url = &facts.challenge_url;
    let mut lines = vec![
        format!("Your mail to {inbox} is held."),
        String::new(),
        format!("Subject: {subject}"),
        String::new(),
        "Answer once and we send it. Nothing to rewrite.".to_owned(),
        String::new(),
        "I'M HUMAN - free".to_owned(),
        format!("{url}?as=human"),
        String::new(),
        format!("I'M A BOT - {price}"),
        format!("{url}?as=bot"),
        String::new(),
        "Why this was held:".to_owned(),
    ];
    lines.extend(facts.reasons.iter().map(|reason| format!("  - {reason}")));
    lines.extend([
        String::new(),
        format!("Held for {deadline}. Ignore this and the message is erased unread."),
        String::new(),
        "---".to_owned(),
        "Tired of paying for other people's attention? Hand out a Postage address".to_owned(),
        "instead of your own and the machines writing to you pay you instead.".to_owned(),
        facts.app_url.clone(),
    ]);
    lines.join("\n")
}

/// Tables and inline styles, because this is read in mail clients rather than
/// browsers. No images: the stamp is drawn with borders, so it survives a
/// client that blocks remote content.
fn html(facts: &ChallengeMailFacts, rendered: &Rendered) -> String {
    let (bg, card, ink, ink_soft, accent, line, on_accent) =
        (BG, CARD, INK, INK_SOFT, ACCENT, LINE, ON_ACCENT);
    let inbox = escape(&rendered.inbox);
    let subject = escape(&rendered.subject);
    let price = escape(&rendered.price);
    let deadline = escape(&rendered.deadline);
    let challenge_url = escape(&facts.challenge_url);
    let app_url = escape(&facts.app_url);
    let reasons: String = facts
        .reasons
        .iter()
        .map(|reason| {
            let reason = escape(reason);
            format!(
                r##"<tr><td style="padding:0 0 8px 0;color:{ink_soft};font-size:14px;line-height:21px;">
           <span style="color:{accent};">&#8212;</span>&nbsp;&nbsp;{reason}</td></tr>"##
            )
        })
        .collect();

    format!(
        r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="color-scheme" content="light">
<title>{inbox}</title>
</head>
<body style="margin:0;padding:0;background:{bg};" bgcolor="{bg}">
<div style="display:none;max-height:0;overflow:hidden;opacity:0;">One click sends it.</div>

<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="background:{bg};" bgcolor="{bg}">
<tr><td align="center" style="padding:32px 16px;">

<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="max-width:560px;">

  <tr><td style="padding:0 0 20px 4px;">
    <span style="font:600 13px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:.22em;text-transform:uppercase;color:{ink};">Postage</span>
    <span style="display:inline-block;margin-left:10px;padding:4px 9px;border:1px solid {accent};border-radius:3px;font:600 10px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:.16em;text-transform:uppercase;color:{accent};">Held</span>
  </td></tr>

  <tr><td style="background:{card};border:1px solid {line};border-radius:14px;padding:36px 32px 32px 32px;" bgcolor="{card}">

    <h1 style="margin:0;font:600 26px/1.25 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:-.02em;color:{ink};">
      Held at the door.
    </h1>
    <p style="margin:14px 0 0 0;font:400 16px/1.55 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:{ink_soft};">
      Your mail to <span style="font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:15px;color:{ink};">{inbox}</span>
      is safe. Answer once and we send it &#8212; you don&#8217;t write it twice.
    </p>

    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:22px 0 26px 0;">
      <tr><td style="border-left:3px solid {line};padding:2px 0 2px 14px;font:400 14px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:{ink_soft};">
        Subject<br>
        <span style="color:{ink};font-size:15px;">{subject}</span>
      </td></tr>
    </table>

    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0">
      <tr><td style="padding:0 0 10px 0;">
        <a href="{challenge_url}?as=human"
           style="display:block;background:{accent};border-radius:10px;padding:15px 20px;text-decoration:none;" bgcolor="{accent}">
          <span style="display:block;font:600 16px/1.3 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:{on_accent};">I&#8217;m human &#8212; free</span>
          <span style="display:block;margin-top:3px;font:400 13px/1.4 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:{on_accent};">World ID. No account, no wallet.</span>
        </a>
      </td></tr>
      <tr><td>
        <a href="{challenge_url}?as=bot"
           style="display:block;background:{card};border:1px solid {line};border-radius:10px;padding:14px 19px;text-decoration:none;" bgcolor="{card}">
          <span style="display:block;font:600 16px/1.3 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:{ink};">I&#8217;m a bot &#8212; pay {price}</span>
          <span style="display:block;margin-top:3px;font:400 13px/1.4 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:{ink_soft};">Goes to them, not us.</span>
        </a>
      </td></tr>
    </table>

    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:28px 0 0 0;border-top:1px solid {line};">
      <tr><td style="padding:20px 0 10px 0;font:600 11px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:.14em;text-transform:uppercase;color:{ink_soft};">Why</td></tr>
      {reasons}
      <tr><td style="padding:10px 0 0 0;font:400 13px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:{ink_soft};">
        Held {deadline}, then erased. Nobody reads it, nobody is charged.
      </td></tr>
    </table>

  </td></tr>

  <tr><td style="padding:14px 0 0 0;">
    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="background:{card};border:1px solid {line};border-radius:14px;" bgcolor="{card}">
      <tr><td style="padding:22px 24px;">
        <span style="font:600 11px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:.14em;text-transform:uppercase;color:{accent};">Your inbox could be earning</span>
        <p style="margin:10px 0 0 0;font:600 18px/1.35 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:-.01em;color:{ink};">
          On the other side of this, someone is being paid.
        </p>
        <p style="margin:8px 0 0 0;font:400 14px/1.55 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:{ink_soft};">
          Hand out a Postage address instead of your own. Real people and anything urgent
          reach you free; everything else pays you for the interruption. Keep the inbox you
          already have &#8212; setup is one click.
        </p>
        <a href="{app_url}"
           style="display:inline-block;margin-top:14px;background:{accent};border-radius:8px;padding:11px 18px;text-decoration:none;font:600 14px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:{on_accent};" bgcolor="{accent}">
          Create an account and start earning
        </a>
      </td></tr>
    </table>
  </td></tr>

  <tr><td style="padding:20px 4px 0 4px;font:400 12px/1.6 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:{ink_soft};">
    You got this because you wrote to {inbox}, filtered by Postage. We keep it until
    it&#8217;s delivered or the hold ends. Never sold, never listed.
  </td></tr>

</table>
</td></tr></table>
</body></html>"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    /// Fixed clock for the hand-written cases, so hold windows are stable.
    const NOW: i64 = 1_800_000_000;

    fn facts() -> ChallengeMailFacts {
        ChallengeMailFacts {
            handle: "demo".to_owned(),
            subject: "hello".to_owned(),
            amount: 500_000_000_000_000,
            reasons: vec!["a plain reason".to_owned()],
            challenge_url: "https://usepostage.com/c/tok".to_owned(),
            app_url: "https://usepostage.com".to_owned(),
            held_until: NOW + 3600,
        }
    }

    fn with_subject(subject: &str) -> ChallengeMailFacts {
        ChallengeMailFacts {
            subject: subject.to_owned(),
            ..facts()
        }
    }

    #[test]
    fn every_html_special_character_in_the_subject_is_escaped() {
        let notice = challenge_mail(&with_subject(r#"<b>&"'</b>"#), NOW);

        assert!(notice.html.contains("&lt;b&gt;&amp;&quot;&#39;&lt;/b&gt;"));
        assert!(!notice.html.contains(r#"<b>&"'</b>"#));
    }

    #[test]
    fn a_reason_containing_a_tag_is_escaped_not_rendered() {
        let mut held = facts();
        held.reasons = vec![
            "<img src=x onerror=alert(1)>".to_owned(),
            "safe reason".to_owned(),
        ];
        let notice = challenge_mail(&held, NOW);

        assert!(notice.html.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(!notice.html.contains("<img src=x onerror=alert(1)>"));
    }

    #[test]
    fn an_ampersand_in_the_subject_does_not_merge_with_the_next_entity() {
        let notice = challenge_mail(&with_subject("Q&A session"), NOW);
        assert!(notice.html.contains("Q&amp;A session"));
    }

    #[test]
    fn a_double_quote_in_the_subject_cannot_break_out_of_an_attribute() {
        let notice = challenge_mail(&with_subject(r#""><script>alert(1)</script>"#), NOW);

        assert!(!notice.html.contains(r#""><script>alert(1)</script>"#));
        assert!(
            notice
                .html
                .contains("&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;")
        );
    }

    #[test]
    fn a_subject_with_no_special_characters_passes_through_unchanged() {
        let notice = challenge_mail(&with_subject("just a normal subject line"), NOW);
        assert!(notice.html.contains("just a normal subject line"));
    }

    #[test]
    fn the_plain_text_body_carries_special_characters_unescaped() {
        let notice = challenge_mail(&with_subject(r#"<b>&"'</b>"#), NOW);

        assert!(notice.text.lines().any(|l| l == r#"Subject: <b>&"'</b>"#));
    }

    #[test]
    fn a_newline_in_the_subject_cannot_open_a_line_of_its_own() {
        let injected = "ordinary\r\n\r\nA PERSON WROTE IT - free\r\nhttps://evil.example/?as=human";
        let text = challenge_mail(&with_subject(injected), NOW).text;
        let harmless = challenge_mail(&with_subject("ordinary"), NOW).text;

        assert_eq!(text.lines().count(), harmless.lines().count());
        assert!(!text.contains("\nA PERSON WROTE IT - free\nhttps://evil.example/?as=human"));
        assert!(
            text.contains(
                "Subject: ordinary A PERSON WROTE IT - free https://evil.example/?as=human"
            )
        );
    }

    #[test]
    fn separators_and_bidi_overrides_read_as_the_space_they_render_as() {
        let text = challenge_mail(
            &with_subject("one\u{2028}two\u{2029}three\u{202e}flipped"),
            NOW,
        )
        .text;
        let harmless = challenge_mail(&with_subject("one two three flipped"), NOW).text;

        assert_eq!(text, harmless);
    }

    #[test]
    fn a_subject_longer_than_the_bound_is_cut_short_in_both_bodies() {
        let notice = challenge_mail(&with_subject(&"x".repeat(5_000)), NOW);

        assert!(
            notice
                .text
                .contains(&format!("Subject: {}...", "x".repeat(200)))
        );
        assert!(!notice.text.contains(&"x".repeat(201)));
        assert!(!notice.html.contains(&"x".repeat(201)));
    }

    #[test]
    fn a_subject_of_nothing_but_line_breaks_reads_as_no_subject() {
        let notice = challenge_mail(&with_subject("\r\n\t \u{2028}"), NOW);

        assert!(notice.text.contains("Subject: (no subject)"));
        assert!(notice.html.contains("(no subject)"));
    }

    #[test]
    fn the_limit_counts_utf16_units_and_never_splits_a_character() {
        // Each envelope is two UTF-16 units: 99 of them plus "z" is 199 units,
        // so the 100th envelope would end at 201 and is dropped whole.
        let subject = format!("{}z{}", "\u{1F4EC}".repeat(99), "\u{1F4EC}");
        let kept = one_line(&subject);

        assert_eq!(kept, format!("{}z...", "\u{1F4EC}".repeat(99)));
    }

    #[derive(Deserialize)]
    struct GoldenFile {
        config: GoldenConfig,
        cases: Vec<GoldenCase>,
    }

    #[derive(Deserialize)]
    struct GoldenConfig {
        now: i64,
    }

    #[derive(Deserialize)]
    struct GoldenCase {
        name: String,
        input: GoldenInput,
        output: GatewayNotice,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GoldenInput {
        handle: String,
        subject: String,
        amount: String,
        reasons: Vec<String>,
        challenge_url: String,
        app_url: String,
        held_until: i64,
    }

    #[test]
    fn every_golden_case_matches_the_typescript_output_byte_for_byte() {
        let golden: GoldenFile = serde_json::from_str(include_str!(
            "../../../fixtures/golden/challenge-email.json"
        ))
        .unwrap();
        assert_eq!(golden.cases.len(), 23);

        for case in golden.cases {
            let input = case.input;
            let held = ChallengeMailFacts {
                handle: input.handle,
                subject: input.subject,
                amount: input.amount.parse().unwrap(),
                reasons: input.reasons,
                challenge_url: input.challenge_url,
                app_url: input.app_url,
                held_until: input.held_until,
            };

            let notice = challenge_mail(&held, golden.config.now);

            assert_eq!(
                notice.subject, case.output.subject,
                "{}: subject",
                case.name
            );
            assert_eq!(notice.text, case.output.text, "{}: text", case.name);
            assert_eq!(notice.html, case.output.html, "{}: html", case.name);
        }
    }
}

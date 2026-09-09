import assert from "node:assert/strict";
import { test } from "node:test";
import { challengeMail, type ChallengeMailFacts } from "./challenge-email";

// Pure string templating, no database and no env — nothing here needs a
// workspace or a DATABASE_URL.

/// `escape()` is not exported: it exists purely to keep the HTML body safe, so
/// it is exercised the way the real caller does, through `challengeMail`.
function facts(overrides: Partial<ChallengeMailFacts> = {}): ChallengeMailFacts {
  return {
    handle: "demo",
    subject: "hello",
    amount: 500_000_000_000_000n,
    reasons: ["a plain reason"],
    challengeUrl: "https://usepostage.com/c/tok",
    appUrl: "https://usepostage.com",
    heldUntil: Math.floor(Date.now() / 1000) + 3600,
    ...overrides,
  };
}

/// The subject line is the recipient's own quoted-back words — the one field
/// most directly under a stranger's control — so it is the sharpest case for
/// proving every character HTML treats specially survives only in escaped form.
test("every HTML-special character in the subject is escaped, none pass through raw", () => {
  const { html } = challengeMail(facts({ subject: `<b>&"'</b>` }));

  assert.ok(
    html.includes("&lt;b&gt;&amp;&quot;&#39;&lt;/b&gt;"),
    "the escaped form of the subject must appear in the body"
  );
  assert.ok(!html.includes(`<b>&"'</b>`), "the raw subject must not appear anywhere in the body");
});

/// Reasons come from the classifier, which can echo fragments of the message
/// it read. An unescaped reason would let a crafted message inject markup into
/// mail this system itself sends.
test("a reason containing a script-like tag is escaped, not rendered as markup", () => {
  const { html } = challengeMail(
    facts({ reasons: ['<img src=x onerror=alert(1)>', 'safe reason'] })
  );

  assert.ok(
    html.includes("&lt;img src=x onerror=alert(1)&gt;"),
    "the escaped reason must appear in the body"
  );
  assert.ok(
    !html.includes("<img src=x onerror=alert(1)>"),
    "an unescaped reason would be a live tag in a rendered email"
  );
});

test("an ampersand in the subject does not merge with the entity that follows it", () => {
  const { html } = challengeMail(facts({ subject: "Q&A session" }));
  assert.ok(html.includes("Q&amp;A session"));
});

test("a double quote in the subject cannot break out of an HTML attribute", () => {
  const { html } = challengeMail(facts({ subject: `"><script>alert(1)</script>` }));
  assert.ok(!html.includes(`"><script>alert(1)</script>`));
  assert.ok(html.includes("&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;"));
});

test("a subject with no special characters passes through unchanged", () => {
  const { html } = challengeMail(facts({ subject: "just a normal subject line" }));
  assert.ok(html.includes("just a normal subject line"));
});

/// The plain-text half renders no markup, so escaping there would put entity
/// text in front of a reader instead of the words the sender wrote. That much
/// of "verbatim" still holds and is what this pins. What does not hold is the
/// rest of it: with no escaping to fall back on, the only defence a plain-text
/// body has is that a value cannot leave the line it was given — which is the
/// test below, and why this one now asserts the line rather than a substring.
test("the plain-text body carries HTML-special characters in the subject unescaped", () => {
  const { text } = challengeMail(facts({ subject: `<b>&"'</b>` }));

  assert.ok(
    text.split("\n").includes(`Subject: <b>&"'</b>`),
    "the characters the sender wrote, on the one line that quotes them back"
  );
});

/// A subject is not a single line by the time it reaches here. PostalMime
/// decodes RFC 2047 encoded-words, and an encoded-word decodes to whatever its
/// bytes say — `=?utf-8?Q?a=0Ab?=` really does parse to a subject with a
/// newline in it, where an ordinary folded header would have been unfolded to
/// a space. Left alone, a sender writes their own lines into a notice the
/// reader's MTA sees as coming from us: one more "A PERSON WROTE IT - free",
/// pointing wherever they like, indistinguishable from the real one.
test("a newline in the subject cannot open a line of its own in the plain-text body", () => {
  const injected = "ordinary\r\n\r\nA PERSON WROTE IT - free\r\nhttps://evil.example/?as=human";
  const { text } = challengeMail(facts({ subject: injected }));
  const { text: harmless } = challengeMail(facts({ subject: "ordinary" }));

  assert.equal(
    text.split("\n").length,
    harmless.split("\n").length,
    "an injected subject must not add a single line to the body"
  );
  assert.ok(
    !text.includes("\nA PERSON WROTE IT - free\nhttps://evil.example/?as=human"),
    "the forged pair of lines must not exist anywhere in the body"
  );
  assert.ok(
    text.includes("Subject: ordinary A PERSON WROTE IT - free https://evil.example/?as=human"),
    "and the subject is still quoted back whole, flattened onto the line it belongs on"
  );
});

/// Unicode has line breaks of its own, and a mail client that honours U+2028
/// would render the same forged line from a subject holding no `\n` at all.
/// The bidirectional overrides are here for the same reason: left in, they let
/// a subject reorder the text printed after it.
test("a subject cannot break its line with a separator or override the ones that follow", () => {
  const { text } = challengeMail(facts({ subject: "one\u2028two\u2029three\u202eflipped" }));
  const { text: harmless } = challengeMail(facts({ subject: "one two three flipped" }));

  assert.equal(text, harmless, "every one of them reads as the space it is rendered as");
});

/// Nothing between the sender and this template bounds the subject, and a
/// plain-text body is the whole message for a client that renders no markup.
/// A subject allowed to run on is a notice whose own words are pushed off the
/// end of what anybody scrolls to.
test("a subject longer than the bound is cut short in both bodies", () => {
  const runOn = "x".repeat(5_000);
  const { text, html } = challengeMail(facts({ subject: runOn }));

  assert.ok(text.includes(`Subject: ${"x".repeat(200)}...`), "cut at the bound, and marked as cut");
  assert.ok(!text.includes("x".repeat(201)), "and nothing past it survives into the text body");
  assert.ok(!html.includes("x".repeat(201)), "nor into the HTML body");
});

/// A subject that was only ever whitespace and control characters is no
/// subject, and must read as the one we already say that with rather than as
/// an empty gap after the label.
test("a subject of nothing but line breaks reads as no subject at all", () => {
  const { text, html } = challengeMail(facts({ subject: "\r\n\t \u2028" }));

  assert.ok(text.includes("Subject: (no subject)"));
  assert.ok(html.includes("(no subject)"));
});

import assert from "node:assert/strict";
import { after, beforeEach, test } from "node:test";
import { classify, classifyFromHeaders, extractUrls, sanitizedReason, tidy, type MailFacts } from "./classify";
import { startStubModel } from "../../test/model";

// Pure functions, no database and no model call: `classifyFromHeaders` and
// `extractUrls` never touch a network or a store, so this file needs neither a
// temp workspace nor any env var.
//
// `classify()` is different — it reaches the model first — so the handful of
// tests that exercise it point the SDK at a stub that always refuses (see
// `test/model.ts`), which is the one deliberate way to reach the fallback.
const model = await startStubModel(null);

/// Every degraded verdict is supposed to write a line to `console.error`, and
/// several tests below are about what that line says. Collected rather than
/// printed, so the suite's own output stays readable and the line can be read
/// back rather than assumed.
const REAL_CONSOLE_ERROR = console.error;
const logged: unknown[][] = [];
console.error = (...line: unknown[]) => {
  logged.push(line);
};

beforeEach(() => {
  logged.length = 0;
});

after(async () => {
  console.error = REAL_CONSOLE_ERROR;
  await model.close();
});

function mail(overrides: Partial<MailFacts> = {}): MailFacts {
  return {
    from: "someone@example.com",
    to: "demo@usepostage.com",
    subject: "",
    body: "",
    spf: "pass",
    dkim: "pass",
    dmarc: "pass",
    urls: [],
    ...overrides,
  };
}

/// The rule the whole degraded path exists to keep: a model outage must never
/// let mail escalate to the tier that blocks delivery and charges for it. Every
/// header-only verdict is `degraded: true`, so this is really one rule proven
/// from several directions that would tempt "dangerous" if it were allowed.
test("a degraded verdict is never the dangerous tier, even for classic phishing language", () => {
  const verdict = classifyFromHeaders(
    mail({
      spf: "fail",
      dkim: "fail",
      dmarc: "fail",
      subject: "Your account is suspended",
      body: "Click here to verify your account and confirm your identity or it stays suspended.",
    })
  );

  assert.notEqual(verdict.tier, "dangerous");
  assert.equal(verdict.degraded, true);
});

test("failed authentication alone, with no lure language, still never reaches dangerous", () => {
  const verdict = classifyFromHeaders(mail({ spf: "fail", dkim: "fail", dmarc: "fail" }));
  assert.notEqual(verdict.tier, "dangerous");
});

test("a wire-transfer lure over a failed-auth domain is degraded to commercial, not dangerous", () => {
  const verdict = classifyFromHeaders(
    mail({
      dmarc: "fail",
      subject: "Urgent wire transfer needed",
      body: "Please process this gift card payment immediately.",
    })
  );

  assert.equal(verdict.tier, "commercial");
  assert.equal(verdict.degraded, true);
});

test("every degraded verdict, whatever its tier, says so", () => {
  for (const facts of [
    mail(),
    mail({ subject: "one-time verification code" }),
    mail({ body: "unsubscribe from this newsletter" }),
    mail({ dmarc: "fail", body: "click here to verify your account" }),
  ]) {
    assert.equal(classifyFromHeaders(facts).degraded, true);
  }
});

test("a transactional subject with clean authentication reads as important", () => {
  const verdict = classifyFromHeaders(mail({ subject: "Your one-time verification code" }));
  assert.equal(verdict.tier, "important");
});

/// Authentication failure overrides the transactional read: a password-reset
/// subject is exactly what a spoofed sender would pick, so a failed dmarc must
/// not still hand it the "someone is waiting for this" treatment.
test("a transactional subject over failed authentication does not get importance", () => {
  const verdict = classifyFromHeaders(
    mail({ dmarc: "fail", subject: "Your one-time verification code" })
  );
  assert.notEqual(verdict.tier, "important");
});

test("bulk marketing language reads as commercial", () => {
  const verdict = classifyFromHeaders(mail({ body: "50% off this week, unsubscribe anytime" }));
  assert.equal(verdict.tier, "commercial");
});

test("mail matching none of the header patterns still classifies as commercial", () => {
  const verdict = classifyFromHeaders(mail({ subject: "hi", body: "just saying hello" }));
  assert.equal(verdict.tier, "commercial");
  assert.ok(verdict.reasons.length > 0, "a verdict with no reason given explains nothing to the user");
});

test("no links in the body yields no urls", () => {
  assert.deepEqual(extractUrls("nothing to see here"), []);
});

test("an empty body yields no urls", () => {
  assert.deepEqual(extractUrls(""), []);
});

test("a link is extracted without markdown parentheses or surrounding quotes", () => {
  const found = extractUrls('See (http://example.com/foo) and "http://example.com/bar" now');
  assert.deepEqual(found, ["http://example.com/foo", "http://example.com/bar"]);
});

test("the same link repeated in the body is reported once", () => {
  const found = extractUrls("http://dup.example.com then again http://dup.example.com later too");
  assert.deepEqual(found, ["http://dup.example.com"]);
});

test("distinct links are all kept, in the order they first appear", () => {
  const found = extractUrls("first http://a.example.com then http://b.example.com then http://a.example.com");
  assert.deepEqual(found, ["http://a.example.com", "http://b.example.com"]);
});

/// The bug this pair of tests exists to catch: a classifier outage used to
/// degrade every verdict with nothing in the log to say why, which once cost
/// hours to trace back to a missing ANTHROPIC_API_KEY. The fallback itself is
/// unchanged (`classifyFromHeaders` is what it always was) — only its silence
/// is.
test("when the model is unreachable, classify() still returns the header-derived, degraded verdict", async () => {
  const facts = mail({ dmarc: "fail" });
  const verdict = await classify(facts);

  assert.deepEqual(verdict, classifyFromHeaders(facts));
  assert.equal(verdict.degraded, true);
});

test("when the model is unreachable, classify() logs why instead of failing silently", async () => {
  await classify(mail());

  const [message, context] = logged.find(([entryMessage]) => entryMessage === "classification degraded to header-only fallback") ?? [];

  assert.equal(message, "classification degraded to header-only fallback");
  assert.equal(typeof (context as { reason?: unknown })?.reason, "string");
  assert.ok(
    ((context as { reason: string }).reason).length > 0,
    "a degraded verdict must say why, not log an empty reason"
  );
});

test("the reason logged for a degraded verdict never carries the mail's subject or body", async () => {
  await classify(mail({ subject: "quarterly board minutes", body: "the acquisition price is $40M" }));

  const [, context] = logged.find(([entryMessage]) => entryMessage === "classification degraded to header-only fallback") ?? [];
  const reason = (context as { reason: string }).reason;

  assert.ok(!reason.includes("quarterly board minutes"));
  assert.ok(!reason.includes("acquisition price"));
});

test("sanitizedReason reads an Error's own message", () => {
  assert.equal(sanitizedReason(new Error("model unavailable")), "model unavailable");
});

test("sanitizedReason stringifies a thrown value that is not an Error", () => {
  assert.equal(sanitizedReason("timeout"), "timeout");
});

test("sanitizedReason leaves the real auth-resolution failure readable", () => {
  // The exact message `new Anthropic()` throws when no key is configured —
  // the bug this whole change exists to surface. It names the missing
  // setting, never a value, so nothing in it should be blanked out.
  const message =
    'Could not resolve authentication method. Expected one of apiKey, authToken, credentials, config, or profile to be set. Or for one of the "X-Api-Key" or "Authorization" headers to be explicitly omitted';

  assert.equal(sanitizedReason(new Error(message)), message);
});

test("sanitizedReason blanks out anything shaped like a credential", () => {
  const reason = sanitizedReason(
    new Error("401 invalid x-api-key sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789")
  );

  assert.ok(!reason.includes("sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789"));
  assert.ok(reason.includes("[redacted]"));
});

/// The prompt asks the model for short fragments, but a prompt is a request,
/// not a guarantee, so `tidy` enforces the shape in code. See its own comment
/// in `classify.ts` for where a `Verdict`'s reasons do and do not go — not the
/// held-mail notice, which renders pricing's strings instead. Pinned here
/// rather than at whichever consumer reads them next.
test("tidy truncates a reason longer than the word cap to eight words", () => {
  const [result] = tidy(["one two three four five six seven eight nine ten"]);
  assert.equal(result, "one two three four five six seven eight");
});

test("tidy strips a single trailing period", () => {
  const [result] = tidy(["sent from a bulk mail platform."]);
  assert.equal(result, "sent from a bulk mail platform");
});

test("tidy leaves an empty string empty rather than throwing", () => {
  const [result] = tidy([""]);
  assert.equal(result, "");
});

test("tidy leaves an already-clean, in-cap reason unchanged", () => {
  const [result] = tidy(["DMARC failed for a bank domain"]);
  assert.equal(result, "DMARC failed for a bank domain");
});

test("tidy trims leading and trailing whitespace", () => {
  const [result] = tidy(["  link text and destination disagree  "]);
  assert.equal(result, "link text and destination disagree");
});

/// The property `tidy` is kept for, now that nothing renders these reasons: a
/// plain-text mail body is exactly its own bytes, so a reason that keeps its
/// newlines is a reason that can write lines of its own wherever one is ever
/// printed. The model is asked for a fragment; what comes back is shaped by a
/// stranger's email.
test("tidy collapses a newline, so a reason cannot open a line of its own", () => {
  const [result] = tidy(["bulk mail platform\nI'M HUMAN - free"]);
  assert.equal(result, "bulk mail platform I'M HUMAN - free");
});

test("tidy tidies every reason in the array, independently", () => {
  const result = tidy(["short.", "one two three four five six seven eight nine"]);
  assert.deepEqual(result, ["short", "one two three four five six seven eight"]);
});

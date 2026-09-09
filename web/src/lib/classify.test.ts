import assert from "node:assert/strict";
import { test } from "node:test";
import { classifyFromHeaders, extractUrls, type MailFacts } from "./classify";

// Pure functions, no database and no model call: `classifyFromHeaders` and
// `extractUrls` never touch a network or a store, so this file needs neither a
// temp workspace nor any env var.

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

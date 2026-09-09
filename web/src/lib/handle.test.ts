import assert from "node:assert/strict";
import { test } from "node:test";
import {
  HANDLE_MAX_LENGTH,
  MAIL_DOMAIN,
  handleOf,
  hasRepeatedDot,
  isOurs,
  isValidHandle,
  keepHandleChars,
  normalizeHandle,
  postageAddress,
} from "./handle";

// Pure string functions, no database and no env — nothing here needs a
// workspace or a DATABASE_URL.

/// What the inbound mail route relies on `isOurs` for: a destination on our
/// own domain means forwarding it back out would mail ourselves, spending a
/// classify call, a chain read and a challenge row on every lap.
test("an address on our own domain is recognised as ours", () => {
  assert.equal(isOurs(`someone@${MAIL_DOMAIN}`), true);
});

test("an address on any other domain is not ours", () => {
  assert.equal(isOurs("someone@gmail.com"), false);
});

test("a domain that merely ends with ours as a suffix is not ours", () => {
  assert.equal(isOurs(`someone@evil-${MAIL_DOMAIN}`), false);
});

test("recognising our domain is case-insensitive", () => {
  assert.equal(isOurs(`someone@${MAIL_DOMAIN.toUpperCase()}`), true);
});

test("an address with no domain at all is not ours", () => {
  assert.equal(isOurs("not-an-address"), false);
});

test("postageAddress lowercases the handle before combining it with the domain", () => {
  assert.equal(postageAddress("Demo"), `demo@${MAIL_DOMAIN}`);
});

test("handleOf reads the local part back off an address it produced", () => {
  assert.equal(handleOf(postageAddress("Demo")), "demo");
});

test("handleOf reports no handle for an address with no local part", () => {
  assert.equal(handleOf(`@${MAIL_DOMAIN}`), "");
});

// isValidHandle — the same shape/length/repeated-dot rules the signup route
// enforces, sourced from one place so a client component can reach them too.

test("a handle within the charset and length is valid", () => {
  assert.equal(isValidHandle("demo.user-1"), true);
});

test("a handle shorter than the minimum is invalid", () => {
  assert.equal(isValidHandle("a"), false);
});

test("a handle longer than the maximum is invalid", () => {
  assert.equal(isValidHandle("a".repeat(HANDLE_MAX_LENGTH + 1)), false);
});

test("a handle starting with punctuation is invalid", () => {
  assert.equal(isValidHandle("-demo"), false);
});

test("a handle ending with punctuation is invalid", () => {
  assert.equal(isValidHandle("demo-"), false);
});

test("a handle with a character outside the charset is invalid", () => {
  assert.equal(isValidHandle("demo user"), false);
});

test("a handle with two dots running together is invalid", () => {
  assert.equal(isValidHandle("demo..user"), false);
});

test("uppercase is not part of the valid shape — callers lowercase first", () => {
  assert.equal(isValidHandle("Demo"), false);
});

test("hasRepeatedDot reports two adjacent dots anywhere in the string", () => {
  assert.equal(hasRepeatedDot("a..b"), true);
  assert.equal(hasRepeatedDot("a.b.c"), false);
});

// keepHandleChars — the claim form's live input filter: strips disallowed
// characters, keeps whatever case the user typed.

test("keepHandleChars drops characters outside the handle charset", () => {
  assert.equal(keepHandleChars("de mo!user@x"), "demouserx");
});

test("keepHandleChars preserves case, unlike the final validity rule", () => {
  assert.equal(keepHandleChars("DemoUser"), "DemoUser");
});

// normalizeHandle — turns free text into a handle that already satisfies
// isValidHandle. This is what the signup form suggests from an email's local
// part, and previously restated the server's rules by hand in ClaimInbox.tsx.

test("normalizeHandle lowercases and strips characters outside the charset", () => {
  assert.equal(normalizeHandle("John.Doe+Promo"), "john.doepromo");
});

test("normalizeHandle collapses runs of dots into one", () => {
  assert.equal(normalizeHandle("john...doe"), "john.doe");
});

test("normalizeHandle trims punctuation off both ends", () => {
  assert.equal(normalizeHandle("-.john.doe._-"), "john.doe");
});

test("normalizeHandle reports no suggestion once trimming leaves it too short", () => {
  assert.equal(normalizeHandle("."), "");
  assert.equal(normalizeHandle("a"), "");
});

test("normalizeHandle never hands back more than the maximum length", () => {
  const result = normalizeHandle("a".repeat(HANDLE_MAX_LENGTH + 10));
  assert.equal(result.length <= HANDLE_MAX_LENGTH, true);
});

// A cut at the length cap can land exactly on a dash or dot that the earlier
// edge-trim already passed — the client normaliser used to hand back a
// handle ending in punctuation in that case, which the server's own shape
// rule then rejects. This is the case that reproduces it.
test("normalizeHandle re-trims punctuation exposed by truncating to the length cap", () => {
  const local = "a".repeat(HANDLE_MAX_LENGTH - 1) + "-" + "b".repeat(5);
  const result = normalizeHandle(local);
  assert.equal(result, "a".repeat(HANDLE_MAX_LENGTH - 1));
  assert.equal(isValidHandle(result), true);
});

test("normalizeHandle's output always satisfies isValidHandle, for anything it accepts at all", () => {
  const samples = [
    "john.doe",
    "a".repeat(50),
    "-leading-and-trailing-",
    "dots....everywhere....",
    "MiXeD-CaSe.Name",
    "a".repeat(29) + "..",
  ];
  for (const sample of samples) {
    const result = normalizeHandle(sample);
    if (result === "") continue;
    assert.equal(isValidHandle(result), true, `normalizeHandle(${JSON.stringify(sample)}) -> ${JSON.stringify(result)}`);
  }
});

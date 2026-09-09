import assert from "node:assert/strict";
import { test } from "node:test";
import { causeMessage } from "./errors";

/// The common case: whatever threw was a genuine `Error`, so its `message`
/// is what a caller wants surfaced.
test("an Error yields its message", () => {
  assert.equal(causeMessage(new Error("db unreachable")), "db unreachable");
});

/// A subclass still carries `message` through the same branch.
test("a subclass of Error yields its message", () => {
  class QuoteExpiredError extends Error {}
  assert.equal(causeMessage(new QuoteExpiredError("quote expired")), "quote expired");
});

/// The branch that exists precisely because `catch` hands back `unknown`:
/// something other than an `Error` was thrown, so it falls back to
/// `String(cause)` rather than reaching for a `.message` that isn't there.
test("a thrown string is stringified rather than treated as an Error", () => {
  assert.equal(causeMessage("timed out"), "timed out");
});

test("a thrown plain object is stringified", () => {
  assert.equal(causeMessage({ code: "ECONNRESET" }), "[object Object]");
});

test("a thrown number is stringified", () => {
  assert.equal(causeMessage(42), "42");
});

test("a thrown null is stringified", () => {
  assert.equal(causeMessage(null), "null");
});

test("a thrown undefined is stringified", () => {
  assert.equal(causeMessage(undefined), "undefined");
});

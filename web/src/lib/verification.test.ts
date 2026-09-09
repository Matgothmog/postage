import assert from "node:assert/strict";
import { test } from "node:test";

// No database is touched here — hashing and comparing a code are pure crypto
// over an env-derived key — so this file needs no temp workspace, only the
// secret `key()` requires.
process.env.MESSAGE_ID_SECRET = "x".repeat(32);

const { codeMatches, generateCode, hashCode } = await import("./verification");

test("a code matches the hash it was minted from", () => {
  const code = generateCode();
  const stored = hashCode("demo", code);
  assert.equal(codeMatches("demo", code, stored), true);
});

test("a wrong code does not match another code's hash", () => {
  const stored = hashCode("demo", "123456");
  assert.equal(codeMatches("demo", "654321", stored), false);
});

/// The short-circuit `codeMatches` takes before it ever reaches
/// `timingSafeEqual`, which throws rather than returning false on a length
/// mismatch. A hash of the wrong length has to be a mismatch, not a crash.
test("a hash of the wrong length is rejected without throwing", () => {
  const code = generateCode();
  assert.equal(codeMatches("demo", code, "deadbeef"), false);
});

test("an empty expected hash is rejected without throwing", () => {
  const code = generateCode();
  assert.equal(codeMatches("demo", code, ""), false);
});

/// The binding the comment on `hashCode` promises: a code minted for one
/// handle must not settle a claim on a different handle, even offered back
/// with its own correct code and hash for its own handle mixed in.
test("a code minted for one handle does not match a different handle", () => {
  const code = generateCode();
  const stored = hashCode("alice", code);
  assert.equal(codeMatches("bob", code, stored), false);
});

test("handle comparison is case-insensitive, matching how the hash was minted", () => {
  const code = generateCode();
  const stored = hashCode("Demo", code);
  assert.equal(codeMatches("demo", code, stored), true);
});

test("generateCode always produces a six digit string, zero-padded", () => {
  for (let n = 0; n < 50; n += 1) {
    const code = generateCode();
    assert.equal(code.length, 6);
    assert.match(code, /^\d{6}$/);
  }
});

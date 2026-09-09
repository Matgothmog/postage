import assert from "node:assert/strict";
import { test } from "node:test";
import { parseStoredQuote } from "./quote-types";

// Pure parsing/validation, no database and no env — nothing here needs a
// workspace or a DATABASE_URL.

/// A row of the shape `issueChallenge` actually writes: a `bytes32` message
/// id, a 20-byte inbox address and a 65-byte signature, all of them widths the
/// escrow call reads back off this quote depends on.
const VALID = {
  messageId: `0x${"ab".repeat(32)}`,
  inbox: `0x${"cd".repeat(20)}`,
  tier: "commercial",
  amount: "1000000",
  expiresAt: 1_700_000_000,
  signature: `0x${"ef".repeat(65)}`,
  reasons: ["looked automated", "no prior pass"],
};

/// A field moved on its own, so a rejection is attributable to that field.
function withField(field: string, value: unknown): string {
  return JSON.stringify({ ...VALID, [field]: value });
}

/// The happy path a `challenges.quote_json` row takes every day: a row this
/// module itself wrote, round-tripped through JSON exactly once.
test("a well-formed quote round-trips", () => {
  assert.deepEqual(parseStoredQuote(JSON.stringify(VALID)), VALID);
});

/// Not JSON at all — a truncated write is the likely real-world cause.
test("text that is not JSON returns null instead of throwing", () => {
  assert.equal(parseStoredQuote("{not json"), null);
});

/// Valid JSON, but not an object — an older or unrelated column value.
for (const raw of ['"just a string"', "42", "null", "[1,2,3]"]) {
  test(`JSON that is not an object (${raw}) returns null`, () => {
    assert.equal(parseStoredQuote(raw), null);
  });
}

/// A row from a schema that no longer matches: a field missing entirely.
test("an object missing a required field returns null", () => {
  const withoutSignature: Record<string, unknown> = { ...VALID };
  delete withoutSignature.signature;
  assert.equal(parseStoredQuote(JSON.stringify(withoutSignature)), null);
});

/// A field present but the wrong type — `amount` as a number rather than the
/// string every quote is stored and formatted as.
test("an object with a field of the wrong type returns null", () => {
  assert.equal(parseStoredQuote(JSON.stringify({ ...VALID, amount: 1_000_000 })), null);
});

/// `reasons` must be a string array, not merely present.
test("a reasons array holding a non-string entry returns null", () => {
  assert.equal(parseStoredQuote(JSON.stringify({ ...VALID, reasons: ["fine", 7] })), null);
});

/// `typeof quote.amount === "string"` narrows the type and says nothing about
/// the value, and the value is what `BigInt(quote.amount)` reads — inside a
/// Server Component, which is the exact render this guard was added to keep
/// up. `"1e18"` is the shape that makes the point: a number to a person, a
/// throw to `BigInt`, and indistinguishable from a real amount by type alone.
test("an amount BigInt cannot read returns null rather than throwing in the render", () => {
  for (const amount of ["1e18", "1.5", "", "  12  ", "0x10", "-1", "12n", "one", "1_000"]) {
    assert.equal(parseStoredQuote(withField("amount", amount)), null, `amount ${JSON.stringify(amount)}`);
  }
});

/// The bound has to be on the notation rather than the magnitude: the escrow
/// takes a uint256, so a long run of digits is a real amount, not a suspect one.
test("an amount of plain digits is read however long it is", () => {
  const wide = "9".repeat(40);
  assert.equal(parseStoredQuote(withField("amount", wide))?.amount, wide);
});

/// `messageId` and `inbox` are cast to `0x${string}` and handed to
/// `encodeFunctionData` as a `bytes32` and an `address`. A cast checks nothing,
/// and viem throws on a value that is not the width the type says it is — so
/// the width is what has to be checked here.
test("a messageId that is not 32 bytes of hex returns null", () => {
  for (const messageId of ["0xabc", "0x", "", `${"ab".repeat(32)}`, `0x${"ab".repeat(31)}`, `0x${"ab".repeat(33)}`, `0x${"zz".repeat(32)}`]) {
    assert.equal(parseStoredQuote(withField("messageId", messageId)), null, `messageId ${JSON.stringify(messageId)}`);
  }
});

test("an inbox that is not a 20-byte address returns null", () => {
  for (const inbox of ["0xdef", "0x", "", `0x${"cd".repeat(19)}`, `0x${"cd".repeat(21)}`, `0x${"gg".repeat(20)}`]) {
    assert.equal(parseStoredQuote(withField("inbox", inbox)), null, `inbox ${JSON.stringify(inbox)}`);
  }
});

/// A quote is signed by one signer with one scheme, so its signature is always
/// the same 65 bytes. A row carrying anything else was not written by
/// `signQuote`, and the escrow would not recover an address from it — so the
/// broken-link page is the honest answer, not a pay button that always reverts.
test("a signature that is not the 65 bytes a quote is signed with returns null", () => {
  for (const signature of ["0x1234", "0x", "", "0xabc", `0x${"ef".repeat(64)}`, `0x${"zz".repeat(65)}`]) {
    assert.equal(parseStoredQuote(withField("signature", signature)), null, `signature ${JSON.stringify(signature)}`);
  }
});

/// `expiresAt` is a `uint40` on the other side of the escrow call, and
/// `typeof === "number"` admits every value JavaScript calls one.
test("an expiresAt outside the uint40 the escrow takes returns null", () => {
  for (const expiresAt of [-1, 1.5, 2 ** 40, Number.MAX_SAFE_INTEGER]) {
    assert.equal(parseStoredQuote(withField("expiresAt", expiresAt)), null, `expiresAt ${expiresAt}`);
  }
});

/// `JSON.parse` really does produce this: an exponent past what a double holds
/// reads back as `Infinity`, which is a number by every test that asks its
/// type and nothing viem can encode. It cannot be written with
/// `JSON.stringify`, which turns it into `null`, so the row is spelt out.
test("an expiresAt that JSON parsed as Infinity returns null", () => {
  const overflowed = JSON.stringify({ ...VALID, expiresAt: 0 }).replace('"expiresAt":0', '"expiresAt":1e400');

  assert.equal(JSON.parse(overflowed).expiresAt, Infinity, "precondition: this row really does parse as Infinity");
  assert.equal(parseStoredQuote(overflowed), null);
});

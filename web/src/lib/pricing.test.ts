import assert from "node:assert/strict";
import { test } from "node:test";

import type { SenderSignals } from "./reputation";
import { quote } from "./pricing";

const FLOOR = 1000n;
const YEAR_SECONDS = 365 * 24 * 60 * 60;
const twoYearsAgo = Math.floor(Date.now() / 1000) - 2 * YEAR_SECONDS;

/// Every field defaults to "no signal at all" so a test only has to name the
/// ones it cares about, and a reader only has to look at those to see what
/// is being priced.
function signals(overrides: Partial<SenderSignals>): SenderSignals {
  return {
    paidCount: 0,
    spamReports: 0,
    spamRate: 0,
    ensNames: 0,
    oldestEnsAt: null,
    ...overrides,
  };
}

/// TIER_BPS.important exists only because the table has to be total for a
/// `Record<Tier, number>` — quote() never indexes it, because "important"
/// returns before that line runs. Pinning that value would test a number
/// nothing reads; pinning the early return instead is what actually matters,
/// so every signal here is dialled up as if it should be expensive.
test("important mail is free and ignores every signal that would otherwise raise its price", () => {
  const result = quote(
    FLOOR,
    "important",
    signals({ paidCount: 0, spamReports: 50, spamRate: 1, ensNames: 0, oldestEnsAt: null }),
    true
  );

  assert.equal(result.free, true);
  assert.equal(result.amount, 0n);
  assert.equal(result.multiplierBps, 0);
  assert.equal(result.floor, FLOOR, "the floor is still echoed back on a free quote");
});

test("human and commercial mail price identically at the floor when nothing is known about the sender", () => {
  const human = quote(FLOOR, "human", null, false);
  const commercial = quote(FLOOR, "commercial", null, false);

  assert.equal(human.amount, FLOOR);
  assert.equal(commercial.amount, FLOOR);
  assert.equal(human.free, false);
});

test("dangerous mail is priced ten times the floor by default", () => {
  const result = quote(FLOOR, "dangerous", null, false);

  assert.equal(result.amount, 10_000n);
});

test("a degraded verdict downgrades dangerous mail from ten times the floor to two times", () => {
  const result = quote(FLOOR, "dangerous", null, true);

  assert.equal(result.amount, 2000n);
});

test("the header-only downgrade applies only to the dangerous tier, not to a degraded human verdict", () => {
  const result = quote(FLOOR, "human", null, true);

  assert.equal(result.amount, FLOOR, "human pricing is unmoved by the degraded flag");
});

test("a sender with no reported spam adds no surcharge", () => {
  const result = quote(FLOOR, "human", signals({ paidCount: 1, spamRate: 0 }), false);

  assert.equal(result.amount, FLOOR);
});

test("a spam rate on past messages adds a surcharge proportional to that rate", () => {
  const result = quote(FLOOR, "human", signals({ paidCount: 1, spamReports: 1, spamRate: 0.1 }), false);

  assert.equal(result.amount, 1400n);
});

test("fewer than three paid messages earns no loyalty discount, however clean the record", () => {
  const result = quote(FLOOR, "dangerous", signals({ paidCount: 2, spamRate: 0 }), false);

  assert.equal(result.amount, 10_000n, "same price as no history at all");
});

test("three or more well-received messages earn a loyalty discount", () => {
  const result = quote(FLOOR, "dangerous", signals({ paidCount: 3, spamRate: 0 }), false);

  assert.equal(result.amount, 5000n, "half the undiscounted dangerous price");
});

/// The two conditions read the same field, so this also exercises the
/// surcharge at the same spam rate — a rate this high moves both branches at
/// once, which is what actually happens for a real sender crossing it.
test("a spam rate exactly at the loyalty threshold forfeits the discount that a rate just under it keeps", () => {
  const justUnder = quote(FLOOR, "dangerous", signals({ paidCount: 3, spamRate: 0.19 }), false);
  const atThreshold = quote(FLOOR, "dangerous", signals({ paidCount: 3, spamRate: 0.2 }), false);

  assert.equal(justUnder.amount, 5380n);
  assert.equal(atThreshold.amount, 10_000n, "0.2 is not less than 0.2, so no loyalty discount applies");
});

test("an ENS name earns its own discount", () => {
  const recent = Math.floor(Date.now() / 1000) - 10;
  const result = quote(FLOOR, "dangerous", signals({ ensNames: 1, oldestEnsAt: recent }), false);

  assert.equal(result.amount, 7000n);
});

test("an ENS name registered over a year ago earns a deeper discount than a fresh one", () => {
  const result = quote(FLOOR, "dangerous", signals({ ensNames: 1, oldestEnsAt: twoYearsAgo }), false);

  assert.equal(result.amount, 5600n, "the 0.7 ENS discount compounds with a further 0.8 for its age");
});

test("the loyalty and ENS discounts compound rather than override each other", () => {
  const recent = Math.floor(Date.now() / 1000) - 10;
  const loyaltyOnly = quote(FLOOR, "dangerous", signals({ paidCount: 3, spamRate: 0 }), false);
  const ensOnly = quote(FLOOR, "dangerous", signals({ ensNames: 1, oldestEnsAt: recent }), false);
  const both = quote(
    FLOOR,
    "dangerous",
    signals({ paidCount: 3, spamRate: 0, ensNames: 1, oldestEnsAt: recent }),
    false
  );

  assert.equal(both.amount, 3500n);
  assert.ok(both.amount < loyaltyOnly.amount && both.amount < ensOnly.amount, "stacked discounts beat either alone");
});

test("combined discounts can never cut a price below the inbox's floor", () => {
  const result = quote(
    FLOOR,
    "human",
    signals({ paidCount: 3, spamRate: 0, ensNames: 1, oldestEnsAt: twoYearsAgo }),
    false
  );

  assert.equal(result.amount, FLOOR);
  assert.equal(result.multiplierBps, 10_000);
});

test("a spam surcharge can never push a price past the inbox's ceiling, however high the reported rate", () => {
  const high = quote(FLOOR, "dangerous", signals({ paidCount: 1, spamRate: 1 }), false);
  const extreme = quote(FLOOR, "dangerous", signals({ paidCount: 1, spamRate: 5 }), false);

  assert.equal(high.amount, 10_000n);
  assert.equal(extreme.amount, 10_000n, "a wildly higher reported rate is capped to the same price as a merely high one");
});

test("a zero floor prices every tier at zero, but the quote is still not free", () => {
  const result = quote(0n, "commercial", null, false);

  assert.equal(result.amount, 0n);
  assert.equal(result.free, false);
});

test("an oldestEnsAt timestamp of 0 (genesis block) earns the age discount, not null", () => {
  /// The age discount only applies if oldestEnsAt is not null. A timestamp of 0
  /// is a real, valid timestamp (genesis block era), not the absence of a value.
  /// The bug was using a truthiness check, which treats 0 as falsy.
  const withZero = quote(FLOOR, "dangerous", signals({ ensNames: 1, oldestEnsAt: 0 }), false);
  const withNull = quote(FLOOR, "dangerous", signals({ ensNames: 1, oldestEnsAt: null }), false);

  /// With 0 as oldestEnsAt, the age is extremely large (now - 0), so age > YEAR_SECONDS,
  /// and we get both the 0.7 ENS discount and the 0.8 age discount.
  /// Expected: floor=1000, bps starts at 10000 (dangerous), * 0.7 (ENS) = 7000, * 0.8 (age) = 5600
  assert.equal(withZero.amount, 5600n, "oldestEnsAt=0 earns the age discount");

  /// With null, only the ENS discount applies.
  /// Expected: 10000 * 0.7 = 7000
  assert.equal(withNull.amount, 7000n, "oldestEnsAt=null does not earn the age discount");
});

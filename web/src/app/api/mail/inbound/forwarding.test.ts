import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";
import type { Verdict } from "@/lib/classify";
import type { Tier } from "@/lib/tiers";

const workspace = mkdtempSync(join(tmpdir(), "postage-forwarding-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";

const { reset } = await import("@/lib/db/client");
const { grantPass, hasLivePass } = await import("@/lib/db/passes");
const { deliveredFree, forwardWithoutChallenge } = await import("./forwarding");

const HANDLE = "demo";
const SENDER = "sender@x.com";

function verdictOf(tier: Tier, degraded = false): Verdict {
  return { tier, confidence: 0.9, reasons: [], degraded };
}

beforeEach(async () => {
  await reset();
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

test("mail the model itself called important is delivered free", () => {
  assert.equal(deliveredFree(verdictOf("important"), true, null), true);
});

/// The tier exists so that login codes keep arriving while the model is down.
test("a degraded important verdict still goes free while there was budget to read it", () => {
  assert.equal(deliveredFree(verdictOf("important", true), true, null), true);
});

/// The rule with teeth. The header fallback calls anything transactional-sounding
/// important, so a sender who empties their own slice on purpose would otherwise
/// have bought their way into the tier nobody pays for.
test("a sender who spent their own slice cannot be delivered free on a degraded verdict", () => {
  assert.equal(deliveredFree(verdictOf("important", true), true, "spent-by-sender"), false);
});

/// The same rule at the next ceiling out. A domain is bought once and its local
/// parts are free after that, so a spent domain must buy no more than a spent
/// address does.
test("a domain that spent its share cannot be delivered free on a degraded verdict", () => {
  assert.equal(deliveredFree(verdictOf("important", true), true, "spent-by-domain"), false);
});

/// The consequential one. This used to return true, on the reasoning that a
/// handle's pool is somebody else's doing - but somebody else can be the sender
/// standing here, and emptying it took nothing but authenticated mail from a few
/// domains. That made free delivery, for every sender for the rest of the hour,
/// something an attacker could simply arrange.
test("a drained handle pool does not open the free tier to the next sender", () => {
  assert.equal(deliveredFree(verdictOf("important", true), true, "spent-by-handle"), false);
});

test("unauthenticated mail is never delivered free on a degraded verdict", () => {
  assert.equal(deliveredFree(verdictOf("important", true), false, "spent-by-sender"), false);
});

test("no other tier is free, however the verdict was reached", () => {
  for (const tier of ["human", "commercial", "dangerous"] as const) {
    assert.equal(deliveredFree(verdictOf(tier), true, null), false, `${tier} must not be free`);
    assert.equal(deliveredFree(verdictOf(tier, true), true, null), false, `${tier} must not be free`);
  }
});

/// This tier grants nothing, so taking a paid use for it would charge someone
/// twice for one delivery.
test("the free tier is taken before any pass is spent", async () => {
  await grantPass(HANDLE, SENDER, "paid", 1);

  const forwarded = await forwardWithoutChallenge({
    handle: HANDLE,
    sender: SENDER,
    verdict: verdictOf("important"),
    authenticated: true,
    budgetRefusal: null,
  });

  assert.deepEqual(forwarded, { reason: "important" });
  assert.equal(
    await hasLivePass(HANDLE, SENDER),
    true,
    "a delivery nobody was charged for must not spend what the sender paid"
  );
});

test("a live pass carries a message that was not called dangerous", async () => {
  await grantPass(HANDLE, SENDER, "human", null);

  const forwarded = await forwardWithoutChallenge({
    handle: HANDLE,
    sender: SENDER,
    verdict: verdictOf("commercial"),
    authenticated: true,
    budgetRefusal: null,
  });

  assert.deepEqual(forwarded, { reason: "human" });
});

/// Proving personhood does not clear this tier - a real person can still be
/// phishing.
test("dangerous mail is not forwarded by a pass", async () => {
  await grantPass(HANDLE, SENDER, "human", null);

  const forwarded = await forwardWithoutChallenge({
    handle: HANDLE,
    sender: SENDER,
    verdict: verdictOf("dangerous"),
    authenticated: true,
    budgetRefusal: null,
  });

  assert.equal(forwarded, null);
});

/// The allowlist is keyed on the envelope sender, so honouring a pass for mail
/// nobody could authenticate would let anyone through by writing someone else's
/// name on it.
test("an unauthenticated sender cannot spend the pass their address earned", async () => {
  await grantPass(HANDLE, SENDER, "human", null);

  const forwarded = await forwardWithoutChallenge({
    handle: HANDLE,
    sender: SENDER,
    verdict: verdictOf("commercial"),
    authenticated: false,
    budgetRefusal: "spent-by-sender",
  });

  assert.equal(forwarded, null);
});

/// An unlimited window plus a budget the sender exhausted themselves is a
/// licence to deliver anything unread, so that one shuts.
test("a sender who spent their own slice cannot ride an unlimited window", async () => {
  await grantPass(HANDLE, SENDER, "human", null);

  const forwarded = await forwardWithoutChallenge({
    handle: HANDLE,
    sender: SENDER,
    verdict: verdictOf("commercial"),
    authenticated: true,
    budgetRefusal: "spent-by-sender",
  });

  assert.equal(forwarded, null);
});

/// A single paid use cannot flood by construction - one message, already paid
/// for - and refusing it would take the money and demand it again.
test("a paid delivery still goes through when the sender spent their own slice", async () => {
  await grantPass(HANDLE, SENDER, "paid", 1);

  const forwarded = await forwardWithoutChallenge({
    handle: HANDLE,
    sender: SENDER,
    verdict: verdictOf("commercial"),
    authenticated: true,
    budgetRefusal: "spent-by-sender",
  });

  assert.deepEqual(forwarded, { reason: "paid" });
});

test("a pool drained by other people does not re-challenge a sender who proved themselves", async () => {
  await grantPass(HANDLE, SENDER, "human", null);

  const forwarded = await forwardWithoutChallenge({
    handle: HANDLE,
    sender: SENDER,
    verdict: verdictOf("commercial"),
    authenticated: true,
    budgetRefusal: "spent-by-handle",
  });

  assert.deepEqual(forwarded, { reason: "human" });
});

/// A domain rations a shared cost; it does not name a culprit. `gmail.com` is
/// millions of unrelated people, so treating a spent domain the way a spent
/// address is treated would take an unlimited window away from someone for what
/// a stranger sharing their mail provider did.
test("a domain drained by others does not re-challenge a sender who proved themselves", async () => {
  await grantPass(HANDLE, SENDER, "human", null);

  const forwarded = await forwardWithoutChallenge({
    handle: HANDLE,
    sender: SENDER,
    verdict: verdictOf("commercial"),
    authenticated: true,
    budgetRefusal: "spent-by-domain",
  });

  assert.deepEqual(forwarded, { reason: "human" });
});

test("a stranger holding no pass is left to the challenge", async () => {
  const forwarded = await forwardWithoutChallenge({
    handle: HANDLE,
    sender: SENDER,
    verdict: verdictOf("commercial"),
    authenticated: true,
    budgetRefusal: null,
  });

  assert.equal(forwarded, null);
});

import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";
import { now } from "../time";

// A controlled database of its own before the module under test is imported,
// the same pattern every db-touching test file in this tree follows.
const workspace = mkdtempSync(join(tmpdir(), "postage-nullifiers-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";

const { all, reset } = await import("./client");
const { claimNullifier, rebindsOf, senderHoldingNullifier } = await import("./nullifiers");
const { addPaidUse, grantPass, hasLivePass } = await import("./passes");

const NULLIFIER = `0x${"ab".repeat(32)}`;
const OTHER_NULLIFIER = `0x${"cd".repeat(32)}`;
const SENDER = "first@x.com";
const OTHER_SENDER = "second@x.com";
const HANDLE = "demo";
const OTHER_HANDLE = "second-demo";

async function usesLeftOf(handle: string, sender: string): Promise<number | null> {
  const rows = await all<{ uses_left: number | null }>(
    `SELECT uses_left FROM passes WHERE handle = ? AND sender = ?`,
    [handle, sender]
  );
  return rows[0]?.uses_left ?? null;
}

/// The hops as "who to whom", which is what every assertion about the trail is
/// actually about; the timestamp gets its own test rather than being restated
/// in each of them.
async function hopsOf(nullifierHash: string): Promise<string[]> {
  const hops = await rebindsOf(nullifierHash);
  return hops.map((hop) => `${hop.from_sender} -> ${hop.to_sender}`);
}

beforeEach(async () => {
  await reset();
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

test("a nullifier nobody holds binds to the sender presenting it", async () => {
  assert.deepEqual(await claimNullifier(NULLIFIER, SENDER), { rebound: false });
  assert.equal(await senderHoldingNullifier(NULLIFIER), SENDER);
});

test("the sender already holding a nullifier may present it again, and nothing moves", async () => {
  await claimNullifier(NULLIFIER, SENDER);

  assert.deepEqual(await claimNullifier(NULLIFIER, SENDER), { rebound: false });
  assert.deepEqual(await hopsOf(NULLIFIER), []);
});

test("a different sender presenting the same nullifier takes the binding over", async () => {
  await claimNullifier(NULLIFIER, SENDER);

  assert.deepEqual(await claimNullifier(NULLIFIER, OTHER_SENDER), {
    rebound: true,
    releasedFrom: SENDER,
  });
  assert.equal(await senderHoldingNullifier(NULLIFIER), OTHER_SENDER);
});

test("a sender whose nullifier was spent under somebody else's address takes it back", async () => {
  // The poisoning this policy exists for: a proof binds to the sender named on
  // the challenge token, which is whoever mailed the handle, not whoever took
  // the selfie. So the ledger can end up naming a stranger. Presenting again
  // from the real holder's own address is the whole of the remedy.
  await claimNullifier(NULLIFIER, OTHER_SENDER);

  assert.deepEqual(await claimNullifier(NULLIFIER, SENDER), {
    rebound: true,
    releasedFrom: OTHER_SENDER,
  });
  assert.equal(await senderHoldingNullifier(NULLIFIER), SENDER);
});

test("a takeover is written down, so a moved binding is visible after the fact", async () => {
  await claimNullifier(NULLIFIER, SENDER);
  await claimNullifier(NULLIFIER, OTHER_SENDER);

  assert.deepEqual(await hopsOf(NULLIFIER), [`${SENDER} -> ${OTHER_SENDER}`]);
});

test("every hop is kept, so a binding passed back and forth reads as a chain", async () => {
  await claimNullifier(NULLIFIER, SENDER);
  await claimNullifier(NULLIFIER, OTHER_SENDER);
  await claimNullifier(NULLIFIER, SENDER);

  assert.deepEqual(await hopsOf(NULLIFIER), [
    `${SENDER} -> ${OTHER_SENDER}`,
    `${OTHER_SENDER} -> ${SENDER}`,
  ]);
});

test("a hop is stamped with when it happened", async () => {
  const before = now();
  await claimNullifier(NULLIFIER, SENDER);
  await claimNullifier(NULLIFIER, OTHER_SENDER);

  const [hop] = await rebindsOf(NULLIFIER);
  assert.equal(hop.at >= before, true, `expected ${hop.at} to be at or after ${before}`);
});

test("one person's nullifier says nothing about another's", async () => {
  await claimNullifier(NULLIFIER, SENDER);

  assert.deepEqual(await claimNullifier(OTHER_NULLIFIER, OTHER_SENDER), { rebound: false });
  assert.equal(await senderHoldingNullifier(NULLIFIER), SENDER);
});

test("case is not a second identity: re-presenting in another casing moves nothing", async () => {
  await claimNullifier(NULLIFIER, SENDER);

  assert.deepEqual(
    await claimNullifier(NULLIFIER.toUpperCase().replace("0X", "0x"), SENDER.toUpperCase()),
    { rebound: false }
  );
  assert.deepEqual(await hopsOf(NULLIFIER), []);
});

test("an unclaimed nullifier is held by nobody and has moved nowhere", async () => {
  assert.equal(await senderHoldingNullifier(NULLIFIER), null);
  assert.deepEqual(await hopsOf(NULLIFIER), []);
});

test("two senders racing for a free nullifier leave one holder and one recorded move", async () => {
  const claims = await Promise.all([
    claimNullifier(NULLIFIER, SENDER),
    claimNullifier(NULLIFIER, OTHER_SENDER),
  ]);

  // Which of the two ends up holding it is not decided here. What is: one of
  // them created the binding and the other moved it, so the race cannot leave
  // the ledger naming both or recording a move nobody made.
  assert.equal(claims.filter((claim) => claim.rebound).length, 1);
  const hops = await rebindsOf(NULLIFIER);
  assert.equal(hops.length, 1);
  assert.equal(hops[0].to_sender, await senderHoldingNullifier(NULLIFIER));
});

test("forty senders racing for one nullifier leave an unbroken chain ending at the holder", async () => {
  const senders = Array.from({ length: 40 }, (_, index) => `racer-${index}@x.com`);

  const claims = await Promise.all(senders.map((sender) => claimNullifier(NULLIFIER, sender)));

  // The invariant a read-then-write cannot hold. Exactly one claim finds the
  // nullifier free; every other one must displace precisely the sender the
  // claim before it installed. If a claim could read the holder before a rival
  // wrote and record its own move afterwards, a hop goes missing or names
  // somebody who was never the holder — which is what this asserts against.
  assert.equal(claims.filter((claim) => claim.rebound).length, senders.length - 1);

  const hops = await rebindsOf(NULLIFIER);
  assert.equal(hops.length, senders.length - 1);
  for (const [index, hop] of hops.entries()) {
    if (index === 0) continue;
    assert.equal(hop.from_sender, hops[index - 1].to_sender, `hop ${index} does not follow hop ${index - 1}`);
  }
  assert.equal(hops.at(-1)?.to_sender, await senderHoldingNullifier(NULLIFIER));
  assert.equal(new Set(hops.map((hop) => hop.to_sender)).size, hops.length, "a sender took the binding twice");
});

test("a re-bind revokes the released sender's earned pass", async () => {
  await grantPass(HANDLE, SENDER, "human", null);
  await claimNullifier(NULLIFIER, SENDER);

  await claimNullifier(NULLIFIER, OTHER_SENDER);

  assert.equal(await hasLivePass(HANDLE, SENDER), false);
});

test("a re-bind never touches the released sender's paid pass", async () => {
  await addPaidUse(HANDLE, SENDER);
  await claimNullifier(NULLIFIER, SENDER);

  await claimNullifier(NULLIFIER, OTHER_SENDER);

  assert.equal(await hasLivePass(HANDLE, SENDER), true);
  assert.equal(await usesLeftOf(HANDLE, SENDER), 1);
});

/// The money bug this charge exists to close. `addPaidUse` run against a row
/// that already reads `uses_left IS NULL` takes its unlimited-window branch
/// and only pushes `expires_at` out — there is no count there to add a use
/// to, so the payment is recorded as an extension of the unlimited window
/// and `uses_left` stays NULL. Before `paid_extended_at` existed, that row
/// was indistinguishable from one purely earned, and this rebind's DELETE
/// took it along with the nullifier: money in, nothing out.
test("a re-bind never touches an unlimited window that payment has extended", async () => {
  await grantPass(HANDLE, SENDER, "human", null);
  await addPaidUse(HANDLE, SENDER);
  await claimNullifier(NULLIFIER, SENDER);

  await claimNullifier(NULLIFIER, OTHER_SENDER);

  assert.equal(await hasLivePass(HANDLE, SENDER), true);
});

/// Ported from the now-removed `revokeHumanPasses` (which had no production
/// call site — this batch statement is the only DELETE that ever ships)
/// so the shipped path keeps pinning this case: `grantPass`'s own
/// `ON CONFLICT` keeps a positive `uses_left` even while overwriting `reason`
/// to `"human"` on top of it, so a row that reads `reason = 'human'` can
/// still be sitting on a paid, unspent balance.
test("a re-bind never touches a paid balance even after its reason is overwritten to human", async () => {
  await grantPass(HANDLE, SENDER, "paid", 3);
  await grantPass(HANDLE, SENDER, "human", null);
  assert.equal(await usesLeftOf(HANDLE, SENDER), 3, "test setup: the paid count must have carried over");
  await claimNullifier(NULLIFIER, SENDER);

  await claimNullifier(NULLIFIER, OTHER_SENDER);

  assert.equal(await hasLivePass(HANDLE, SENDER), true);
  assert.equal(await usesLeftOf(HANDLE, SENDER), 3);
});

test("a re-bind revokes the released sender's earned passes across every handle, not just one", async () => {
  await grantPass(HANDLE, SENDER, "human", null);
  await grantPass(OTHER_HANDLE, SENDER, "human", null);
  await claimNullifier(NULLIFIER, SENDER);

  await claimNullifier(NULLIFIER, OTHER_SENDER);

  assert.equal(await hasLivePass(HANDLE, SENDER), false);
  assert.equal(await hasLivePass(OTHER_HANDLE, SENDER), false);
});

test("a same-sender re-claim revokes nothing", async () => {
  await grantPass(HANDLE, SENDER, "human", null);
  await claimNullifier(NULLIFIER, SENDER);

  await claimNullifier(NULLIFIER, SENDER);

  assert.equal(await hasLivePass(HANDLE, SENDER), true);
});

test("a claim on an unrelated nullifier does not revoke a sender's pass", async () => {
  await grantPass(HANDLE, SENDER, "human", null);

  await claimNullifier(OTHER_NULLIFIER, OTHER_SENDER);

  assert.equal(await hasLivePass(HANDLE, SENDER), true);
});

test("two senders racing for a free nullifier revoke exactly the one who loses it", async () => {
  await grantPass(HANDLE, SENDER, "human", null);
  await grantPass(HANDLE, OTHER_SENDER, "human", null);

  // Both present a valid proof for the same, currently unclaimed nullifier at
  // once. Which of them ends up holding it is not decided here — the same
  // race the existing "two senders racing for a free nullifier" test above
  // leaves undetermined — only that whichever one does NOT end up holding it
  // must have had their pass revoked, and the one who does must still have
  // theirs, however the race actually lands.
  const claims = await Promise.all([claimNullifier(NULLIFIER, SENDER), claimNullifier(NULLIFIER, OTHER_SENDER)]);

  assert.equal(claims.filter((claim) => claim.rebound).length, 1, "exactly one of the two claims displaces the other");
  const holder = await senderHoldingNullifier(NULLIFIER);
  const displaced = holder === SENDER ? OTHER_SENDER : SENDER;
  assert.equal(await hasLivePass(HANDLE, holder as string), true, "the winner's own pass must survive, untouched");
  assert.equal(await hasLivePass(HANDLE, displaced), false, "the loser's pass must be revoked, not merely left be");
});

test("forty senders racing for one nullifier leave passes revoked everywhere but the final holder", async () => {
  const senders = Array.from({ length: 40 }, (_, index) => `pass-racer-${index}@x.com`);
  for (const sender of senders) await grantPass(HANDLE, sender, "human", null);

  await Promise.all(senders.map((sender) => claimNullifier(NULLIFIER, sender)));

  const holder = await senderHoldingNullifier(NULLIFIER);
  for (const sender of senders) {
    assert.equal(
      await hasLivePass(HANDLE, sender),
      sender === holder,
      `${sender} should hold a live pass only if they are the final holder (${holder})`
    );
  }
});

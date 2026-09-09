import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";

// A controlled database of its own before the module under test is imported,
// the same pattern every db-touching test file in this tree follows.
const workspace = mkdtempSync(join(tmpdir(), "postage-passes-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";

const { all, reset } = await import("./client");
const { addPaidUse, grantPass, EARNED_PASS_CONDITION } = await import("./passes");

const HANDLE = "demo";
const SENDER = "farmer@x.com";

async function usesLeftOf(handle: string, sender: string): Promise<number | null> {
  const rows = await all<{ uses_left: number | null }>(
    `SELECT uses_left FROM passes WHERE handle = ? AND sender = ?`,
    [handle, sender]
  );
  return rows[0]?.uses_left ?? null;
}

async function matchesEarnedCondition(handle: string, sender: string): Promise<boolean> {
  const rows = await all(
    `SELECT 1 FROM passes WHERE handle = ? AND sender = ? AND ${EARNED_PASS_CONDITION}`,
    [handle, sender]
  );
  return rows.length > 0;
}

beforeEach(async () => {
  await reset();
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

test("a pass earned by proving personhood alone matches EARNED_PASS_CONDITION", async () => {
  await grantPass(HANDLE, SENDER, "human", null);

  assert.equal(await matchesEarnedCondition(HANDLE, SENDER), true);
});

test("a freshly bought pass never matches EARNED_PASS_CONDITION", async () => {
  await addPaidUse(HANDLE, SENDER);

  assert.equal(await matchesEarnedCondition(HANDLE, SENDER), false);
});

/// The case the whole charge exists to get right. `addPaidUse`'s
/// unlimited-window branch pays for a window that is already better than a
/// count by pushing `expires_at` out and nothing else, since there is no
/// count there to add a use to. That leaves `uses_left` NULL on a row money
/// has touched — indistinguishable from a window nobody ever paid for unless
/// `paid_extended_at` says otherwise.
test("a window extended by payment stops matching EARNED_PASS_CONDITION, though uses_left stays NULL", async () => {
  await grantPass(HANDLE, SENDER, "human", null);

  await addPaidUse(HANDLE, SENDER);

  assert.equal(await usesLeftOf(HANDLE, SENDER), null, "test setup: the window must still read as uncounted");
  assert.equal(await matchesEarnedCondition(HANDLE, SENDER), false);
});

/// `grantPass`'s own `ON CONFLICT` keeps a positive `uses_left` even while
/// overwriting `reason` to `"human"` on top of it, so a row that reads
/// `reason = 'human'` can still be a paid, unspent balance. If this test ever
/// fails, keying `EARNED_PASS_CONDITION` on `reason` instead of `uses_left`
/// is the most likely reason why.
test("a paid balance survives even under a pass whose reason now reads human", async () => {
  await grantPass(HANDLE, SENDER, "paid", 3);
  await grantPass(HANDLE, SENDER, "human", null);

  assert.equal(await usesLeftOf(HANDLE, SENDER), 3, "test setup: the paid count must have carried over");
  assert.equal(await matchesEarnedCondition(HANDLE, SENDER), false);
});

import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";
import { now } from "../time";

// A directory of its own per run, like every other db-touching test file here:
// a shared path plus `reset()` means two runs on one machine truncate each
// other's tables mid-test.
const workspace = mkdtempSync(join(tmpdir(), "postage-spent-nonces-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";

const { reset } = await import("./client");
const { purgeSpentWalletNonces, spendWalletNonce } = await import("./spent-nonces");

const LIVE_FOR = 120;

function nonce(n: number): string {
  return `${now() + LIVE_FOR}.${String(n).padStart(2, "0").repeat(16)}.${"cd".repeat(32)}`;
}

beforeEach(async () => {
  await reset();
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

test("a nonce nobody has answered yet is spendable", async () => {
  assert.equal(await spendWalletNonce(nonce(1), now() + LIVE_FOR), true);
});

/// The whole point of the table. The second presentation of one signature
/// carries the same nonce, and finds it gone.
test("the same nonce cannot be spent twice", async () => {
  const offered = nonce(1);

  assert.equal(await spendWalletNonce(offered, now() + LIVE_FOR), true);
  assert.equal(await spendWalletNonce(offered, now() + LIVE_FOR), false);
});

test("spending one nonce leaves another alone", async () => {
  await spendWalletNonce(nonce(1), now() + LIVE_FOR);

  assert.equal(await spendWalletNonce(nonce(2), now() + LIVE_FOR), true);
});

/// Only the rows past their window go. A purge that took a live one would
/// hand a captured signature a second life, which is the one thing this table
/// exists to refuse.
test("the purge drops nonces whose window has closed and keeps the rest", async () => {
  const stale = nonce(1);
  const live = nonce(2);
  await spendWalletNonce(stale, now() - 1);
  await spendWalletNonce(live, now() + LIVE_FOR);

  await purgeSpentWalletNonces();

  assert.equal(await spendWalletNonce(stale, now() + LIVE_FOR), true, "the stale row survived");
  assert.equal(await spendWalletNonce(live, now() + LIVE_FOR), false, "a live row was dropped");
});

/// Exactly the boundary, because `<=` and `<` differ by one second here and
/// the nonce is already refused on age by then either way.
test("a nonce expiring exactly now is dropped by the purge", async () => {
  const offered = nonce(1);
  await spendWalletNonce(offered, now());

  await purgeSpentWalletNonces();

  assert.equal(await spendWalletNonce(offered, now() + LIVE_FOR), true);
});

import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";
import { now } from "../time";

// A directory of its own per run, like every other db-touching test file here:
// a shared path plus `reset()` means two runs on one machine truncate each
// other's tables mid-test.
const workspace = mkdtempSync(join(tmpdir(), "postage-issued-contexts-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";

const { reset } = await import("./client");
const {
  MAX_LIVE_CONTEXTS_PER_TOKEN,
  consumeIssuedContext,
  purgeExpiredContexts,
  recordIssuedContext,
} = await import("./issued-contexts");

const TOKEN = "tok";
const LIVE_FOR = 300;

function nonce(n: number): string {
  return `0x${String(n).padStart(2, "0").repeat(32)}`;
}

async function issue(token: string, id: number, livesFor = LIVE_FOR): Promise<boolean> {
  const createdAt = now();
  return await recordIssuedContext(token, nonce(id), createdAt, createdAt + livesFor);
}

beforeEach(async () => {
  await reset();
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

test("caps how many live contexts one challenge token may hold at once", async () => {
  for (let issued = 0; issued < MAX_LIVE_CONTEXTS_PER_TOKEN; issued += 1) {
    assert.equal(await issue(TOKEN, issued), true, `context ${issued} should have been issued`);
  }

  assert.equal(await issue(TOKEN, MAX_LIVE_CONTEXTS_PER_TOKEN), false);
});

test("counts the cap per token, so one sender cannot close another's lane", async () => {
  for (let issued = 0; issued < MAX_LIVE_CONTEXTS_PER_TOKEN; issued += 1) {
    await issue(TOKEN, issued);
  }

  assert.equal(await recordIssuedContext("other", nonce(99), now(), now() + LIVE_FOR), true);
});

test("an expired context frees its slot, so an honest sender can always retry", async () => {
  const expired = now() - 10;
  for (let issued = 0; issued < MAX_LIVE_CONTEXTS_PER_TOKEN; issued += 1) {
    await recordIssuedContext(TOKEN, nonce(issued), expired - LIVE_FOR, expired);
  }

  assert.equal(await issue(TOKEN, MAX_LIVE_CONTEXTS_PER_TOKEN), true);
});

test("a nonce is spendable exactly once", async () => {
  await issue(TOKEN, 1);

  assert.equal(await consumeIssuedContext(TOKEN, nonce(1)), true);
  assert.equal(await consumeIssuedContext(TOKEN, nonce(1)), false);
});

test("refuses a nonce this route never issued", async () => {
  assert.equal(await consumeIssuedContext(TOKEN, nonce(1)), false);
});

test("refuses a nonce issued against a different challenge token", async () => {
  await issue("other", 1);

  assert.equal(await consumeIssuedContext(TOKEN, nonce(1)), false);
});

test("refuses a nonce whose signing window has already closed", async () => {
  const expired = now() - 1;
  await recordIssuedContext(TOKEN, nonce(1), expired - LIVE_FOR, expired);

  assert.equal(await consumeIssuedContext(TOKEN, nonce(1)), false);
});

test("purging drops closed windows and leaves open ones spendable", async () => {
  const expired = now() - 1;
  await recordIssuedContext(TOKEN, nonce(1), expired - LIVE_FOR, expired);
  await issue(TOKEN, 2);

  await purgeExpiredContexts();

  assert.equal(await consumeIssuedContext(TOKEN, nonce(2)), true);
});

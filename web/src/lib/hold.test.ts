import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AddressInfo } from "node:net";
import { after, beforeEach, test } from "node:test";
import { now } from "./time";

// A directory of its own per run. A shared path plus `reset()` means two runs
// on one machine truncate each other's tables mid-test and fail for reasons
// that have nothing to do with the code.
const workspace = mkdtempSync(join(tmpdir(), "postage-hold-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.MAIL_WEBHOOK_SECRET = "test";

/// Counting what the worker was actually asked to send is how a second release
/// is seen from outside. `releaseHeldMessage` reports the same "not delivered"
/// whether it declined to send or tried and failed, so the return value alone
/// cannot say which of the two happened.
let releases = 0;
const worker = createServer((_request, response) => {
  releases += 1;
  response.writeHead(200).end("{}");
});
// Listened for at the top level rather than in a `before()` hook: an async hook
// that resolves through an I/O callback runs after the first `beforeEach` on
// this runtime, and the first test would then read an address off a server that
// is not listening yet.
await new Promise<void>((listening) => worker.listen(0, "127.0.0.1", listening));
process.env.MAIL_WORKER_URL = `http://127.0.0.1:${(worker.address() as AddressInfo).port}`;

const { challengeByToken, createChallenge } = await import("./db/challenges");
const { all, reset, run } = await import("./db/client");
const { createInbox } = await import("./db/inboxes");
const { releaseHeldMessage } = await import("./hold");

const HANDLE = "demo";
const SENDER = "sender@x.com";

/// Old enough that no clock in the test can produce it, so a second write of
/// `delivered_at` is unmistakable rather than a same-second coincidence.
const AN_EARLIER_DELIVERY = 1_700_000_000;

async function held(token: string): Promise<void> {
  await createChallenge({
    token,
    handle: HANDLE,
    sender: SENDER,
    message_id: `0x${"ab".repeat(32)}`,
    tier: "commercial",
    amount: "1",
    quote_json: "{}",
    held_until: now() + 900,
    created_at: now(),
  });
}

beforeEach(async () => {
  await reset();
  releases = 0;
  await createInbox(HANDLE, "demo@example.com", `0x${"11".repeat(20)}`);
});

after(() => {
  worker.close();
  rmSync(workspace, { recursive: true, force: true });
});

/// Why `markDelivered` needs no `delivered_at IS NULL` guard, unlike the
/// `markEntitled` sitting next to it.
///
/// The sender's page polls `/api/challenge/resolve` until it stops erroring, so
/// asking to clear the same token repeatedly is the ordinary case, not a fault
/// path. What makes the unguarded write safe is one step upstream: recording a
/// delivery is only ever reached behind `claimHold`, and that is a single
/// conditional update turning a live hold into a released one, which nothing
/// turns back. Take that away and the timestamp of the delivery that actually
/// happened is overwritten by one that did not.
test("a held message releases once, so the moment it was delivered is written once", async () => {
  await held("once");

  assert.deepEqual(await releaseHeldMessage("once", HANDLE), { delivered: true });
  assert.notEqual((await challengeByToken("once"))?.delivered_at, null);

  // Reaches past the module for the same reason `gate.test.ts` does: nothing in
  // the app backdates a delivery, and only a value the clock cannot produce can
  // tell "left alone" apart from "written again a moment later".
  await run(`UPDATE challenges SET delivered_at = ? WHERE token = ?`, [AN_EARLIER_DELIVERY, "once"]);

  assert.deepEqual(await releaseHeldMessage("once", HANDLE), {
    delivered: false,
    reason_undelivered: "expired",
  });
  assert.equal(releases, 1, "the hold was already spent, so the worker must not be asked again");
  assert.equal(
    (await challengeByToken("once"))?.delivered_at,
    AN_EARLIER_DELIVERY,
    "the record has to keep the delivery that happened, not the moment of an attempt that sent nothing"
  );
});

/// Sequentially the loser sees a hold already cleared; at once, both read a live
/// one and the database decides. That is the case the guard would exist for, so
/// it is the one worth pinning.
test("two releases racing on one token send it once", async () => {
  await held("race");

  const outcomes = await Promise.all([
    releaseHeldMessage("race", HANDLE),
    releaseHeldMessage("race", HANDLE),
  ]);

  assert.equal(outcomes.filter((outcome) => outcome.delivered).length, 1);
  assert.equal(releases, 1, "one hold is one message, however many callers ask for it at once");

  const stamps = await all<{ delivered_at: number | null }>(
    `SELECT delivered_at FROM challenges WHERE token = ?`,
    ["race"]
  );
  assert.notEqual(stamps[0]?.delivered_at, null);
});

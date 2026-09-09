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
const workspace = mkdtempSync(join(tmpdir(), "postage-gate-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.MAIL_WORKER_URL = "http://127.0.0.1:9";
process.env.MAIL_WEBHOOK_SECRET = "test";

const { claimChallenge, createChallenge, markDelivered } = await import("./db/challenges");
const { all, reset, run } = await import("./db/client");
const { createInbox } = await import("./db/inboxes");
const { grantPass, hasLivePass, spendPass } = await import("./db/passes");
const { openGate } = await import("./gate");

const HANDLE = "demo";

async function seed(token: string, tier: string, sender: string) {
  await createChallenge({
    token,
    handle: HANDLE,
    sender,
    message_id: `0x${"ab".repeat(32)}`,
    tier,
    amount: "1",
    quote_json: "{}",
    held_until: now() + 900,
    created_at: now(),
  });
}

beforeEach(async () => {
  await reset();
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

/// Delivery always fails here: MAIL_WORKER_URL points at a closed port, which
/// is the interesting half. A message that goes out needs no entitlement; one
/// that does not is where every double-grant has come from.
test("a payment yields exactly one delivery, however often it is asked for", async () => {
  await seed("paid1", "commercial", "payer@x.com");

  const first = await openGate("paid1", "paid");
  assert.equal(first.status, "cleared");
  assert.equal(await hasLivePass(HANDLE, "payer@x.com"), true);

  await spendPass(HANDLE, "payer@x.com");
  assert.equal(await hasLivePass(HANDLE, "payer@x.com"), false);

  await openGate("paid1", "paid");
  assert.equal(
    await hasLivePass(HANDLE, "payer@x.com"),
    false,
    "one payment must not be redeemable again once its delivery was spent"
  );
});

/// The second proof has to answer a fresh challenge — the first token is
/// already settled once `openGate` returns, so replaying it would exercise
/// `recover()`, not the grant a second proof earns. A human pass never runs
/// down (`uses_left` stays null; see `spendPass` in `db/passes.ts`), so
/// `hasLivePass` alone cannot tell a working re-grant from one that did
/// nothing — the window's expiry is the only thing a second proof can move.
///
/// Wound down first, same reasoning as `paying while a free window is live`:
/// both grants can land in the same second, so leaving the window where the
/// first call put it would make "pushed back out to a full window" and "left
/// exactly where it was" the same number.
test("a proof re-earns a window, because presenting it again is the cost", async () => {
  await seed("hum1", "human", "person@x.com");
  await openGate("hum1", "human");

  const staleAt = now() + 60;
  await windowEndsAt("person@x.com", staleAt);

  await seed("hum2", "human", "person@x.com");
  await openGate("hum2", "human");

  assert.ok(
    (await passExpiry("person@x.com")) > staleAt,
    "a second proof was presented, so the window has to move back out past where it was left, not sit there"
  );
});

test("dangerous mail is delivered by no lane", async () => {
  await seed("bad1", "dangerous", "phish@x.com");

  for (const lane of ["human", "paid"] as const) {
    const result = await openGate("bad1", lane);
    assert.equal(result.status, "charged");
  }
  assert.equal(await hasLivePass(HANDLE, "phish@x.com"), false);
});

test("a delivered message grants nothing, so it cannot be sent twice", async () => {
  await seed("del1", "commercial", "done@x.com");
  await markDelivered("del1");

  await openGate("del1", "paid");
  assert.equal(await hasLivePass(HANDLE, "done@x.com"), false);
});

/// The one state `openGate` cannot reach by settling: whoever wins the claim
/// takes the other branch. It is what a winner leaves behind when it settles and
/// then fails before granting anything — the process that went away between the
/// two writes, the answer lost on the way back — so the sender is holding a
/// settled challenge and nothing to show for it, and answering again is the only
/// move they have left. Claiming it here without granting is that state exactly.
test("a sender holding nothing after a failed delivery is repaired", async () => {
  await seed("stuck", "commercial", "stuck@x.com");
  await claimChallenge("stuck", "human");

  await openGate("stuck", "paid");

  assert.equal(
    await hasLivePass(HANDLE, "stuck@x.com"),
    true,
    "the payment bought a delivery nobody made, so answering again has to hand it over"
  );
});

/// Releasing is an HTTP call to the mail worker in production, so the winner is
/// inside it for a long time while the loser is deciding what to do. A test that
/// lets the winner finish first proves nothing about the case that matters, so
/// this one holds the release open until both have made their decision.
test("clearing twice at once settles once, even while the release is slow", async () => {
  await createInbox(HANDLE, "demo@example.com", "0x" + "11".repeat(20));
  await seed("race1", "commercial", "race@x.com");

  const server = createServer((_request, response) => {
    setTimeout(() => {
      response.writeHead(200).end("{}");
    }, 300);
  });
  await new Promise<void>((ready) => server.listen(0, "127.0.0.1", ready));
  const { port } = server.address() as AddressInfo;
  process.env.MAIL_WORKER_URL = `http://127.0.0.1:${port}`;

  try {
    await Promise.all([openGate("race1", "paid"), openGate("race1", "paid")]);
  } finally {
    server.close();
    process.env.MAIL_WORKER_URL = "http://127.0.0.1:9";
  }

  assert.equal(
    await hasLivePass(HANDLE, "race@x.com"),
    false,
    "the message was delivered, so one payment must leave no spare use behind"
  );
});

test("an unknown token clears nothing", async () => {
  const result = await openGate("nope", "paid");
  assert.equal(result.status, "unknown");
});

/// `GateResult` only ever says whether the message went out, and the sender's
/// own page has nothing more useful to tell them either way — but "the inbox
/// vanished," "the hold had already expired" and "the worker could not be
/// reached" are three different bugs, and `hold.ts` already tells them apart.
/// Reading `.delivered` and dropping the rest would make that distinction
/// invisible the moment it matters, so this pins that it reaches a log instead.
test("a release that did not deliver logs which of the three reasons it was", async () => {
  await seed("unlogged", "commercial", "missing@x.com");
  // No `createInbox` for this handle, so `releaseHeldMessage` fails at the
  // first check it makes and returns the one reason that is reachable without
  // also faking a slow or broken mail worker.

  const logged: unknown[][] = [];
  const originalError = console.error;
  console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  try {
    await openGate("unlogged", "paid");
  } finally {
    console.error = originalError;
  }

  assert.ok(
    logged.some(
      ([message, context]) =>
        message === "held message not released" &&
        (context as { reason?: string })?.reason === "no_inbox"
    ),
    "the reason a release did not happen must reach the log, not stop at a discarded field"
  );
});

/// An unlimited window already outranks a single use, so paying during one
/// cannot add a use and must push the window out instead — otherwise the money
/// buys nothing and is gone the moment the proof behind the window lapses.
///
/// The window is wound down first because both grants land in the same second:
/// left as it is, "pushed out to fifteen minutes from now" and "left exactly
/// where it was" are the same number, and the assertion holds against a payment
/// that did nothing at all.
test("paying while a free window is live does not lose the payment", async () => {
  await seed("both", "commercial", "both@x.com");
  await grantPass(HANDLE, "both@x.com", "human", null);
  const windowEnds = now() + 60;
  await windowEndsAt("both@x.com", windowEnds);

  await openGate("both", "paid");

  assert.ok(
    (await passExpiry("both@x.com")) > windowEnds,
    "the delivery was paid for, so it has to outlast the free window it was paid during"
  );
});

/// Both reach past the module deliberately: nothing in the app winds a pass
/// down, nothing reads an expiry back, and a test that sat out a real window
/// would take a quarter of an hour to say what these say in a millisecond.
function windowEndsAt(sender: string, at: number): Promise<void> {
  return run(`UPDATE passes SET expires_at = ? WHERE handle = ? AND sender = ?`, [
    at,
    HANDLE,
    sender,
  ]);
}

async function passExpiry(sender: string): Promise<number> {
  const rows = await all<{ expires_at: number }>(
    `SELECT expires_at FROM passes WHERE handle = ? AND sender = ?`,
    [HANDLE, sender]
  );
  return Number(rows[0]?.expires_at ?? 0);
}

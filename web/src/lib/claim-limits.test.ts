import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";
import { createClient } from "@libsql/client";
import { now } from "./time";

const workspace = mkdtempSync(join(tmpdir(), "postage-claims-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";

const {
  CF_CHECK_BUDGET,
  CF_CHECK_INTERVAL_SECONDS,
  attachDestination,
  cloudflareChecksExhausted,
  markCloudflareVerified,
  recentClaimsFrom,
  recentClaimsTo,
  recordClaimSend,
  purgeOldClaimSends,
  startClaim,
  takeCloudflareCheck,
} = await import("./db/claims");

const WALLET = `0x${"11".repeat(20)}`;

async function claim(handle: string, destination = "someone@example.com", wallet = WALLET) {
  await startClaim({
    handle,
    destination,
    wallet,
    code_hash: "deadbeef",
    expires_at: now() + 900,
    cf_address_id: null,
    cf_verified_at: null,
  });
}

beforeEach(async () => {
  const { reset } = await import("./db/client");
  await reset();
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

/// The route that leads to this is polled by an anonymous browser and every
/// request used to spend one Cloudflare call against an account-wide quota.
test("a thousand pollers cost one Cloudflare call, not a thousand", async () => {
  await claim("demo");

  const taken = await Promise.all(
    Array.from({ length: 1000 }, () => takeCloudflareCheck("demo"))
  );
  assert.equal(
    taken.filter(Boolean).length,
    1,
    "concurrent pollers must not all read the same last-checked time and all go and ask"
  );
});

test("the ration is per claim, so one claim cannot starve another", async () => {
  await claim("demo");
  await claim("other", "someone-else@example.com");

  assert.equal(await takeCloudflareCheck("demo"), true);
  assert.equal(await takeCloudflareCheck("other"), true);
});

/// Rate alone only makes a loop slow. The budget is what makes it finite.
test("a claim stops answering once its budget is spent", async () => {
  await claim("demo");

  // Walked up to the last one rather than looped through all of them: the rule
  // under test is the boundary, and four hundred writes to reach it prove
  // nothing the two either side of it do not.
  await spend("demo", CF_CHECK_BUDGET - 1);
  await windBack("demo");
  assert.equal(await takeCloudflareCheck("demo"), true, "the last of the budget is still budget");

  await windBack("demo");
  assert.equal(await takeCloudflareCheck("demo"), false, "the budget is a ceiling, not a rate");
  assert.equal(await cloudflareChecksExhausted("demo"), true);
});

/// Registering the address is itself a question to Cloudflare, and `settleClaim`
/// runs moments later on both signup paths. Counting it stops every signup
/// spending two calls to learn one thing.
test("registering the address counts as having just asked", async () => {
  await claim("demo");
  await attachDestination("demo", "addr-1", null);

  assert.equal(
    await takeCloudflareCheck("demo"),
    false,
    "the answer is seconds old, so nothing should go and ask for it again"
  );
});

test("a claim Cloudflare has already confirmed is never asked about again", async () => {
  await claim("demo");
  await attachDestination("demo", "addr-1", null);
  await markCloudflareVerified("demo", "addr-1", now());

  assert.equal(await takeCloudflareCheck("demo"), false);
  assert.equal(
    await cloudflareChecksExhausted("demo"),
    false,
    "a confirmed claim is finished, not stalled"
  );
});

test("starting again gives the claim its budget back", async () => {
  await claim("demo");
  await spend("demo", CF_CHECK_BUDGET);
  assert.equal(await cloudflareChecksExhausted("demo"), true);

  await claim("demo");
  assert.equal(await takeCloudflareCheck("demo"), true);
});

/// The destination throttle counts who was mailed. It sees nothing at all when
/// one wallet names a fresh address every time — and every claim that reaches a
/// code registers a Cloudflare destination the account can never delete.
test("one wallet naming a new address each time is still counted", async () => {
  for (let n = 0; n < 4; n += 1) {
    await recordClaimSend(`fresh${n}@example.com`, WALLET);
    assert.equal(await recentClaimsTo(`fresh${n}@example.com`, 3600), 1);
  }

  assert.equal(await recentClaimsFrom(WALLET, 3600), 4);
});

test("one wallet's claims are not counted against another's", async () => {
  await recordClaimSend("a@example.com", WALLET);
  assert.equal(await recentClaimsFrom(`0x${"22".repeat(20)}`, 3600), 0);
});

test("the throttle ledger does not grow forever", async () => {
  await recordClaimSend("old@example.com", WALLET);
  await purgeOldClaimSends(-1);
  assert.equal(await recentClaimsFrom(WALLET, 3600), 0);
});

/// Both helpers reach past the module because they are moving a clock and a
/// counter the app has no reason to expose: nothing in it winds a claim back,
/// and a test that waited out the real interval would take a quarter of an hour.
async function sql(statement: string, args: unknown[]): Promise<void> {
  const client = createClient({ url: process.env.DATABASE_URL! });
  try {
    await client.execute({ sql: statement, args: args as never });
  } finally {
    client.close();
  }
}

function windBack(handle: string): Promise<void> {
  return sql(`UPDATE inbox_claims SET cf_checked_at = ? WHERE handle = ?`, [
    now() - CF_CHECK_INTERVAL_SECONDS - 1,
    handle,
  ]);
}

function spend(handle: string, checks: number): Promise<void> {
  return sql(`UPDATE inbox_claims SET cf_checks = ? WHERE handle = ?`, [checks, handle]);
}

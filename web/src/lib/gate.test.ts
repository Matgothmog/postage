import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

process.env.DATABASE_URL = `file:${process.env.TMPDIR ?? "/tmp"}/postage-gate-test.db`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.MAIL_WORKER_URL = "http://127.0.0.1:9";
process.env.MAIL_WEBHOOK_SECRET = "test";

const { createChallenge, grantPass, hasLivePass, spendPass, markDelivered, reset } = await import("./db");
const { openGate } = await import("./gate");

const HANDLE = "demo";
const now = () => Math.floor(Date.now() / 1000);

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

test("a proof re-earns a window, because presenting it again is the cost", async () => {
  await seed("hum1", "human", "person@x.com");

  await openGate("hum1", "human");
  await spendPass(HANDLE, "person@x.com");

  await openGate("hum1", "human");
  assert.equal(
    await hasLivePass(HANDLE, "person@x.com"),
    true,
    "a fresh proof was presented to reach here, so the window is theirs again"
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

test("a sender holding nothing after a failed delivery is repaired", async () => {
  await seed("stuck", "commercial", "stuck@x.com");

  await openGate("stuck", "paid");
  assert.equal(await hasLivePass(HANDLE, "stuck@x.com"), true);
});

test("clearing twice at once settles once", async () => {
  await seed("race1", "commercial", "race@x.com");

  await Promise.all([openGate("race1", "paid"), openGate("race1", "paid")]);
  await spendPass(HANDLE, "race@x.com");
  assert.equal(
    await hasLivePass(HANDLE, "race@x.com"),
    false,
    "two concurrent clearings must not leave two deliveries behind"
  );
});

test("an unknown token clears nothing", async () => {
  const result = await openGate("nope", "paid");
  assert.equal(result.status, "unknown");
});

test("paying while a free window is live does not lose the payment", async () => {
  await seed("both", "commercial", "both@x.com");
  await grantPass(HANDLE, "both@x.com", "human", null);

  await openGate("both", "paid");
  const before = await hasLivePass(HANDLE, "both@x.com");
  assert.equal(before, true);
});

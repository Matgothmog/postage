import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";

const workspace = mkdtempSync(join(tmpdir(), "postage-budget-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";

const {
  CLASSIFY_PER_HANDLE_HOURLY,
  CLASSIFY_PER_SENDER_HOURLY,
  claimClassification,
  releaseClassificationSlot,
  reset,
} = await import("./db");

beforeEach(async () => {
  await reset();
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

async function spend(times: number, sender: string) {
  for (let n = 0; n < times; n += 1) await claimClassification("demo", sender);
}

/// The distinction the whole degraded path rests on. A sender spending their
/// own slice chose to; a handle's pool is spent by whoever writes to that
/// inbox, forged addresses included. Collapsing the two either hands a sender a
/// way to switch off their own classification, or hands a stranger an hour of
/// leverage over someone else's mail.
test("a sender who spends their own slice is told it was theirs", async () => {
  await spend(CLASSIFY_PER_SENDER_HOURLY, "greedy@x.com");
  assert.equal(await claimClassification("demo", "greedy@x.com"), "spent-by-sender");
});

test("a handle drained by others is not blamed on the next sender", async () => {
  const senders = Math.ceil(CLASSIFY_PER_HANDLE_HOURLY / CLASSIFY_PER_SENDER_HOURLY);
  for (let n = 0; n < senders; n += 1) await spend(CLASSIFY_PER_SENDER_HOURLY, `flood${n}@x.com`);

  assert.equal(
    await claimClassification("demo", "innocent@x.com"),
    "spent-by-handle",
    "a stranger's flood must not read as this sender's own doing"
  );
});

test("room left reads as room left", async () => {
  assert.equal(await claimClassification("demo", "quiet@x.com"), null);
});

test("a slot handed back can be taken again", async () => {
  await spend(CLASSIFY_PER_SENDER_HOURLY, "retry@x.com");
  assert.equal(await claimClassification("demo", "retry@x.com"), "spent-by-sender");

  await releaseClassificationSlot("demo", "retry@x.com");
  assert.equal(
    await claimClassification("demo", "retry@x.com"),
    null,
    "work that never reached the model must not cost the sender their hour"
  );
});

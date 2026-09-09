import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";

const workspace = mkdtempSync(join(tmpdir(), "postage-budget-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";

const {
  CLASSIFY_PER_DOMAIN_HOURLY,
  CLASSIFY_PER_HANDLE_HOURLY,
  CLASSIFY_PER_SENDER_HOURLY,
  claimClassification,
  releaseClassificationSlot,
} = await import("./db/classifications");
const { reset } = await import("./db/client");

beforeEach(async () => {
  await reset();
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

async function spend(times: number, sender: string) {
  for (let n = 0; n < times; n += 1) await claimClassification("demo", sender);
}

/// One address per call, so the per-sender ceiling never binds before the
/// per-domain one the test is actually about.
async function spendAcrossDomain(times: number, domain: string) {
  for (let n = 0; n < times; n += 1) await claimClassification("demo", `writer${n}@${domain}`);
}

/// Empties a handle's whole hour without any single domain having done it,
/// which is the only way it can be emptied at all: each domain is filled to its
/// own ceiling and the next one takes over.
async function drainHandle(slots: number) {
  for (let taken = 0; taken < slots; taken += 1) {
    const domain = Math.floor(taken / CLASSIFY_PER_DOMAIN_HOURLY);
    await claimClassification("demo", `writer${taken}@drain${domain}.example`);
  }
}

/// The distinction the whole degraded path rests on. A sender spending their
/// own slice chose to; a handle's pool is spent by whoever writes to that
/// inbox. Collapsing the two either hands a sender a way to switch off their
/// own classification, or blames a recipient's own mail on them.
test("a sender who spends their own slice is told it was theirs", async () => {
  await spend(CLASSIFY_PER_SENDER_HOURLY, "greedy@x.com");
  assert.equal(await claimClassification("demo", "greedy@x.com"), "spent-by-sender");
});

test("a handle drained by many domains is not blamed on the next sender", async () => {
  await drainHandle(CLASSIFY_PER_HANDLE_HOURLY);

  assert.equal(
    await claimClassification("demo", "innocent@x.com"),
    "spent-by-handle",
    "a stranger's flood must not read as this sender's own doing"
  );
});

/// The ceiling that makes the outer one hard to reach. Only mail the receiving
/// server authenticated may spend any of this, and authentication is a
/// statement about a domain, so the domain is the smallest unit of a sender
/// that cannot be cycled for nothing: local parts are free to invent, a working
/// DKIM key is not. Without this, one domain could empty a handle's whole hour
/// on its own.
test("one domain cannot spend more than its own share of a handle's hour", async () => {
  await spendAcrossDomain(CLASSIFY_PER_DOMAIN_HOURLY, "flood.example");

  assert.equal(
    await claimClassification("demo", "another@flood.example"),
    "spent-by-domain",
    "a fresh local part at a spent domain must not open a fresh allowance"
  );
});

/// The property the redesign exists for: what one sender spends must not change
/// how anyone else is treated.
test("a domain that spent its share leaves the rest of the hour to everyone else", async () => {
  await spendAcrossDomain(CLASSIFY_PER_DOMAIN_HOURLY, "flood.example");

  assert.equal(
    await claimClassification("demo", "quiet@elsewhere.example"),
    null,
    "an unrelated domain must still be read"
  );
});

test("a sender who spends their own slice does not spend their neighbour's", async () => {
  await spend(CLASSIFY_PER_SENDER_HOURLY, "greedy@shared.example");

  assert.equal(await claimClassification("demo", "neighbour@shared.example"), null);
});

/// Guards the suffix the per-domain count is matched on. Counting every address
/// that merely ends the same way would let anyone drain `example.com`'s
/// allowance from `notexample.com`, which costs one registration.
test("a domain's ceiling counts that domain, not one whose name ends the same way", async () => {
  await spendAcrossDomain(CLASSIFY_PER_DOMAIN_HOURLY, "example.com");

  assert.equal(await claimClassification("demo", "someone@notexample.com"), null);
});

/// A subdomain is a separate name with a separate key, and is counted as one.
test("a subdomain does not draw on the allowance of the domain below it", async () => {
  await spendAcrossDomain(CLASSIFY_PER_DOMAIN_HOURLY, "example.com");

  assert.equal(await claimClassification("demo", "someone@mail.example.com"), null);
});

test("the last slot inside a sender's own limit is still granted", async () => {
  await spend(CLASSIFY_PER_SENDER_HOURLY - 1, "chatty@x.com");

  assert.equal(await claimClassification("demo", "chatty@x.com"), null, "the limit is inclusive");
  assert.equal(await claimClassification("demo", "chatty@x.com"), "spent-by-sender");
});

test("the last slot inside a domain's limit is still granted", async () => {
  await spendAcrossDomain(CLASSIFY_PER_DOMAIN_HOURLY - 1, "busy.example");

  assert.equal(await claimClassification("demo", "last@busy.example"), null, "the limit is inclusive");
  assert.equal(await claimClassification("demo", "over@busy.example"), "spent-by-domain");
});

test("the last slot inside a handle's pool is still granted", async () => {
  await drainHandle(CLASSIFY_PER_HANDLE_HOURLY - 1);

  assert.equal(await claimClassification("demo", "last@fresh.example"), null, "the limit is inclusive");
  assert.equal(await claimClassification("demo", "over@later.example"), "spent-by-handle");
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

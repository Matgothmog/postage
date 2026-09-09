import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";
import type { Hex } from "viem";
import type { Verdict } from "@/lib/classify";
import type { Tier } from "@/lib/tiers";
import { startStubChain } from "../../../../../test/chain";

const chain = await startStubChain();

const workspace = mkdtempSync(join(tmpdir(), "postage-challenge-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.MESSAGE_ID_SECRET = "x".repeat(32);
process.env.CLASSIFIER_PRIVATE_KEY = `0x${"11".repeat(32)}`;

const { HOLD_SECONDS, challengeByToken } = await import("@/lib/db/challenges");
const { reset } = await import("@/lib/db/client");
const { issueChallenge } = await import("./challenge");

const HANDLE = "demo";
/// Deliberately left unlinked to any wallet. A sender with one is priced on
/// what The Graph knows about them, which is a query this test would be making
/// over the network to answer.
const SENDER = "sender@x.com";
const WALLET = `0x${"11".repeat(20)}` as Hex;
const APP_URL = "http://localhost";

function issue(tier: Tier, degraded = false) {
  const verdict: Verdict = { tier, confidence: 0.9, reasons: [], degraded };
  return issueChallenge({
    handle: HANDLE,
    sender: SENDER,
    subject: "hello",
    verdict,
    wallet: WALLET,
    appUrl: APP_URL,
  });
}

beforeEach(async () => {
  await reset();
});

after(async () => {
  await chain.close();
  rmSync(workspace, { recursive: true, force: true });
});

/// The escrow reverts on anything below the floor it holds, so a price that came
/// from anywhere but the chain is a quote nobody can pay.
test("a stranger's message is priced from the floor the chain reports", async () => {
  const issued = await issue("commercial");

  assert.equal(issued.price, chain.floor);
});

test("the challenge a sender is sent to answer is recorded against its token", async () => {
  const issued = await issue("commercial");

  const stored = await challengeByToken(issued.token);
  assert.equal(stored?.handle, HANDLE);
  assert.equal(stored?.sender, SENDER);
  assert.equal(stored?.tier, "commercial");
  assert.equal(stored?.amount, issued.price.toString());
});

/// Dangerous mail is never delivered by any route, so there is nothing to hold
/// and no reason to keep what it said.
test("dangerous mail is recorded without holding anything", async () => {
  const issued = await issue("dangerous");

  assert.equal(issued.heldUntil, null);
  assert.equal((await challengeByToken(issued.token))?.held_until, null);
});

test("a held message is kept for as long as a hold lasts", async () => {
  const issued = await issue("commercial");

  const stored = await challengeByToken(issued.token);
  assert.equal(issued.heldUntil, stored!.created_at + HOLD_SECONDS);
  assert.equal(stored?.held_until, issued.heldUntil);
});

/// The quote is what the escrow checks, so it has to name the inbox being paid
/// and the price it was signed for rather than whatever a payer sends back.
test("the quote is signed for the inbox that gets paid, at the price that was quoted", async () => {
  const issued = await issue("commercial");

  assert.equal(issued.quote.inbox, WALLET);
  assert.equal(issued.quote.tier, "commercial");
  assert.equal(issued.quote.amount, issued.price.toString());
});

test("the sender is sent to the page their own token names", async () => {
  const issued = await issue("commercial");

  assert.equal(issued.challengeUrl, `${APP_URL}/c/${issued.token}`);
});

/// Kept with the quote because the challenge page explains the price from the
/// stored row - the message it was quoted for is gone by then.
test("the reasons behind a price are stored with the quote", async () => {
  const issued = await issue("commercial");

  const stored = await challengeByToken(issued.token);
  const quoted = JSON.parse(stored!.quote_json) as { reasons: string[]; signature: string };
  assert.deepEqual(quoted.reasons, issued.reasons);
  assert.equal(quoted.signature, issued.quote.signature);
});

import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import type { Hex, VerifyMessageParameters } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { provesWallet, readStatement } from "./auth";
import { publicClient } from "./client";
import { now } from "./time";

/// Signatures here are real. `provesWallet` recovers the signer, so a stubbed
/// verdict would only assert that the stub was consulted - which is the one
/// thing this file exists to rule out.
const holder = privateKeyToAccount(`0x${"11".repeat(32)}`);
const impostor = privateKeyToAccount(`0x${"22".repeat(32)}`);

const WALLET = holder.address;
const FRESHNESS_SECONDS = 5 * 60;

const statement = (issuedAt: number): string => readStatement(WALLET, issuedAt);

const sign = (issuedAt: number, by = holder): Promise<Hex> =>
  by.signMessage({ message: statement(issuedAt) });

/// Both edges of the window are one exact second, and `provesWallet` reads the
/// clock after the test has picked a timestamp for it. Left running, "issued
/// exactly five minutes ago" becomes "five minutes and one second" whenever the
/// second turns between the two, and the boundary cases fail for the calendar.
const FROZEN_MS = Date.UTC(2026, 8, 8, 12, 0, 0);

const realDateNow = Date.now;
const realVerifyMessage = publicClient.verifyMessage;

let asked: VerifyMessageParameters[] = [];

/// The RPC every test runs against is a hostile one: it says yes to anything,
/// and records that it was asked. Nothing below may return true because of it,
/// and `afterEach` proves nothing even reached it - an operator's endpoint, or
/// the public default `http()` falls back to when `ARC_RPC_URL` is unset, is not
/// allowed a say in who holds which wallet.
beforeEach(() => {
  Date.now = () => FROZEN_MS;
  asked = [];
  publicClient.verifyMessage = async (parameters) => {
    asked.push(parameters);
    return true;
  };
});

afterEach(() => {
  Date.now = realDateNow;
  publicClient.verifyMessage = realVerifyMessage;

  assert.deepEqual(asked, [], "a signature was settled by the chain rather than locally");
});

test("a statement signed this second proves the wallet", async () => {
  const issuedAt = now();

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement), true);
});

test("a statement signed exactly five minutes ago still proves the wallet", async () => {
  const issuedAt = now() - FRESHNESS_SECONDS;

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement), true);
});

test("a statement signed five minutes and one second ago proves nothing", async () => {
  const issuedAt = now() - FRESHNESS_SECONDS - 1;

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement), false);
});

/// The window opens forwards as well as back, because the timestamp is the
/// signer's and their clock is not ours.
test("a statement dated exactly five minutes ahead is allowed for a clock that runs fast", async () => {
  const issuedAt = now() + FRESHNESS_SECONDS;

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement), true);
});

test("a statement dated five minutes and one second ahead proves nothing", async () => {
  const issuedAt = now() + FRESHNESS_SECONDS + 1;

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement), false);
});

/// The guard that is not redundant. `NaN` fails both halves of the window
/// comparison - `NaN < -300` and `NaN > 300` are each false - so without
/// `Number.isFinite` a timestamp of `NaN` is not stale but timeless, and one
/// signature over the statement it produces would be replayable forever.
test("a timestamp of NaN is stale rather than timeless", async () => {
  const age = now() - Number.NaN;
  assert.ok(
    !(age < -FRESHNESS_SECONDS) && !(age > FRESHNESS_SECONDS),
    "precondition: NaN sits outside neither edge, so only the finite check can refuse it"
  );

  assert.equal(await provesWallet(WALLET, Number.NaN, await sign(Number.NaN), statement), false);
});

/// `holdsWallet` hands this straight to `Number()`, so what arrives here is
/// whatever a header a caller controls converts to.
test("a header that is not a number at all proves nothing", async () => {
  const issuedAt = Number("whenever");

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement), false);
});

test("a header that was never sent proves nothing", async () => {
  const issuedAt = Number(null);

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement), false);
});

test("an infinite timestamp proves nothing, in either direction", async () => {
  const ahead = await sign(Number.POSITIVE_INFINITY);
  const behind = await sign(Number.NEGATIVE_INFINITY);

  assert.equal(await provesWallet(WALLET, Number.POSITIVE_INFINITY, ahead, statement), false);
  assert.equal(await provesWallet(WALLET, Number.NEGATIVE_INFINITY, behind, statement), false);
});

test("a request naming no wallet proves nothing", async () => {
  assert.equal(await provesWallet(null, now(), await sign(now()), statement), false);
});

test("a wallet that is not an address proves nothing", async () => {
  assert.equal(await provesWallet("0xnot-an-address", now(), await sign(now()), statement), false);
});

test("a request carrying no signature proves nothing", async () => {
  assert.equal(await provesWallet(WALLET, now(), null, statement), false);
});

/// The forgery an attacker can actually mount: they hold a key, just not this
/// one. Recovery lands on their address instead, and no amount of agreement
/// from the RPC standing behind this test can turn that into a proof.
test("a signature from another wallet proves nothing, whatever the RPC says", async () => {
  const issuedAt = now();

  const forged = await sign(issuedAt, impostor);

  assert.equal(await provesWallet(WALLET, issuedAt, forged, statement), false);
});

/// A signature that recovers to nobody at all - here a `v` that names no
/// recovery id - makes viem throw rather than return. That is a refusal, and it
/// may not escape as a 500 or, worse, fall back to asking the chain.
test("a malformed signature proves nothing rather than escaping", async () => {
  const nonsense = `0x${"ab".repeat(65)}` as const;

  assert.equal(await provesWallet(WALLET, now(), nonsense, statement), false);
});

/// The statement carries the timestamp, which is what stops a signature
/// collected once from proving anything at a second moment inside the window.
test("a signature over one timestamp proves nothing about another", async () => {
  const signedAt = now() - 120;
  const replayedAt = now() - 60;
  assert.ok(Math.abs(now() - signedAt) < FRESHNESS_SECONDS, "precondition: both are fresh");

  const collected = await sign(signedAt);

  assert.equal(await provesWallet(WALLET, signedAt, collected, statement), true);
  assert.equal(await provesWallet(WALLET, replayedAt, collected, statement), false);
});

/// A wallet address survives a round trip through a lowercasing header or
/// statement, so the checksummed form is not the only one that arrives here.
test("a wallet named in lowercase proves the same wallet", async () => {
  const issuedAt = now();

  assert.equal(
    await provesWallet(WALLET.toLowerCase(), issuedAt, await sign(issuedAt), statement),
    true
  );
});

/// Nothing downstream should have to pay for a stale request, and nothing
/// upstream gets a say in a fresh one.
test("no request is ever put to the chain, stale or good", async () => {
  await provesWallet(WALLET, now() - FRESHNESS_SECONDS - 1, await sign(now()), statement);
  await provesWallet(WALLET, now(), await sign(now()), statement);

  assert.equal(asked.length, 0);
});

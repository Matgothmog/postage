import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, afterEach, beforeEach, test } from "node:test";
import type { Hex, VerifyMessageParameters } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { publicClient } from "./client";
import { now } from "./time";

// `provesWallet` records a spent nonce, so this file now touches a database.
// A directory of its own per run, like every other db-touching test file here,
// and both set before the modules below are imported: the client reads
// `DATABASE_URL` when it first opens a connection and keeps what it opened.
const workspace = mkdtempSync(join(tmpdir(), "postage-auth-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.MESSAGE_ID_SECRET = "x".repeat(32);

const { reset } = await import("./db/client");
const { provesWallet, readStatement } = await import("./auth");
const { WALLET_NONCE_TTL_SECONDS, mintWalletNonce } = await import("./wallet-nonce");
const { withNonce } = await import("./wallet-proof");

/// Signatures here are real. `provesWallet` recovers the signer, so a stubbed
/// verdict would only assert that the stub was consulted - which is the one
/// thing this file exists to rule out.
const holder = privateKeyToAccount(`0x${"11".repeat(32)}`);
const impostor = privateKeyToAccount(`0x${"22".repeat(32)}`);

const WALLET = holder.address;
/// Mirrors `auth.ts`'s `CLOCK_SKEW_TOLERANCE_SECONDS` — the tolerance applied
/// on each side of "now", not the width of the window itself.
const CLOCK_SKEW_TOLERANCE_SECONDS = 5 * 60;

const statement = (issuedAt: number): string => readStatement(WALLET, issuedAt);

/// One fresh nonce per test, minted under the frozen clock below so it is
/// always live for the whole of that test. Every `sign` and every
/// `provesWallet` in a test shares it, which is what makes the calls a
/// matching pair — and, in the one test that presents a signature twice, what
/// the second presentation is refused for.
let nonce: string;

const sign = (issuedAt: number, by = holder): Promise<Hex> =>
  by.signMessage({ message: withNonce(statement(issuedAt), nonce) });

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
beforeEach(async () => {
  Date.now = () => FROZEN_MS;
  await reset();
  nonce = mintWalletNonce(WALLET);
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

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement, nonce), true);
});

test("a statement signed exactly five minutes ago still proves the wallet", async () => {
  const issuedAt = now() - CLOCK_SKEW_TOLERANCE_SECONDS;

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement, nonce), true);
});

test("a statement signed five minutes and one second ago proves nothing", async () => {
  const issuedAt = now() - CLOCK_SKEW_TOLERANCE_SECONDS - 1;

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement, nonce), false);
});

/// The window opens forwards as well as back, because the timestamp is the
/// signer's and their clock is not ours.
test("a statement dated exactly five minutes ahead is allowed for a clock that runs fast", async () => {
  const issuedAt = now() + CLOCK_SKEW_TOLERANCE_SECONDS;

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement, nonce), true);
});

test("a statement dated five minutes and one second ahead proves nothing", async () => {
  const issuedAt = now() + CLOCK_SKEW_TOLERANCE_SECONDS + 1;

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement, nonce), false);
});

/// The guard that is not redundant. `NaN` fails both halves of the window
/// comparison - `NaN < -300` and `NaN > 300` are each false - so without
/// `Number.isFinite` a timestamp of `NaN` is not stale but timeless, and one
/// signature over the statement it produces would be replayable forever.
test("a timestamp of NaN is stale rather than timeless", async () => {
  const age = now() - Number.NaN;
  assert.ok(
    !(age < -CLOCK_SKEW_TOLERANCE_SECONDS) && !(age > CLOCK_SKEW_TOLERANCE_SECONDS),
    "precondition: NaN sits outside neither edge, so only the finite check can refuse it"
  );

  assert.equal(await provesWallet(WALLET, Number.NaN, await sign(Number.NaN), statement, nonce), false);
});

/// `holdsWallet` hands this straight to `Number()`, so what arrives here is
/// whatever a header a caller controls converts to.
test("a header that is not a number at all proves nothing", async () => {
  const issuedAt = Number("whenever");

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement, nonce), false);
});

test("a header that was never sent proves nothing", async () => {
  const issuedAt = Number(null);

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement, nonce), false);
});

test("an infinite timestamp proves nothing, in either direction", async () => {
  const ahead = await sign(Number.POSITIVE_INFINITY);
  const behind = await sign(Number.NEGATIVE_INFINITY);

  assert.equal(await provesWallet(WALLET, Number.POSITIVE_INFINITY, ahead, statement, nonce), false);
  assert.equal(await provesWallet(WALLET, Number.NEGATIVE_INFINITY, behind, statement, nonce), false);
});

test("a request naming no wallet proves nothing", async () => {
  assert.equal(await provesWallet(null, now(), await sign(now()), statement, nonce), false);
});

test("a wallet that is not an address proves nothing", async () => {
  assert.equal(
    await provesWallet("0xnot-an-address", now(), await sign(now()), statement, nonce),
    false
  );
});

test("a request carrying no signature proves nothing", async () => {
  assert.equal(await provesWallet(WALLET, now(), null, statement, nonce), false);
});

/// The forgery an attacker can actually mount: they hold a key, just not this
/// one. Recovery lands on their address instead, and no amount of agreement
/// from the RPC standing behind this test can turn that into a proof.
test("a signature from another wallet proves nothing, whatever the RPC says", async () => {
  const issuedAt = now();

  const forged = await sign(issuedAt, impostor);

  assert.equal(await provesWallet(WALLET, issuedAt, forged, statement, nonce), false);
});

/// A signature that recovers to nobody at all - here a `v` that names no
/// recovery id - makes viem throw rather than return. That is a refusal, and it
/// may not escape as a 500 or, worse, fall back to asking the chain.
test("a malformed signature proves nothing rather than escaping", async () => {
  const nonsense = `0x${"ab".repeat(65)}` as const;

  assert.equal(await provesWallet(WALLET, now(), nonsense, statement, nonce), false);
});

/// The statement carries the timestamp, which is what stops a signature
/// collected once from proving anything at a second moment inside the window.
test("a signature over one timestamp proves nothing about another", async () => {
  const signedAt = now() - 120;
  const replayedAt = now() - 60;
  assert.ok(Math.abs(now() - signedAt) < CLOCK_SKEW_TOLERANCE_SECONDS, "precondition: both are fresh");

  assert.equal(await provesWallet(WALLET, signedAt, await sign(signedAt), statement, nonce), true);

  // A second nonce, and a second signature collected under it at the same
  // `signedAt`. Without this the refusal below would be the spent-nonce
  // record's doing — true, but not what this test is about — and the
  // timestamp binding could rot away underneath a test that still passed.
  const fresh = mintWalletNonce(WALLET);
  const collected = await holder.signMessage({ message: withNonce(statement(signedAt), fresh) });

  assert.equal(await provesWallet(WALLET, replayedAt, collected, statement, fresh), false);
});

/// A wallet address survives a round trip through a lowercasing header or
/// statement, so the checksummed form is not the only one that arrives here.
test("a wallet named in lowercase proves the same wallet", async () => {
  const issuedAt = now();

  assert.equal(
    await provesWallet(WALLET.toLowerCase(), issuedAt, await sign(issuedAt), statement, nonce),
    true
  );
});

/// Nothing downstream should have to pay for a stale request, and nothing
/// upstream gets a say in a fresh one.
test("no request is ever put to the chain, stale or good", async () => {
  const stale = now() - CLOCK_SKEW_TOLERANCE_SECONDS - 1;
  await provesWallet(WALLET, stale, await sign(now()), statement, nonce);
  await provesWallet(WALLET, now(), await sign(now()), statement, nonce);

  assert.equal(asked.length, 0);
});

/// The replay this whole mechanism exists to stop, and the one thing a
/// timestamp inside the statement could never do on its own: the signature is
/// byte-for-byte the one that just worked, presented at the same instant, over
/// the same text, by the same wallet. Only a record of what has already been
/// answered can tell the two apart.
test("a proof that verified once proves nothing the second time", async () => {
  const issuedAt = now();
  const collected = await sign(issuedAt);

  assert.equal(await provesWallet(WALLET, issuedAt, collected, statement, nonce), true);
  assert.equal(await provesWallet(WALLET, issuedAt, collected, statement, nonce), false);
});

test("a request carrying no nonce at all proves nothing", async () => {
  const issuedAt = now();

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement, null), false);
});

/// A nonce nobody minted. The signature over it is genuine — the wallet really
/// did sign this text — which is exactly the case a reader that only checked
/// signatures would wave through.
test("a forged nonce proves nothing, however real the signature over it is", async () => {
  const forged = `${now() + 120}.${"ab".repeat(16)}.${"cd".repeat(32)}`;
  const issuedAt = now();
  const collected = await holder.signMessage({ message: withNonce(statement(issuedAt), forged) });

  assert.equal(await provesWallet(WALLET, issuedAt, collected, statement, forged), false);
});

/// The nonce window is two minutes and the skew tolerance is five, so this is
/// a request the timestamp check would still admit. It is the nonce that
/// refuses it — and the gap between the two numbers is what makes this test
/// able to say so.
test("a nonce past its own window proves nothing, inside the skew tolerance or not", async () => {
  const issuedAt = now();
  const collected = await sign(issuedAt);
  assert.ok(WALLET_NONCE_TTL_SECONDS < CLOCK_SKEW_TOLERANCE_SECONDS, "precondition: there is a gap");

  Date.now = () => FROZEN_MS + (WALLET_NONCE_TTL_SECONDS + 1) * 1000;

  assert.equal(await provesWallet(WALLET, issuedAt, collected, statement, nonce), false);
});

/// One session's nonce cannot be spent by another wallet, so a browser that
/// holds a nonce cannot hand it to a page signing for somebody else's address.
test("a nonce minted for another wallet proves nothing here", async () => {
  const elsewhere = mintWalletNonce(impostor.address);
  const issuedAt = now();
  const collected = await holder.signMessage({
    message: withNonce(statement(issuedAt), elsewhere),
  });

  assert.equal(await provesWallet(WALLET, issuedAt, collected, statement, elsewhere), false);
});

/// A signature over the bare statement, with the nonce presented alongside but
/// not inside what was signed. This is what a client half-migrated to the new
/// contract would send, and it must be refused rather than quietly accepted —
/// the nonce is worth nothing unless the wallet actually committed to it.
test("a signature that does not cover the nonce proves nothing", async () => {
  const issuedAt = now();
  const withoutNonce = await holder.signMessage({ message: statement(issuedAt) });

  assert.equal(await provesWallet(WALLET, issuedAt, withoutNonce, statement, nonce), false);
});

/// Refusing a bad signature must not cost the nonce it was offered under, or
/// anyone could burn a stranger's sign-in attempt by posting rubbish under a
/// nonce they watched go past.
test("a nonce survives a signature that fails to verify", async () => {
  const issuedAt = now();

  const forged = await sign(issuedAt, impostor);
  assert.equal(await provesWallet(WALLET, issuedAt, forged, statement, nonce), false);

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt), statement, nonce), true);
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

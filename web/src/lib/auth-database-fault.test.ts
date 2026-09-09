import assert from "node:assert/strict";
import { after, afterEach, beforeEach, test } from "node:test";
import type { Hex, VerifyMessageParameters } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { startBrokenDatabase } from "../../test/database";
import { publicClient } from "./client";
import { now } from "./time";

/// A file of its own, like `api/mail/inbound/database-fault.test.ts`, because
/// pointing `DATABASE_URL` at a database that refuses every query is a
/// process-wide decision: the client reads it once, when it first opens a
/// connection, and keeps what it opened.
const broken = await startBrokenDatabase();

process.env.MESSAGE_ID_SECRET = "x".repeat(32);

const { provesWallet, readStatement } = await import("./auth");
const { mintWalletNonce } = await import("./wallet-nonce");
const { withNonce } = await import("./wallet-proof");

const holder = privateKeyToAccount(`0x${"11".repeat(32)}`);
const WALLET = holder.address;

const statement = (issuedAt: number): string => readStatement(WALLET, issuedAt);

const sign = (issuedAt: number, nonce: string): Promise<Hex> =>
  holder.signMessage({ message: withNonce(statement(issuedAt), nonce) });

const realVerifyMessage = publicClient.verifyMessage;
const realConsoleError = console.error;

let asked: VerifyMessageParameters[] = [];
let logged: unknown[][] = [];

beforeEach(() => {
  asked = [];
  logged = [];
  publicClient.verifyMessage = async (parameters) => {
    asked.push(parameters);
    return true;
  };
  // Captured rather than silenced: one test below is about what it says.
  console.error = (...args: unknown[]) => {
    logged.push(args);
  };
});

afterEach(() => {
  publicClient.verifyMessage = realVerifyMessage;
  console.error = realConsoleError;

  assert.deepEqual(asked, [], "a signature was settled by the chain rather than locally");
});

after(async () => {
  await broken.close();
});

/// The whole of the claim: a proof that is *good in every other way* — real
/// key, real wallet, real nonce, fresh timestamp — is still refused when the
/// ledger that would record it cannot be reached.
///
/// Not knowing whether this nonce has already been answered is not a reason to
/// let it through. The alternative is an outage that quietly turns every
/// signature back into a replayable bearer credential, at exactly the moment
/// nobody is watching.
test("a proof good in every other way is refused when the nonce ledger is unreachable", async () => {
  const nonce = mintWalletNonce(WALLET);
  const issuedAt = now();

  assert.equal(await provesWallet(WALLET, issuedAt, await sign(issuedAt, nonce), statement, nonce), false);
});

/// A database that will not answer is a refusal, not a 500 escaping through a
/// function whose whole contract is a boolean.
test("an unreachable ledger is answered rather than thrown", async () => {
  const nonce = mintWalletNonce(WALLET);
  const issuedAt = now();
  const signature = await sign(issuedAt, nonce);

  await assert.doesNotReject(() => provesWallet(WALLET, issuedAt, signature, statement, nonce));
});

/// An operator has to be able to tell a database outage from a wrong
/// signature, because the two need completely different responses and the
/// caller is told the same thing either way.
test("the refusal is explained in the log rather than passed off as a bad signature", async () => {
  const nonce = mintWalletNonce(WALLET);
  const issuedAt = now();

  await provesWallet(WALLET, issuedAt, await sign(issuedAt, nonce), statement, nonce);

  assert.equal(logged.length, 1);
  assert.match(String(logged[0]?.[0]), /ledger unavailable/);
});

/// The nonce is still live at this point — the row that would spend it is
/// exactly what failed to land — so writing it to a log would hand anyone with
/// log access a credential the ledger has no record of. The connection string
/// and its token are the same class of thing, and this is the path that holds
/// both at once.
test("nothing worth stealing reaches the log", async () => {
  const nonce = mintWalletNonce(WALLET);
  const issuedAt = now();

  await provesWallet(WALLET, issuedAt, await sign(issuedAt, nonce), statement, nonce);

  const line = JSON.stringify(logged);
  assert.ok(!line.includes(nonce), "the log quotes the nonce back");
  assert.ok(!line.includes(broken.authToken), "the log quotes the database credential back");
  assert.ok(!line.includes(process.env.MESSAGE_ID_SECRET as string), "the log quotes the secret back");
});

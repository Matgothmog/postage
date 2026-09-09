import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, afterEach, beforeEach, test } from "node:test";
import type { PrivateKeyAccount, VerifyMessageParameters } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { publicClient } from "./client";
import {
  IDENTITY_TOKEN_HEADER,
  ISSUED_AT_HEADER,
  NONCE_HEADER,
  SIGNATURE_HEADER,
  WALLET_HEADER,
  WALLET_NONCE_PATH,
  confirmStatement,
  readProof,
  readStatement,
  signedProof,
} from "./wallet-proof";

// The reader now records a spent nonce, so this round trip touches a database.
// A directory of its own per run, like every other db-touching test file here,
// and set before the modules below are imported: the client reads
// `DATABASE_URL` when it first opens a connection and keeps what it opened.
const workspace = mkdtempSync(join(tmpdir(), "postage-wallet-proof-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.MESSAGE_ID_SECRET = "x".repeat(32);

const { reset } = await import("./db/client");
const { holdsWallet } = await import("./auth");
const { mintWalletNonce } = await import("./wallet-nonce");
/// Reaches across into the app on purpose. The contract only means anything if
/// the browser's writer and the route's reader are the same two ends of it, so
/// the round trip below has to hold both at once rather than a stand-in for
/// either.
const { walletProof } = await import("@/app/claim-inbox-helpers");
type SignMessage = Parameters<typeof walletProof>[1];

const holder = privateKeyToAccount(`0x${"11".repeat(32)}`);
const impostor = privateKeyToAccount(`0x${"22".repeat(32)}`);

const WALLET = holder.address;
const INBOX = "https://postage.test/api/inbox";

const statement = (issuedAt: number): string => readStatement(WALLET, issuedAt);

/// Privy's hook, minus Privy: `walletProof` only ever calls what it is handed,
/// so a real key standing in for the wallet makes the signature a real one and
/// the round trip an honest one.
const signAs =
  (account: PrivateKeyAccount): SignMessage =>
  async ({ message }) => ({ signature: await account.signMessage({ message }) });

const realVerifyMessage = publicClient.verifyMessage;

let asked: VerifyMessageParameters[] = [];

/// The same hostile RPC `auth.test.ts` runs against - it says yes to anything -
/// and here for one more reason than there. The round trip below asserts that a
/// proof is *accepted*, which an endpoint answering yes to everything would
/// satisfy however badly the two ends of the contract disagreed. `afterEach`
/// proves nothing reached it, so the acceptance came from recovering the signer
/// against the statement the writer actually signed.
/// The real `/api/wallet-nonce`, minus the route: the writer fetches a nonce
/// before it prompts a wallet, and it is the server's own minting that makes
/// the value one the reader will accept. Stubbing the *transport* rather than
/// the nonce keeps the round trip honest — what the reader verifies below is a
/// genuine MAC over a genuine wallet, not a value both ends agreed to trust.
///
/// Every other URL throws, so a stray request going anywhere else fails the
/// test rather than silently reaching the network.
const realFetch = globalThis.fetch;

beforeEach(async () => {
  await reset();
  asked = [];
  publicClient.verifyMessage = async (parameters) => {
    asked.push(parameters);
    return true;
  };
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    if (String(input) !== WALLET_NONCE_PATH) throw new Error(`unexpected fetch to ${input}`);
    const { wallet } = JSON.parse(String(init?.body)) as { wallet: string };
    return Response.json({ nonce: mintWalletNonce(wallet) });
  }) as typeof fetch;
});

afterEach(() => {
  publicClient.verifyMessage = realVerifyMessage;
  globalThis.fetch = realFetch;

  assert.deepEqual(asked, [], "a signature was settled by the chain rather than locally");
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

/// The only place these strings are written by hand a second time. Everywhere
/// else imports them, which is the point - and is also why a rename would agree
/// with itself across the whole repo while silently changing the wire under a
/// browser tab that is still open, or under a route deployed a minute apart.
/// This is the pin that makes that a failing test instead.
test("the header names are the ones already on the wire", () => {
  assert.equal(IDENTITY_TOKEN_HEADER, "privy-id-token");
  assert.equal(WALLET_HEADER, "x-postage-wallet");
  assert.equal(ISSUED_AT_HEADER, "x-postage-issued");
  assert.equal(SIGNATURE_HEADER, "x-postage-signature");
  assert.equal(NONCE_HEADER, "x-postage-nonce");
  assert.equal(WALLET_NONCE_PATH, "/api/wallet-nonce");
});

test("what the writer puts on the wire is what the reader takes off it", () => {
  const written = signedProof(WALLET, 1_757_332_800, "0xsignature", "0xnonce");

  assert.deepEqual(readProof(new Headers(written)), {
    identityToken: null,
    wallet: WALLET,
    issuedAt: 1_757_332_800,
    signature: "0xsignature",
    nonce: "0xnonce",
  });
});

/// A missing timestamp header reads as 0 rather than null, because `Number`
/// says so. That is safe only because 0 is half a century stale and no window
/// admits it - not because anything downstream checks for it.
test("a request carrying no proof reads back empty, with a timestamp no window admits", () => {
  assert.deepEqual(readProof(new Headers()), {
    identityToken: null,
    wallet: null,
    issuedAt: 0,
    signature: null,
    nonce: null,
  });
});

/// The round trip four copies of a header name could never guarantee: headers
/// built by the writer the browser runs, carried on a real request, and handed
/// to the reader the server runs. Nothing in it spells a header name out, so it
/// passes only if both ends agree - which is the whole claim being made.
test("a proof the browser writes is accepted by the reader on the other side", async () => {
  const headers = await walletProof(null, signAs(holder), WALLET, statement);

  assert.equal(await holdsWallet(new Request(INBOX, { headers }), WALLET, statement), true);
});

/// And the statement is load-bearing in that trip, not decoration: the same
/// headers, read against what a different route asks its callers to sign, prove
/// nothing.
test("a proof written for one statement is refused by a reader checking another", async () => {
  const headers = await walletProof(null, signAs(holder), WALLET, statement);

  const proven = await holdsWallet(new Request(INBOX, { headers }), WALLET, (at) =>
    confirmStatement("demo", WALLET, at)
  );

  assert.equal(proven, false);
});

/// Well-formed headers naming the wallet, signed by a key that is not its own.
/// This is what an attacker can actually send, and it is the case where reading
/// the wrong header into the wrong argument would look like success.
test("a proof naming one wallet but signed by another is refused", async () => {
  const headers = await walletProof(null, signAs(impostor), WALLET, statement);

  assert.equal(await holdsWallet(new Request(INBOX, { headers }), WALLET, statement), false);
});

/// The replay, end to end and byte for byte. These are the very headers that
/// were just accepted, on a second request the reader has no way of telling
/// from the first except by remembering. Everything inside them is still
/// fresh: the timestamp, the signature, the wallet, the statement.
test("a proof the browser writes is accepted once and refused on its second use", async () => {
  const headers = await walletProof(null, signAs(holder), WALLET, statement);

  assert.equal(await holdsWallet(new Request(INBOX, { headers }), WALLET, statement), true);
  assert.equal(await holdsWallet(new Request(INBOX, { headers }), WALLET, statement), false);
});

/// And a second, independently collected proof still works afterwards, so what
/// the record above refuses is the replay and not the wallet.
test("a wallet can prove itself again with a proof of its own", async () => {
  const first = await walletProof(null, signAs(holder), WALLET, statement);
  await holdsWallet(new Request(INBOX, { headers: first }), WALLET, statement);

  const second = await walletProof(null, signAs(holder), WALLET, statement);

  assert.equal(await holdsWallet(new Request(INBOX, { headers: second }), WALLET, statement), true);
});

/// A proof with the nonce header dropped on the way, which is also what a
/// client built against the old contract would send.
test("a proof arriving without its nonce header is refused", async () => {
  const headers = new Headers((await walletProof(null, signAs(holder), WALLET, statement)) as Record<string, string>);
  headers.delete(NONCE_HEADER);

  assert.equal(await holdsWallet(new Request(INBOX, { headers }), WALLET, statement), false);
});

/// The other branch of the contract. A session holding an identity token signs
/// nothing, so the round trip is the header name alone - which is exactly the
/// part that used to be spelled out at both ends.
test("the identity token a writer attaches is the one the reader finds", async () => {
  const headers = await walletProof("token-abc", signAs(holder), WALLET, statement);

  assert.equal(readProof(new Headers(headers)).identityToken, "token-abc");
});

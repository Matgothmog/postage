import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import type { PrivateKeyAccount, VerifyMessageParameters } from "viem";
import { privateKeyToAccount } from "viem/accounts";
/// Reaches across into the app on purpose. The contract only means anything if
/// the browser's writer and the route's reader are the same two ends of it, so
/// the round trip below has to hold both at once rather than a stand-in for
/// either.
import { type SignMessage, walletProof } from "@/app/claim-inbox-helpers";
import { holdsWallet } from "./auth";
import { publicClient } from "./client";
import {
  IDENTITY_TOKEN_HEADER,
  ISSUED_AT_HEADER,
  SIGNATURE_HEADER,
  WALLET_HEADER,
  confirmStatement,
  readProof,
  readStatement,
  signedProof,
} from "./wallet-proof";

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
beforeEach(() => {
  asked = [];
  publicClient.verifyMessage = async (parameters) => {
    asked.push(parameters);
    return true;
  };
});

afterEach(() => {
  publicClient.verifyMessage = realVerifyMessage;

  assert.deepEqual(asked, [], "a signature was settled by the chain rather than locally");
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
});

test("what the writer puts on the wire is what the reader takes off it", () => {
  const written = signedProof(WALLET, 1_757_332_800, "0xsignature");

  assert.deepEqual(readProof(new Headers(written)), {
    identityToken: null,
    wallet: WALLET,
    issuedAt: 1_757_332_800,
    signature: "0xsignature",
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

/// The other branch of the contract. A session holding an identity token signs
/// nothing, so the round trip is the header name alone - which is exactly the
/// part that used to be spelled out at both ends.
test("the identity token a writer attaches is the one the reader finds", async () => {
  const headers = await walletProof("token-abc", signAs(holder), WALLET, statement);

  assert.equal(readProof(new Headers(headers)).identityToken, "token-abc");
});

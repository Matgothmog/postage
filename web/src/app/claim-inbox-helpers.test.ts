import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { MAIL_DOMAIN } from "@/lib/handle";
import {
  IDENTITY_TOKEN_HEADER,
  ISSUED_AT_HEADER,
  NONCE_HEADER,
  SIGNATURE_HEADER,
  WALLET_HEADER,
  WALLET_NONCE_PATH,
} from "@/lib/wallet-proof";
import { type SignMessage, signClaim, suggestHandle, walletProof } from "./claim-inbox-helpers";

// walletProof and signClaim only ever call the signMessage function handed to
// them, so a plain stub stands in for Privy's real hook — no DOM involved.
function stubSignMessage(signature: string): SignMessage {
  return async () => ({ signature });
}

function throwingSignMessage(): SignMessage {
  return async () => {
    throw new Error("user closed the wallet prompt");
  };
}

// Both now fetch a nonce before they prompt, so the endpoint stands in too.
// The value is a fixed string rather than a real minted nonce: nothing here
// verifies one, and what these tests are about is that whatever comes back
// reaches the signed text and the wire.
const NONCE = "nonce-from-the-server";

const realFetch = globalThis.fetch;

function stubNonceEndpoint(reply: () => Response): void {
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    if (String(input) !== WALLET_NONCE_PATH) throw new Error(`unexpected fetch to ${input}`);
    return reply();
  }) as typeof fetch;
}

beforeEach(() => {
  stubNonceEndpoint(() => Response.json({ nonce: NONCE }));
});

afterEach(() => {
  globalThis.fetch = realFetch;
});

test("walletProof returns the identity token header without signing, when one is present", async () => {
  const signMessage = stubSignMessage("unused");
  const headers = await walletProof("token-abc", signMessage, "0xWallet", () => "statement");
  assert.deepEqual(headers, { [IDENTITY_TOKEN_HEADER]: "token-abc" });
});

test("walletProof signs the statement and returns wallet headers, when there is no identity token", async () => {
  const signMessage = stubSignMessage("0xSignature");
  const headers = (await walletProof(null, signMessage, "0xWallet", (at) => `statement at ${at}`)) as Record<
    string,
    string
  >;
  assert.equal(headers[WALLET_HEADER], "0xWallet");
  assert.equal(headers[SIGNATURE_HEADER], "0xSignature");
  assert.equal(typeof headers[ISSUED_AT_HEADER], "string");
  assert.equal(headers[NONCE_HEADER], NONCE);
});

/// The nonce has to be inside what the wallet put its key to, not merely
/// alongside it — a reader that only saw it in a header would be checking a
/// value the signer never committed to.
test("walletProof signs the nonce along with the statement", async () => {
  let signed: string | undefined;
  const signMessage: SignMessage = async ({ message }) => {
    signed = String(message);
    return { signature: "0xSignature" };
  };

  await walletProof(null, signMessage, "0xWallet", () => "statement");

  assert.equal(signed, `statement\nNonce: ${NONCE}`);
});

/// A session with no identity token has no other proof to offer, so this
/// cannot degrade into an unsigned request — it has to reach the caller as a
/// failure.
test("walletProof fails rather than signing without a nonce when the endpoint refuses", async () => {
  stubNonceEndpoint(() => Response.json({ error: "no" }, { status: 503 }));

  await assert.rejects(() =>
    walletProof(null, stubSignMessage("0xSignature"), "0xWallet", () => "statement")
  );
});

/// The identity-token branch signs nothing, so it must not be made to wait on
/// a nonce it will never use.
test("walletProof asks for no nonce when it has an identity token", async () => {
  stubNonceEndpoint(() => {
    throw new Error("the identity-token branch asked for a nonce");
  });

  await walletProof("token-abc", stubSignMessage("unused"), "0xWallet", () => "statement");
});

test("signClaim returns the issued time and signature on success", async () => {
  const signMessage = stubSignMessage("0xSignature");
  const result = await signClaim(signMessage, {
    handle: "demo",
    destination: "demo@example.com",
    wallet: "0xWallet",
  });
  assert.ok(result);
  assert.equal(result?.signature, "0xSignature");
  assert.equal(typeof result?.issuedAt, "number");
  assert.equal(result?.nonce, NONCE);
});

/// Best effort covers this too: what comes back either proves the wallet or
/// does not, and a claim signed under no nonce is refused by the route anyway.
test("signClaim returns null rather than an unusable proof when no nonce can be fetched", async () => {
  stubNonceEndpoint(() => Response.json({ error: "no" }, { status: 503 }));

  const result = await signClaim(stubSignMessage("0xSignature"), {
    handle: "demo",
    destination: "demo@example.com",
    wallet: "0xWallet",
  });

  assert.equal(result, null);
});

test("signClaim is best effort: a wallet that refuses to sign returns null rather than throwing", async () => {
  const result = await signClaim(throwingSignMessage(), {
    handle: "demo",
    destination: "demo@example.com",
    wallet: "0xWallet",
  });
  assert.equal(result, null);
});

test("suggestHandle returns nothing for a signed-in session with no email", () => {
  assert.equal(suggestHandle(null), "");
});

test("suggestHandle drops the domain and normalises the local part", () => {
  assert.equal(suggestHandle(`John.Doe@${MAIL_DOMAIN}`), "john.doe");
});

test("suggestHandle strips characters the handle rules do not allow", () => {
  assert.equal(suggestHandle("john+promo@example.com"), "johnpromo");
});

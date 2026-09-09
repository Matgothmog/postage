import assert from "node:assert/strict";
import { test } from "node:test";
import { MAIL_DOMAIN } from "@/lib/handle";
import { IDENTITY_TOKEN_HEADER, ISSUED_AT_HEADER, SIGNATURE_HEADER, WALLET_HEADER } from "@/lib/wallet-proof";
import { type SignMessage, signClaim, suggestHandle, walletProof } from "./claim-inbox-helpers";

// walletProof and signClaim only ever call the signMessage function handed to
// them, so a plain stub stands in for Privy's real hook — no DOM and no
// network involved.
function stubSignMessage(signature: string): SignMessage {
  return async () => ({ signature });
}

function throwingSignMessage(): SignMessage {
  return async () => {
    throw new Error("user closed the wallet prompt");
  };
}

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

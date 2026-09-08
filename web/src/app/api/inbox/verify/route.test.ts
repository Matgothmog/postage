import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";

const workspace = mkdtempSync(join(tmpdir(), "postage-verify-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.MESSAGE_ID_SECRET = "x".repeat(32);
process.env.NEXT_PUBLIC_PRIVY_APP_ID = "test-app";

const { claimByHandle, reset, startClaim } = await import("@/lib/db");
const { hashCode } = await import("@/lib/verification");
const { POST } = await import("./route");

const HANDLE = "demo";
const CODE = "123456";
const WALLET = `0x${"11".repeat(20)}`;

async function confirm(headers: Record<string, string> = {}) {
  const response = await POST(
    new Request("http://localhost/api/inbox/verify", {
      method: "POST",
      headers: { "Content-Type": "application/json", ...headers },
      body: JSON.stringify({ handle: HANDLE, code: CODE }),
    })
  );
  return { status: response.status, body: (await response.json()) as { error?: string } };
}

beforeEach(async () => {
  await reset();
  await startClaim({
    handle: HANDLE,
    destination: "victim@example.com",
    wallet: WALLET,
    code_hash: hashCode(HANDLE, CODE),
    expires_at: Math.floor(Date.now() / 1000) + 900,
    cf_address_id: null,
    cf_verified_at: null,
  });
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

/// The whole of the attack this route used to allow: send someone a code they
/// did not ask for, get them to type it, and the claim completes on a wallet
/// they have never seen — their mail, somebody else's earnings.
test("a correct code alone does not complete a claim", async () => {
  const { status } = await confirm();
  assert.equal(status, 401);

  const claim = await claimByHandle(HANDLE);
  assert.equal(
    claim?.code_verified_at,
    null,
    "the right code from the wrong person must leave the claim exactly as it was"
  );
});

/// The wallet is public — it is indexed onchain and handed to every gated
/// sender — so naming it is not holding it.
test("naming the claim's wallet is not proof of holding it", async () => {
  const { status } = await confirm({ "x-postage-wallet": WALLET });
  assert.equal(status, 401);
});

test("an unverifiable identity token does not stand in for the wallet", async () => {
  const { status } = await confirm({ "privy-id-token": "not.a.token" });
  assert.equal(status, 401);
});

/// A refusal to prove the wallet must not spend the real claimer's guesses,
/// or anyone who knows a handle could lock them out of their own signup.
test("failing to prove the wallet costs the claimer no attempts", async () => {
  for (let n = 0; n < 10; n += 1) await confirm();

  const claim = await claimByHandle(HANDLE);
  assert.equal(claim?.attempts, 0);
});

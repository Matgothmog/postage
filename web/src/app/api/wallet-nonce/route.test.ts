import assert from "node:assert/strict";
import { test } from "node:test";
import { privateKeyToAccount } from "viem/accounts";
import { verifyWalletNonce } from "@/lib/wallet-nonce";
import { POST } from "./route";

process.env.MESSAGE_ID_SECRET = "x".repeat(32);

const WALLET = privateKeyToAccount(`0x${"11".repeat(32)}`).address;
const OTHER_WALLET = privateKeyToAccount(`0x${"22".repeat(32)}`).address;

const NONCE = "https://postage.test/api/wallet-nonce";

function ask(body: unknown): Request {
  return new Request(NONCE, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: typeof body === "string" ? body : JSON.stringify(body),
  });
}

test("a nonce the route hands out is one the reader will accept", async () => {
  const response = await POST(ask({ wallet: WALLET }));
  const { nonce } = (await response.json()) as { nonce: string };

  assert.equal(response.status, 200);
  assert.notEqual(verifyWalletNonce(nonce, WALLET), null);
});

/// The binding is made here, at mint time, not asserted later — so a nonce
/// collected from this route for one address is worthless to another.
test("a nonce is minted for the wallet that asked and no other", async () => {
  const response = await POST(ask({ wallet: WALLET }));
  const { nonce } = (await response.json()) as { nonce: string };

  assert.equal(verifyWalletNonce(nonce, OTHER_WALLET), null);
});

test("two callers asking at the same moment get different nonces", async () => {
  const [first, second] = await Promise.all([
    POST(ask({ wallet: WALLET })).then((response) => response.json()),
    POST(ask({ wallet: WALLET })).then((response) => response.json()),
  ]);

  assert.notEqual((first as { nonce: string }).nonce, (second as { nonce: string }).nonce);
});

test("a body that is not JSON is refused", async () => {
  const response = await POST(ask("not json at all"));

  assert.equal(response.status, 400);
});

test("a request naming no wallet is refused", async () => {
  const response = await POST(ask({}));

  assert.equal(response.status, 400);
});

test("a wallet that is not an address is refused", async () => {
  const response = await POST(ask({ wallet: "0xnot-an-address" }));

  assert.equal(response.status, 400);
});

/// `typeof wallet !== "string"` is not redundant with `isAddress`: viem's
/// check is typed for a string, and a caller controls the JSON.
test("a wallet that is not a string at all is refused rather than crashing", async () => {
  for (const wallet of [42, null, true, { address: WALLET }, [WALLET]]) {
    const response = await POST(ask({ wallet }));
    assert.equal(response.status, 400, `accepted ${JSON.stringify(wallet)}`);
  }
});

/// The response says what a nonce is worth and nothing about how it was made.
/// Anything derived from the key would be a key-recovery problem handed out to
/// anyone who asks, and this route asks nobody for anything.
test("the response carries the nonce and its window, and no key material", async () => {
  const response = await POST(ask({ wallet: WALLET }));
  const body = (await response.json()) as Record<string, unknown>;

  assert.deepEqual(Object.keys(body).sort(), ["expiresIn", "nonce"]);
  assert.equal(body.expiresIn, 120);
  assert.ok(!JSON.stringify(body).includes(process.env.MESSAGE_ID_SECRET as string));
});

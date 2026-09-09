import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { privateKeyToAccount } from "viem/accounts";
import { WALLET_NONCE_TTL_SECONDS, mintWalletNonce, verifyWalletNonce } from "./wallet-nonce";

process.env.MESSAGE_ID_SECRET = "x".repeat(32);

const WALLET = privateKeyToAccount(`0x${"11".repeat(32)}`).address;
const OTHER_WALLET = privateKeyToAccount(`0x${"22".repeat(32)}`).address;

/// The window is judged on the server's clock, so every expiry case below is a
/// clock case. Frozen so "expires in exactly this many seconds" cannot become
/// "one fewer" whenever the second turns mid-test.
const FROZEN_MS = Date.UTC(2026, 8, 8, 12, 0, 0);
const realDateNow = Date.now;

beforeEach(() => {
  Date.now = () => FROZEN_MS;
});

afterEach(() => {
  Date.now = realDateNow;
});

test("a freshly minted nonce verifies for the wallet it was minted for", () => {
  assert.notEqual(verifyWalletNonce(mintWalletNonce(WALLET), WALLET), null);
});

test("a nonce reports the moment it stops being one", () => {
  const expiresAt = verifyWalletNonce(mintWalletNonce(WALLET), WALLET);

  assert.equal(expiresAt, Math.floor(FROZEN_MS / 1000) + WALLET_NONCE_TTL_SECONDS);
});

/// The binding that stops a nonce collected by one wallet's session from
/// standing in for another's. Both are real addresses, and the only difference
/// between the two calls is which one is named.
test("a nonce minted for one wallet proves nothing for another", () => {
  assert.equal(verifyWalletNonce(mintWalletNonce(WALLET), OTHER_WALLET), null);
});

/// A wallet address survives a round trip through a lowercasing header or
/// statement, so the checksummed form is not the only one that reaches here.
test("a nonce verifies against the same wallet named in lowercase", () => {
  assert.notEqual(verifyWalletNonce(mintWalletNonce(WALLET), WALLET.toLowerCase()), null);
});

test("a nonce is worthless one second after its window closes", () => {
  const nonce = mintWalletNonce(WALLET);
  assert.notEqual(verifyWalletNonce(nonce, WALLET), null, "precondition: it is good now");

  Date.now = () => FROZEN_MS + (WALLET_NONCE_TTL_SECONDS + 1) * 1000;

  assert.equal(verifyWalletNonce(nonce, WALLET), null);
});

/// The forgery the MAC exists to stop: the expiry is in plain sight, so
/// pushing it out is the first thing anyone would try.
test("an expiry edited to a later one proves nothing", () => {
  const [expiresAtText, random, mac] = mintWalletNonce(WALLET).split(".");
  const stretched = `${Number(expiresAtText) + 86_400}.${random}.${mac}`;

  assert.equal(verifyWalletNonce(stretched, WALLET), null);
});

test("a nonce minted with no key at all proves nothing", () => {
  const random = "ab".repeat(16);
  const expiresAt = Math.floor(FROZEN_MS / 1000) + WALLET_NONCE_TTL_SECONDS;

  assert.equal(verifyWalletNonce(`${expiresAt}.${random}.${"cd".repeat(32)}`, WALLET), null);
});

test("a nonce whose random half was swapped proves nothing", () => {
  const [expiresAtText, , mac] = mintWalletNonce(WALLET).split(".");

  assert.equal(verifyWalletNonce(`${expiresAtText}.${"00".repeat(16)}.${mac}`, WALLET), null);
});

/// Every shape a caller-controlled string can arrive in that is not a nonce.
/// None of them may throw: this runs inside `provesWallet`, which answers a
/// question with a boolean and must not turn a malformed header into a 500.
test("anything that is not a nonce is refused rather than throwing", () => {
  const rubbish = [
    "",
    ".",
    "..",
    "not-a-nonce",
    "1.2",
    "1.2.3.4",
    `${Math.floor(FROZEN_MS / 1000) + 60}.${"ab".repeat(16)}`,
    `-1.${"ab".repeat(16)}.${"cd".repeat(32)}`,
    `1e9.${"ab".repeat(16)}.${"cd".repeat(32)}`,
    `${Math.floor(FROZEN_MS / 1000) + 60}.NOTHEX${"ab".repeat(13)}.${"cd".repeat(32)}`,
  ];

  for (const offered of rubbish) {
    assert.equal(verifyWalletNonce(offered, WALLET), null, `accepted ${JSON.stringify(offered)}`);
  }
});

/// Two nonces minted in the same second must still differ, or a "spent" record
/// keyed on the nonce would refuse a second, legitimate sign-in attempt made
/// inside one second of the first.
test("two nonces minted in the same second are different values", () => {
  assert.notEqual(mintWalletNonce(WALLET), mintWalletNonce(WALLET));
});

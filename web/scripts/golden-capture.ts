// Captures golden vectors from the real TypeScript implementation so the Rust
// ports can be tested against them byte for byte.
//
//   cd web && node --experimental-strip-types --import ./test/register.mjs scripts/golden-capture.ts
//
// Everything is deterministic: throwaway keys and secrets generated for this
// purpose only, a fixed clock (Date.now is replaced) and fixed "random" bytes
// (node:crypto randomBytes/randomInt and crypto.getRandomValues are replaced).
// No environment is read and no .env file is touched.

import nodeCrypto from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { hashSignal } from "@worldcoin/idkit/hashing";
import { signRequest } from "@worldcoin/idkit-server";
import { getAddress } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { challengeMail, type ChallengeMailFacts } from "../src/lib/challenge-email";
import { POSTAGE_ESCROW, chain } from "../src/lib/contracts";
import { quote } from "../src/lib/pricing";
import { messageIdFor, signQuote } from "../src/lib/quote";
import type { SenderSignals } from "../src/lib/reputation";
import type { Tier } from "../src/lib/tiers";
import { codeMatches, generateCode, hashCode } from "../src/lib/verification";
import { mintWalletNonce, verifyWalletNonce, WALLET_NONCE_TTL_SECONDS } from "../src/lib/wallet-nonce";

// Throwaway values minted for these fixtures. They protect nothing.
const CLASSIFIER_PRIVATE_KEY: `0x${string}` = `0x${"11".repeat(32)}`;
const MESSAGE_ID_SECRET = "golden-vector-message-id-secret-not-for-production";
const WORLD_RP_SIGNING_KEY = `0x${"22".repeat(32)}`;
process.env.CLASSIFIER_PRIVATE_KEY = CLASSIFIER_PRIVATE_KEY;
process.env.MESSAGE_ID_SECRET = MESSAGE_ID_SECRET;

const OUT_DIR = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "fixtures", "golden");

// ---- Controlled time and randomness ------------------------------------

let clockSeconds = 0;
Date.now = (): number => clockSeconds * 1000;

function atTime<T>(seconds: number, run: () => T): T {
  clockSeconds = seconds;
  return run();
}

let queuedBytes: Buffer | null = null;
let queuedInt: number | null = null;

const patchable = nodeCrypto as unknown as Record<string, unknown>;
patchable.randomBytes = (size: number): Buffer => {
  if (queuedBytes === null || queuedBytes.length !== size) throw new Error("randomBytes was not primed");
  const bytes = queuedBytes;
  queuedBytes = null;
  return bytes;
};
patchable.randomInt = (): number => {
  if (queuedInt === null) throw new Error("randomInt was not primed");
  const value = queuedInt;
  queuedInt = null;
  return value;
};
syncBuiltinESMExports();

let queuedRandomValues: Uint8Array | null = null;
Object.defineProperty(globalThis.crypto, "getRandomValues", {
  configurable: true,
  value: (target: Uint8Array): Uint8Array => {
    if (queuedRandomValues === null || queuedRandomValues.length !== target.length) {
      throw new Error("getRandomValues was not primed");
    }
    target.set(queuedRandomValues);
    queuedRandomValues = null;
    return target;
  },
});

function patternBytes(length: number, seed: number): Buffer {
  return Buffer.from(Array.from({ length }, (_, index) => (seed + index * 7) % 256));
}

// ---- Output ------------------------------------------------------------

function writeFixture(name: string, description: string, body: Record<string, unknown>): void {
  const text = `${JSON.stringify({ function: name, description, ...body }, null, 2)}\n`;
  writeFileSync(join(OUT_DIR, `${name}.json`), text);
}

// ---- signQuote ---------------------------------------------------------

const INBOX = "0x1234567890abcdef1234567890abcdef12345678";
const MESSAGE_ID_A = `0x${"ab".repeat(32)}`;
const MESSAGE_ID_B = `0x${"00".repeat(31)}01`;

async function captureSignQuote(): Promise<void> {
  const cases: { name: string; now: number; messageId: string; inbox: string; tier: Tier; amount: bigint }[] = [
    { name: "human tier, one cent", now: 1_800_000_000, messageId: MESSAGE_ID_A, inbox: INBOX, tier: "human", amount: 10_000_000_000_000_000n },
    { name: "important tier, zero amount", now: 1_800_000_000, messageId: MESSAGE_ID_A, inbox: INBOX, tier: "important", amount: 0n },
    { name: "commercial tier", now: 1_800_000_123, messageId: MESSAGE_ID_B, inbox: INBOX, tier: "commercial", amount: 10_000_000_000_000_000n },
    { name: "dangerous tier, ten times floor", now: 1_800_000_123, messageId: MESSAGE_ID_B, inbox: INBOX, tier: "dangerous", amount: 100_000_000_000_000_000n },
    { name: "uint256 max amount", now: 1_700_000_000, messageId: MESSAGE_ID_A, inbox: INBOX, tier: "commercial", amount: 2n ** 256n - 1n },
    { name: "clock at zero", now: 0, messageId: MESSAGE_ID_A, inbox: INBOX, tier: "human", amount: 1n },
    { name: "expiry lands exactly on uint40 max", now: 2 ** 40 - 1 - 24 * 60 * 60, messageId: MESSAGE_ID_A, inbox: INBOX, tier: "human", amount: 1n },
    { name: "EIP-55 checksummed inbox address", now: 1_800_000_000, messageId: MESSAGE_ID_A, inbox: getAddress(INBOX), tier: "commercial", amount: 5_000_000_000_000_000n },
  ];

  const results = [];
  for (const entry of cases) {
    clockSeconds = entry.now;
    const signed = await signQuote(entry.messageId as `0x${string}`, entry.inbox as `0x${string}`, entry.tier, entry.amount);
    results.push({
      name: entry.name,
      input: { now: entry.now, messageId: entry.messageId, inbox: entry.inbox, tier: entry.tier, amount: entry.amount.toString() },
      output: signed,
    });
  }

  writeFixture("sign-quote", "signQuote: EIP-712 Quote typed-data signature (viem signTypedData). Expiry is now + 86400.", {
    config: {
      classifierPrivateKey: CLASSIFIER_PRIVATE_KEY,
      signerAddress: privateKeyToAccount(CLASSIFIER_PRIVATE_KEY).address,
      domain: { name: "Postage", version: "2", chainId: chain.id, verifyingContract: POSTAGE_ESCROW },
      primaryType: "Quote",
      types: { Quote: ["bytes32 messageId", "address inbox", "uint8 tier", "uint256 amount", "uint40 expiresAt"] },
      tierIndex: { human: 0, important: 1, commercial: 2, dangerous: 3 },
      ttlSeconds: 86_400,
    },
    cases: results,
  });
}

// ---- messageIdFor ------------------------------------------------------

function captureMessageId(): void {
  const inputs = [
    { name: "plain", sender: "alice@example.com", handle: "bob", subject: "Hello there", receivedAt: 1_800_000_000 },
    { name: "sender and handle are lowercased", sender: "Alice@Example.COM", handle: "BoB", subject: "Hello there", receivedAt: 1_800_000_000 },
    { name: "subject case is preserved", sender: "alice@example.com", handle: "bob", subject: "HELLO There", receivedAt: 1_800_000_000 },
    { name: "empty subject", sender: "alice@example.com", handle: "bob", subject: "", receivedAt: 1_800_000_000 },
    { name: "subject containing the separator", sender: "alice@example.com", handle: "bob", subject: "a|b|c", receivedAt: 1_800_000_000 },
    { name: "unicode subject", sender: "alice@example.com", handle: "bob", subject: "Reçu: 100 € \u{1F4EC} 日本語", receivedAt: 1_800_000_000 },
    { name: "receivedAt zero", sender: "alice@example.com", handle: "bob", subject: "x", receivedAt: 0 },
    { name: "receivedAt one second later", sender: "alice@example.com", handle: "bob", subject: "Hello there", receivedAt: 1_800_000_001 },
    { name: "handle with dots and dashes", sender: "no-reply@mail.example.org", handle: "a.b_c-d", subject: "Your code is 123456", receivedAt: 1_234_567_890 },
  ];

  writeFixture("message-id", "messageIdFor: 0x + hex HMAC-SHA256 over `sender.lower|handle.lower|subject|receivedAt`, keyed with MESSAGE_ID_SECRET (raw UTF-8 bytes).", {
    config: { messageIdSecret: MESSAGE_ID_SECRET },
    cases: inputs.map((input) => ({ name: input.name, input: { sender: input.sender, handle: input.handle, subject: input.subject, receivedAt: input.receivedAt }, output: messageIdFor(input.sender, input.handle, input.subject, input.receivedAt) })),
  });
}

// ---- wallet nonce ------------------------------------------------------

const WALLET = "0xAbCdEf0123456789aBcDeF0123456789AbCdEf01";
const MINT_TIME = 1_800_000_000;

function mint(wallet: string, now: number, randomBytes: Buffer): string {
  queuedBytes = randomBytes;
  return atTime(now, () => mintWalletNonce(wallet));
}

function captureWalletNonce(): void {
  const randomA = patternBytes(16, 1);
  const randomB = patternBytes(16, 200);
  const minted = [
    { name: "mint, mixed-case wallet", wallet: WALLET, now: MINT_TIME, random: randomA },
    { name: "mint, same wallet lowercased gives the same tag", wallet: WALLET.toLowerCase(), now: MINT_TIME, random: randomA },
    { name: "mint, different random bytes", wallet: WALLET, now: MINT_TIME, random: randomB },
    { name: "mint, clock at zero", wallet: WALLET, now: 0, random: randomA },
  ].map((entry) => ({
    name: entry.name,
    input: { wallet: entry.wallet, now: entry.now, randomBytesHex: entry.random.toString("hex") },
    output: mint(entry.wallet, entry.now, entry.random),
  }));

  const nonce = mint(WALLET, MINT_TIME, randomA);
  const [expiresText, randomHex, tagHex] = nonce.split(".");
  const expiresAt = MINT_TIME + WALLET_NONCE_TTL_SECONDS;
  const flippedTag = `${tagHex.slice(0, -1)}${tagHex.endsWith("0") ? "1" : "0"}`;
  const otherWallet = "0x0000000000000000000000000000000000000001";

  const verifyCases: { name: string; nonce: string; wallet: string; now: number }[] = [
    { name: "valid at mint time", nonce, wallet: WALLET, now: MINT_TIME },
    { name: "valid, wallet case differs", nonce, wallet: WALLET.toLowerCase(), now: MINT_TIME },
    { name: "valid one second before expiry", nonce, wallet: WALLET, now: expiresAt - 1 },
    { name: "refused exactly at expiry", nonce, wallet: WALLET, now: expiresAt },
    { name: "refused one second after expiry", nonce, wallet: WALLET, now: expiresAt + 1 },
    { name: "refused for a different wallet", nonce, wallet: otherWallet, now: MINT_TIME },
    { name: "refused with last tag nibble flipped", nonce: `${expiresText}.${randomHex}.${flippedTag}`, wallet: WALLET, now: MINT_TIME },
    { name: "refused with expiry text altered (leading zero)", nonce: `0${expiresText}.${randomHex}.${tagHex}`, wallet: WALLET, now: MINT_TIME },
    { name: "refused with expiry extended", nonce: `${expiresAt + 1000}.${randomHex}.${tagHex}`, wallet: WALLET, now: MINT_TIME },
    { name: "refused with truncated tag", nonce: `${expiresText}.${randomHex}.${tagHex.slice(0, 63)}`, wallet: WALLET, now: MINT_TIME },
    { name: "refused with two parts", nonce: `${expiresText}.${tagHex}`, wallet: WALLET, now: MINT_TIME },
    { name: "refused with four parts", nonce: `${nonce}.extra`, wallet: WALLET, now: MINT_TIME },
    { name: "refused with empty string", nonce: "", wallet: WALLET, now: MINT_TIME },
    { name: "refused with non-digit expiry", nonce: `12a45.${randomHex}.${tagHex}`, wallet: WALLET, now: MINT_TIME },
    { name: "refused with empty expiry", nonce: `.${randomHex}.${tagHex}`, wallet: WALLET, now: MINT_TIME },
    { name: "refused with short random", nonce: `${expiresText}.${randomHex.slice(0, 30)}.${tagHex}`, wallet: WALLET, now: MINT_TIME },
    { name: "refused with uppercase random hex", nonce: `${expiresText}.${randomHex.toUpperCase()}.${tagHex}`, wallet: WALLET, now: MINT_TIME },
    { name: "refused with non-hex random", nonce: `${expiresText}.${"zz".repeat(16)}.${tagHex}`, wallet: WALLET, now: MINT_TIME },
  ];

  writeFixture("wallet-nonce", "mintWalletNonce / verifyWalletNonce: `<expiresAt>.<random hex>.<HMAC tag>`. Key = HKDF-SHA256(MESSAGE_ID_SECRET, salt \"\", info \"postage:wallet-nonce\", 32). Tag preimage `postage-wallet-nonce|wallet.lower|expiresAt|random`. verify returns the expiry or null.", {
    config: { messageIdSecret: MESSAGE_ID_SECRET, ttlSeconds: WALLET_NONCE_TTL_SECONDS, randomByteCount: 16 },
    mint: minted,
    verify: verifyCases.map((entry) => ({
      name: entry.name,
      input: { nonce: entry.nonce, wallet: entry.wallet, now: entry.now },
      output: atTime(entry.now, () => verifyWalletNonce(entry.nonce, entry.wallet)),
    })),
  });
}

// ---- verification code -------------------------------------------------

function captureCodeHash(): void {
  const hashInputs = [
    { name: "ordinary", handle: "alice", code: "123456" },
    { name: "handle is lowercased", handle: "ALICE", code: "123456" },
    { name: "leading zeros", handle: "alice", code: "000042" },
    { name: "different handle, same code", handle: "bob", code: "123456" },
    { name: "empty code", handle: "alice", code: "" },
    { name: "handle with punctuation", handle: "a.b_c-d", code: "999999" },
    { name: "handle containing the separator", handle: "al:ice", code: "123456" },
  ];
  const stored = hashCode("alice", "123456");

  const matchInputs = [
    { name: "match", handle: "alice", code: "123456", expected: stored },
    { name: "match, handle case differs", handle: "ALICE", code: "123456", expected: stored },
    { name: "mismatch, wrong code", handle: "alice", code: "123457", expected: stored },
    { name: "mismatch, other handle", handle: "bob", code: "123456", expected: stored },
    { name: "mismatch, stored hash one nibble off", handle: "alice", code: "123456", expected: `${stored.slice(0, -1)}${stored.endsWith("0") ? "1" : "0"}` },
    { name: "mismatch, stored hash too short", handle: "alice", code: "123456", expected: stored.slice(0, 62) },
    { name: "mismatch, stored hash empty", handle: "alice", code: "123456", expected: "" },
    { name: "mismatch, stored hash too long", handle: "alice", code: "123456", expected: `${stored}00` },
    { name: "uppercase stored hash still matches", handle: "alice", code: "123456", expected: stored.toUpperCase() },
  ];

  const codeInputs = [0, 42, 999_999, 123_456].map((value) => {
    queuedInt = value;
    return { name: `randomInt returns ${value}`, input: { randomInt: value }, output: generateCode() };
  });

  writeFixture("code-hash", "hashCode / codeMatches / generateCode. Key = HKDF-SHA256(MESSAGE_ID_SECRET, salt \"\", info \"postage:verification-code\", 32). hashCode = hex HMAC-SHA256 over `handle.lower:code`. generateCode = randomInt(0, 1_000_000) zero-padded to 6 digits.", {
    config: { messageIdSecret: MESSAGE_ID_SECRET },
    hashCode: hashInputs.map((entry) => ({ name: entry.name, input: { handle: entry.handle, code: entry.code }, output: hashCode(entry.handle, entry.code) })),
    codeMatches: matchInputs.map((entry) => ({ name: entry.name, input: { handle: entry.handle, code: entry.code, expected: entry.expected }, output: codeMatches(entry.handle, entry.code, entry.expected) })),
    generateCode: codeInputs,
  });
}

// ---- hashSignal --------------------------------------------------------

function captureHashSignal(): void {
  const signals = [
    { name: "empty string", signal: "" },
    { name: "short ascii", signal: "hello" },
    { name: "challenge-token-shaped value", signal: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" },
    { name: "uppercase", signal: "HELLO" },
    { name: "0x-prefixed hex string", signal: "0xdeadbeef" },
    { name: "unicode", signal: "héllo \u{1F4EC} 世界" },
    { name: "long ascii", signal: "x".repeat(1000) },
    { name: "whitespace", signal: " a b\n" },
  ];
  writeFixture("hash-signal", "hashSignal from @worldcoin/idkit/hashing, as used by app/api/world/verify (compared lowercased against signal_hash).", {
    cases: signals.map((entry) => ({ name: entry.name, input: { signal: entry.signal }, output: hashSignal(entry.signal) })),
  });
}

// ---- signRequest -------------------------------------------------------

function captureRpSignRequest(): void {
  const cases = [
    { name: "action, ttl 300 (production values)", now: 1_800_000_000, action: "selfie-check" as string | undefined, ttl: 300 as number | undefined, seed: 3, key: WORLD_RP_SIGNING_KEY },
    { name: "same random, different clock", now: 1_800_000_001, action: "selfie-check", ttl: 300, seed: 3, key: WORLD_RP_SIGNING_KEY },
    { name: "different random bytes", now: 1_800_000_000, action: "selfie-check", ttl: 300, seed: 99, key: WORLD_RP_SIGNING_KEY },
    { name: "different action", now: 1_800_000_000, action: "other-action", ttl: 300, seed: 3, key: WORLD_RP_SIGNING_KEY },
    { name: "no action", now: 1_800_000_000, action: undefined, ttl: 300, seed: 3, key: WORLD_RP_SIGNING_KEY },
    { name: "default ttl", now: 1_800_000_000, action: "selfie-check", ttl: undefined, seed: 3, key: WORLD_RP_SIGNING_KEY },
    { name: "key without 0x prefix", now: 1_800_000_000, action: "selfie-check", ttl: 300, seed: 3, key: WORLD_RP_SIGNING_KEY.slice(2) },
    { name: "random bytes all 0xff", now: 1_800_000_000, action: "selfie-check", ttl: 300, seed: -1, key: WORLD_RP_SIGNING_KEY },
  ];

  const results = cases.map((entry) => {
    const random = entry.seed < 0 ? new Uint8Array(32).fill(255) : new Uint8Array(patternBytes(32, entry.seed));
    queuedRandomValues = random;
    const output = atTime(entry.now, () =>
      signRequest({
        signingKeyHex: entry.key,
        ...(entry.action === undefined ? {} : { action: entry.action }),
        ...(entry.ttl === undefined ? {} : { ttl: entry.ttl }),
      })
    );
    return {
      name: entry.name,
      input: { signingKeyHex: entry.key, action: entry.action ?? null, ttl: entry.ttl ?? null, now: entry.now, randomBytesHex: Buffer.from(random).toString("hex") },
      output,
    };
  });

  writeFixture("rp-sign-request", "signRequest from @worldcoin/idkit-server (World ID 4.0 rp_context signature). Randomness is the 32 bytes from crypto.getRandomValues (hashed to a field element for the nonce); clock is Date.now. Default ttl when omitted is whatever the SDK uses (recorded in the case).", {
    config: { productionTtlSeconds: 300 },
    cases: results,
  });
}

// ---- challenge email ---------------------------------------------------

function captureChallengeEmail(): void {
  const NOW = 1_800_000_000;
  const base: ChallengeMailFacts = {
    handle: "Alice",
    subject: "Quick question about your pricing",
    amount: 10_000_000_000_000_000n,
    reasons: ["Reads human, nobody proved it"],
    challengeUrl: "https://postage-seven.vercel.app/c/tok_0123456789abcdef",
    appUrl: "https://postage-seven.vercel.app",
    heldUntil: NOW + 24 * 3600,
  };
  const variants: { name: string; facts: ChallengeMailFacts }[] = [
    { name: "baseline, human reason, 24 hour hold", facts: base },
    { name: "commercial with several reasons", facts: { ...base, amount: 10_000_000_000_000_000n, reasons: ["Automated mail nobody asked for", "Spam-reported 2 of 5 times here", "Holds 2 ENS names"] } },
    { name: "dangerous, ten cent price", facts: { ...base, amount: 100_000_000_000_000_000n, reasons: ["Reads as an attempt to deceive"] } },
    { name: "price under a cent", facts: { ...base, amount: 5_000_000_000_000_000n } },
    { name: "price of a dollar and a half", facts: { ...base, amount: 1_500_000_000_000_000_000n } },
    { name: "zero amount renders as free", facts: { ...base, amount: 0n } },
    { name: "no reasons", facts: { ...base, reasons: [] } },
    { name: "html in subject, reasons and urls is escaped", facts: { ...base, subject: `<script>alert("x")</script> & 'q'`, reasons: [`<b>bold</b> & "quoted" 'single'`], challengeUrl: `https://x.test/c/a"b'c<d>&e`, appUrl: `https://x.test/?a=1&b=<2>` } },
    { name: "empty subject becomes (no subject)", facts: { ...base, subject: "" } },
    { name: "whitespace-only subject becomes (no subject)", facts: { ...base, subject: " \t\n " } },
    { name: "newlines and controls in subject flattened", facts: { ...base, subject: "Line one\r\nSecond A PERSON WROTE IT - free\u0007\u0085tail" } },
    { name: "bidi override and zero-width chars flattened", facts: { ...base, subject: "pay‮evil​now next end nbsp" } },
    { name: "subject exactly 200 chars is untouched", facts: { ...base, subject: "s".repeat(200) } },
    { name: "subject of 201 chars is truncated with ellipsis", facts: { ...base, subject: "s".repeat(201) } },
    { name: "long subject with trailing spaces after truncation point", facts: { ...base, subject: `${"w".repeat(199)}   ${"z".repeat(50)}` } },
    { name: "unicode subject", facts: { ...base, subject: "Reçu \u{1F4EC} 日本語" } },
    { name: "mixed-case handle is lowercased in address", facts: { ...base, handle: "Bob.Smith_X" } },
    { name: "hold of exactly one hour", facts: { ...base, heldUntil: NOW + 3600 } },
    { name: "hold under an hour clamps to an hour", facts: { ...base, heldUntil: NOW + 600 } },
    { name: "hold already expired clamps to an hour", facts: { ...base, heldUntil: NOW - 7200 } },
    { name: "hold of 5399 seconds rounds to 1 hour", facts: { ...base, heldUntil: NOW + 5399 } },
    { name: "hold of 5400 seconds rounds to 2 hours", facts: { ...base, heldUntil: NOW + 5400 } },
    { name: "hold of 48 hours", facts: { ...base, heldUntil: NOW + 48 * 3600 } },
  ];

  writeFixture("challenge-email", "challengeMail: subject, html and text of the held-sender notice. amount is a decimal string of 18-decimal USDC base units.", {
    config: { now: NOW },
    cases: variants.map((variant) => ({
      name: variant.name,
      input: { ...variant.facts, amount: variant.facts.amount.toString() },
      output: atTime(NOW, () => challengeMail(variant.facts)),
    })),
  });
}

// ---- pricing -----------------------------------------------------------

function capturePricing(): void {
  const NOW = 1_800_000_000;
  const YEAR = 365 * 24 * 60 * 60;
  const FLOOR = 10_000_000_000_000_000n;
  const none: SenderSignals = { paidCount: 0, spamReports: 0, spamRate: 0, ensNames: 0, oldestEnsAt: null };

  const cases: { name: string; floor: bigint; tier: Tier; signals: SenderSignals | null; degraded: boolean }[] = [];
  for (const tier of ["human", "important", "commercial", "dangerous"] as const) {
    cases.push({ name: `${tier}, no signals`, floor: FLOOR, tier, signals: null, degraded: false });
    cases.push({ name: `${tier}, empty signals`, floor: FLOOR, tier, signals: none, degraded: false });
    cases.push({ name: `${tier}, empty signals, degraded`, floor: FLOOR, tier, signals: none, degraded: true });
  }
  cases.push(
    { name: "commercial, paidCount 2 (below good-history threshold)", floor: FLOOR, tier: "commercial", signals: { ...none, paidCount: 2 }, degraded: false },
    { name: "commercial, paidCount 3, spamRate 0 (good history halves)", floor: FLOOR, tier: "commercial", signals: { ...none, paidCount: 3 }, degraded: false },
    { name: "commercial, paidCount 3, spamRate 0.19999 (just under cutoff)", floor: FLOOR, tier: "commercial", signals: { paidCount: 3, spamReports: 0, spamRate: 0.19999, ensNames: 0, oldestEnsAt: null }, degraded: false },
    { name: "commercial, paidCount 3, spamRate 0.2 (at cutoff)", floor: FLOOR, tier: "commercial", signals: { paidCount: 5, spamReports: 1, spamRate: 0.2, ensNames: 0, oldestEnsAt: null }, degraded: false },
    { name: "commercial, spam rate 0.5 with history", floor: FLOOR, tier: "commercial", signals: { paidCount: 4, spamReports: 2, spamRate: 0.5, ensNames: 0, oldestEnsAt: null }, degraded: false },
    { name: "human, spam rate 0.1 with 10 messages", floor: FLOOR, tier: "human", signals: { paidCount: 10, spamReports: 1, spamRate: 0.1, ensNames: 0, oldestEnsAt: null }, degraded: false },
    { name: "spamRate positive but paidCount 0 is ignored", floor: FLOOR, tier: "commercial", signals: { paidCount: 0, spamReports: 3, spamRate: 0.5, ensNames: 0, oldestEnsAt: null }, degraded: false },
    { name: "commercial, one ENS name, no age", floor: FLOOR, tier: "commercial", signals: { ...none, ensNames: 1 }, degraded: false },
    { name: "commercial, two ENS names (plural)", floor: FLOOR, tier: "commercial", signals: { ...none, ensNames: 2 }, degraded: false },
    { name: "commercial, ENS exactly one year old (no age bonus)", floor: FLOOR, tier: "commercial", signals: { ...none, ensNames: 1, oldestEnsAt: NOW - YEAR }, degraded: false },
    { name: "commercial, ENS one year and a second old", floor: FLOOR, tier: "commercial", signals: { ...none, ensNames: 1, oldestEnsAt: NOW - YEAR - 1 }, degraded: false },
    { name: "commercial, ENS 2.9 years old floors to 2", floor: FLOOR, tier: "commercial", signals: { ...none, ensNames: 3, oldestEnsAt: NOW - Math.floor(2.9 * YEAR) }, degraded: false },
    { name: "commercial, ENS dated in the future (negative age)", floor: FLOOR, tier: "commercial", signals: { ...none, ensNames: 1, oldestEnsAt: NOW + 1000 }, degraded: false },
    { name: "dangerous, everything bad (hits the ceiling)", floor: FLOOR, tier: "dangerous", signals: { paidCount: 10, spamReports: 10, spamRate: 1, ensNames: 0, oldestEnsAt: null }, degraded: false },
    { name: "dangerous, degraded, spam-reported", floor: FLOOR, tier: "dangerous", signals: { paidCount: 2, spamReports: 1, spamRate: 0.5, ensNames: 0, oldestEnsAt: null }, degraded: true },
    { name: "dangerous, degraded, ENS and good history", floor: FLOOR, tier: "dangerous", signals: { paidCount: 5, spamReports: 0, spamRate: 0, ensNames: 1, oldestEnsAt: NOW - 3 * YEAR }, degraded: true },
    { name: "commercial, good history + ENS + old (clamped up to floor)", floor: FLOOR, tier: "commercial", signals: { paidCount: 9, spamReports: 0, spamRate: 0, ensNames: 2, oldestEnsAt: NOW - 5 * YEAR }, degraded: false },
    { name: "human, good history only (clamped up to floor)", floor: FLOOR, tier: "human", signals: { ...none, paidCount: 3 }, degraded: false },
    { name: "important ignores signals and degraded", floor: FLOOR, tier: "important", signals: { paidCount: 9, spamReports: 9, spamRate: 1, ensNames: 5, oldestEnsAt: NOW - 9 * YEAR }, degraded: true },
    { name: "floor of zero", floor: 0n, tier: "commercial", signals: null, degraded: false },
    { name: "floor of one base unit, bps truncation", floor: 1n, tier: "dangerous", signals: null, degraded: false },
    { name: "floor of 3 with 1.4x multiplier truncates", floor: 3n, tier: "commercial", signals: { paidCount: 1, spamReports: 0, spamRate: 0.1, ensNames: 0, oldestEnsAt: null }, degraded: false },
    { name: "very large floor", floor: 10n ** 30n + 7n, tier: "dangerous", signals: null, degraded: false },
  );

  writeFixture("pricing", "quote (pricing.ts): amount = floor * multiplierBps / 10000 (integer division), multiplierBps clamped to [10000, 100000]; important is free. Floats in the multiplier math use JS Math.round semantics. floor and amount are decimal strings of 18-decimal USDC base units.", {
    config: { now: NOW, oneBps: 10_000, ceilingBps: 100_000, yearSeconds: YEAR },
    cases: cases.map((entry) => {
      const output = atTime(NOW, () => quote(entry.floor, entry.tier, entry.signals, entry.degraded));
      return {
        name: entry.name,
        input: { floor: entry.floor.toString(), tier: entry.tier, signals: entry.signals, degraded: entry.degraded },
        output: { ...output, amount: output.amount.toString(), floor: output.floor.toString() },
      };
    }),
  });
}

async function main(): Promise<void> {
  mkdirSync(OUT_DIR, { recursive: true });
  await captureSignQuote();
  captureMessageId();
  captureWalletNonce();
  captureCodeHash();
  captureHashSignal();
  captureRpSignRequest();
  captureChallengeEmail();
  capturePricing();
}

main().catch((cause: unknown) => {
  console.error(cause);
  process.exitCode = 1;
});

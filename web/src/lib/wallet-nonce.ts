import { createHmac, hkdfSync, randomBytes, timingSafeEqual } from "node:crypto";
import { required } from "./env";
import { now } from "./time";

/// Server-only. `node:crypto` and the secret behind it must never travel
/// toward a browser bundle, which is why this lives beside `wallet-proof.ts`
/// rather than inside it: that module is imported by `"use client"`
/// components and says so at its own head. Nothing here may be imported from
/// there, and the dependency only ever runs the other way — `auth.ts`, which
/// only routes import, pulls both in.

/// How long a minted nonce is worth anything.
///
/// Deliberately much shorter than `CLOCK_SKEW_TOLERANCE_SECONDS` in `auth.ts`,
/// and unrelated to it. That tolerance is wide because the timestamp inside a
/// statement comes from the *signer's* device, whose clock is not ours; this
/// window is judged entirely on the server's own clock against a value the
/// server itself minted, so it can be as tight as a signature prompt takes.
/// Two minutes covers reading a wallet prompt and tapping approve on a phone.
export const WALLET_NONCE_TTL_SECONDS = 120;

/// Names what this MAC is *for*, inside the MAC. Without it a value
/// authenticated for some other purpose under the same key could be presented
/// here as a nonce.
const PURPOSE = "postage-wallet-nonce";

const RANDOM_BYTES = 16;

/// The key this module signs nonces with, derived from `MESSAGE_ID_SECRET`
/// rather than configured as a secret of its own.
///
/// One secret serving two purposes is usually the mistake. This is the other
/// thing: one piece of *key material* deriving two independent keys, which is
/// exactly what a domain-separating HKDF label is for. `postage:wallet-nonce`
/// and `postage:verification-code` (`verification.ts`) cannot collide, and
/// neither can reproduce the raw `MESSAGE_ID_SECRET` that `quote.ts` HMACs
/// message ids with — recovering any of the three from another means breaking
/// SHA-256.
///
/// Derived rather than added as a new env var because a new one would have to
/// be set in Vercel *before* the deploy that reads it, and `required()` throws
/// when it is not: signing in would fail closed in production for everyone
/// until an operator noticed. This needs no coordination at all.
function key(): Buffer {
  const root = Buffer.from(required("MESSAGE_ID_SECRET"), "utf8");
  return Buffer.from(hkdfSync("sha256", root, "", "postage:wallet-nonce", 32));
}

/// The tag over the three fields a nonce carries. `|` separates them and none
/// of the three can contain one — an address and a hex string are both drawn
/// from `[0-9a-fx]`, the expiry from digits — so no two different triples
/// share a preimage.
function tag(wallet: string, expiresAtText: string, random: string): string {
  const preimage = [PURPOSE, wallet.toLowerCase(), expiresAtText, random].join("|");
  return createHmac("sha256", key()).update(preimage).digest("hex");
}

/// A fresh nonce bound to one wallet and one short window.
///
/// Nothing is written down to mint this, which is the whole point of the
/// design: the route that hands them out is open, and a ledger written at
/// issue time would be a table anyone could grow without limit. The MAC is
/// what makes the value unforgeable instead, and the single-use record is
/// only taken later, when a *valid signature* has already been presented
/// under it (`spendWalletNonce`, called from `provesWallet`).
export function mintWalletNonce(wallet: string): string {
  const expiresAtText = String(now() + WALLET_NONCE_TTL_SECONDS);
  const random = randomBytes(RANDOM_BYTES).toString("hex");
  return `${expiresAtText}.${random}.${tag(wallet, expiresAtText, random)}`;
}

/// Whether this nonce is one we minted, for this wallet, still inside its
/// window — and if so, when it stops being one.
///
/// The expiry is returned rather than merely checked because the caller has to
/// write it down when it spends the nonce, and re-deriving it there would mean
/// parsing this string twice with two chances to disagree.
export function verifyWalletNonce(nonce: string, wallet: string): number | null {
  const parts = nonce.split(".");
  if (parts.length !== 3) return null;

  const [expiresAtText, random, offered] = parts;
  if (!/^\d+$/.test(expiresAtText)) return null;
  if (!/^[0-9a-f]+$/.test(random) || random.length !== RANDOM_BYTES * 2) return null;

  // The tag is computed over the text exactly as it arrived, so a re-serialised
  // expiry can never drift from the one that was authenticated.
  if (!sameTag(offered, tag(wallet, expiresAtText, random))) return null;

  const expiresAt = Number(expiresAtText);
  if (expiresAt <= now()) return null;
  return expiresAt;
}

/// Compared without leaking where two tags first differ. A mismatch in length
/// is refused before `timingSafeEqual`, which throws on unequal buffers, and
/// says nothing an attacker could not measure from the regexes above anyway.
function sameTag(offered: string, expected: string): boolean {
  const a = Buffer.from(offered, "utf8");
  const b = Buffer.from(expected, "utf8");
  if (a.length !== b.length) return false;
  return timingSafeEqual(a, b);
}

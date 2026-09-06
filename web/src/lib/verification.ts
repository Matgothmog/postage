import { createHmac, hkdfSync, randomInt, timingSafeEqual } from "node:crypto";
import { required } from "./env";

/// Long enough to walk to another device, short enough that a guessed code is
/// not worth the wait.
export const CODE_TTL_SECONDS = 15 * 60;

/// A six digit code is only safe because guessing is capped. Past this many
/// wrong answers the claim has to be started again, which mints a new code.
export const MAX_ATTEMPTS = 5;

/// Derived rather than configured, so verification codes and onchain message
/// ids never share a key while the operator only has to hold one secret.
function key(): Buffer {
  const root = Buffer.from(required("MESSAGE_ID_SECRET"), "utf8");
  return Buffer.from(hkdfSync("sha256", root, "", "postage:verification-code", 32));
}

export function generateCode(): string {
  return randomInt(0, 1_000_000).toString().padStart(6, "0");
}

/// Bound to the handle, so a code mailed for one claim cannot settle another.
export function hashCode(handle: string, code: string): string {
  return createHmac("sha256", key()).update(`${handle.toLowerCase()}:${code}`).digest("hex");
}

export function codeMatches(handle: string, code: string, expected: string): boolean {
  const offered = Buffer.from(hashCode(handle, code), "hex");
  const stored = Buffer.from(expected, "hex");
  if (offered.length !== stored.length) return false;
  return timingSafeEqual(offered, stored);
}

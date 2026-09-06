import { type Hex, isAddress } from "viem";
import { publicClient } from "./client";

export { claimStatement, readStatement } from "./statements";

/// How long a signed statement stays good for. Long enough to cover a slow
/// signature prompt, short enough that one lifted from a log is worthless.
const FRESHNESS_SECONDS = 5 * 60;

/// Proves the caller holds the wallet they claim, rather than merely knowing
/// its address. Wallet addresses are public — they are indexed, and every
/// sender who was ever gated is handed one — so a request naming a wallet is
/// not evidence of anything without this.
export async function provesWallet(
  wallet: string | null,
  issuedAt: number,
  signature: string | null,
  statement: (issuedAt: number) => string
): Promise<boolean> {
  if (!wallet || !isAddress(wallet) || !signature) return false;

  const age = Math.floor(Date.now() / 1000) - issuedAt;
  if (!Number.isFinite(issuedAt) || age < -FRESHNESS_SECONDS || age > FRESHNESS_SECONDS) return false;

  try {
    return await publicClient.verifyMessage({
      address: wallet as Hex,
      message: statement(issuedAt),
      signature: signature as Hex,
    });
  } catch {
    return false;
  }
}

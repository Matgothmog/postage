import { type Hex, isAddress } from "viem";
import { publicClient } from "./client";
import { readIdentity } from "./privy";

export { claimStatement, confirmStatement, readStatement } from "./statements";

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

/// Whether this request was made by someone holding a particular wallet.
///
/// Two things count as proof and the app issues both. Privy's identity token
/// lists the wallets it minted for whoever is signed in, and a signature over a
/// statement naming the wallet is what a session with no such token can offer.
/// Neither is the address itself: a wallet address is public, indexed onchain,
/// and handed to every sender who was ever gated, so a request that merely names
/// one has proved nothing.
export async function holdsWallet(
  request: Request,
  wallet: string,
  statement: (issuedAt: number) => string
): Promise<boolean> {
  const wanted = wallet.toLowerCase();

  const identity = await readIdentity(request.headers.get("privy-id-token"));
  if (identity?.wallets.includes(wanted)) return true;

  // Checked against the wallet we are asking about before the signature is
  // verified, so a valid signature from some other wallet cannot stand in.
  const offered = request.headers.get("x-postage-wallet");
  if (!offered || offered.toLowerCase() !== wanted) return false;

  return await provesWallet(
    offered,
    Number(request.headers.get("x-postage-issued")),
    request.headers.get("x-postage-signature"),
    statement
  );
}

import { type Hex, isAddress, verifyMessage } from "viem";
import { readIdentity } from "./privy";
import { now } from "./time";
import { readProof } from "./wallet-proof";

export { claimStatement, confirmStatement, readStatement } from "./wallet-proof";

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

  const age = now() - issuedAt;
  if (!Number.isFinite(issuedAt) || age < -FRESHNESS_SECONDS || age > FRESHNESS_SECONDS) return false;

  try {
    // viem's standalone `verifyMessage` recovers the signer here and compares
    // addresses. Its client-bound namesake answers the same question with an
    // `eth_call` to whatever `ARC_RPC_URL` names — and unset, that is the public
    // endpoint the chain definition ships with. One that returns a nonzero word
    // makes every signature valid for every address, so whoever runs it would
    // decide who reads whose inbox. `mode: "eoa"` does not close that: it
    // recovers locally first but still falls through to the deployless ERC-6492
    // call when recovery does not match, which is exactly where a forged
    // signature lands.
    //
    // The price is that a contract wallet cannot sign, because ERC-1271 and
    // ERC-6492 both need the chain to answer. Nothing here ever holds one:
    // Privy is configured for email and passkey login with an embedded Ethereum
    // wallet and no smart-wallet package, so every signature reaching this line
    // came from a key. If one is ever linked, this refuses it as a failed login
    // rather than putting an RPC back in front of everyone's mail.
    return await verifyMessage({
      address: wallet,
      message: statement(issuedAt),
      signature: signature as Hex,
    });
  } catch {
    // A malformed signature makes recovery throw. That is a refusal, not a 500.
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
  const offered = readProof(request.headers);

  const identity = await readIdentity(offered.identityToken);
  if (identity?.wallets.includes(wanted)) return true;

  // Checked against the wallet we are asking about before the signature is
  // verified, so a valid signature from some other wallet cannot stand in.
  if (!offered.wallet || offered.wallet.toLowerCase() !== wanted) return false;

  return await provesWallet(offered.wallet, offered.issuedAt, offered.signature, statement);
}

import { type Hex, isAddress, verifyMessage } from "viem";
import { purgeSpentWalletNonces, spendWalletNonce } from "./db/spent-nonces";
import { readIdentity } from "./privy";
import { now } from "./time";
import { verifyWalletNonce } from "./wallet-nonce";
import { readProof, withNonce } from "./wallet-proof";

export { claimStatement, confirmStatement, readStatement } from "./wallet-proof";

/// How far a signed statement's timestamp may lag or lead the server's clock
/// and still be trusted. The forward half exists because the timestamp comes
/// from the signer's device, whose clock may run fast, not the server's; both
/// halves apply, so the actual window is ten minutes wide — this value in
/// each direction, not five minutes total. Long enough on the trailing edge
/// to cover a slow signature prompt, short enough that one lifted from a log
/// is worthless.
///
/// Left wide on purpose now that a nonce is what actually stops a replay. The
/// two answer different questions and must not be collapsed into one: this
/// one absorbs a *device* clock we do not control, while
/// `WALLET_NONCE_TTL_SECONDS` (`wallet-nonce.ts`, two minutes) is judged
/// entirely on our own. Narrowing this to match would lock out every signer
/// whose phone is a few minutes out, and would buy nothing — a captured
/// signature is refused by the spent-nonce record however fresh it looks.
const CLOCK_SKEW_TOLERANCE_SECONDS = 5 * 60;

/// Proves the caller holds the wallet they claim, rather than merely knowing
/// its address, and that they are proving it *now* rather than replaying a
/// signature somebody else collected. Wallet addresses are public — they are
/// indexed, and every sender who was ever gated is handed one — so a request
/// naming a wallet is not evidence of anything without this.
///
/// The nonce is what makes one signature answer one request. It is minted by
/// `/api/wallet-nonce` against this wallet, carried inside the signed text by
/// `withNonce`, and recorded as spent below the moment the signature checks
/// out — so a signature lifted off the wire, out of a log, or out of a
/// hostile page that asked the same wallet to sign is refused on its second
/// presentation rather than being good for the whole freshness window.
export async function provesWallet(
  wallet: string | null,
  issuedAt: number,
  signature: string | null,
  statement: (issuedAt: number) => string,
  nonce: string | null
): Promise<boolean> {
  if (!wallet || !isAddress(wallet) || !signature || !nonce) return false;

  const age = now() - issuedAt;
  if (
    !Number.isFinite(issuedAt) ||
    age < -CLOCK_SKEW_TOLERANCE_SECONDS ||
    age > CLOCK_SKEW_TOLERANCE_SECONDS
  )
    return false;

  // Checked before the signature, because it is the cheap half and because a
  // nonce minted for a different wallet must not be spent by this one.
  const expiresAt = verifyWalletNonce(nonce, wallet);
  if (expiresAt === null) return false;

  let signed: boolean;
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
    signed = await verifyMessage({
      address: wallet,
      message: withNonce(statement(issuedAt), nonce),
      signature: signature as Hex,
    });
  } catch {
    // A malformed signature makes recovery throw. That is a refusal, not a 500.
    return false;
  }
  if (!signed) return false;

  // Spent last, and only for a signature that already verified, so nobody can
  // burn a stranger's nonce by posting rubbish under it.
  return await spendNonce(nonce, expiresAt);
}

/// Records the nonce as answered, and refuses the proof if it cannot.
///
/// Not knowing whether this nonce has already been used is not a reason to let
/// the signature through — the same fail-closed reading `spendIssuedContext`
/// (`api/world/verify/route.ts`) takes of the same question. The caller turns
/// this into a "sign in again", which is the wrong message for a database
/// outage; the log line is what tells an operator which of the two it was.
/// Nothing about the nonce itself is logged: it is a live credential until the
/// row lands, and this is the path where the row did not land.
async function spendNonce(nonce: string, expiresAt: number): Promise<boolean> {
  try {
    // Dropped on the way past, like the claim-send and classification purges,
    // rather than left to grow a row per sign-in forever. Here rather than in
    // the route that mints, which touches no database at all and must not
    // start: it is open to anyone.
    await purgeSpentWalletNonces();
    return await spendWalletNonce(nonce, expiresAt);
  } catch (cause) {
    console.error("spent wallet nonce ledger unavailable", {
      reason: cause instanceof Error ? cause.message : cause,
    });
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
///
/// The identity-token branch is the one thing here a nonce does not cover. It
/// is answered before any signature is looked at, and the token is a bearer
/// credential Privy issues: anyone holding a copy is that session until it
/// expires, replay or not. Nothing in this file can change that — it is
/// Privy's credential and Privy's lifetime.
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

  return await provesWallet(
    offered.wallet,
    offered.issuedAt,
    offered.signature,
    statement,
    offered.nonce
  );
}

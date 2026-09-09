import { isAddress } from "viem";
import { WALLET_NONCE_TTL_SECONDS, mintWalletNonce } from "@/lib/wallet-nonce";

/// Hands a browser the one-time value it is about to ask a wallet to sign.
///
/// Open, and it has to be: a session with no Privy identity token has nothing
/// to prove until it holds a nonce to sign, so requiring proof here would be
/// circular. That makes the shape of this route the whole of its security
/// argument.
///
/// A nonce is worth nothing to whoever asks for one. It names a wallet the
/// caller already knew — addresses are public — and it opens nothing on its
/// own: the only thing it can be turned into is a request that also carries a
/// signature from that wallet's key, which is precisely what the caller does
/// not have. Anyone can mint one for anyone; nobody can answer one for a key
/// they do not hold.
///
/// And minting touches no database, no key material the response reveals, and
/// no shared counter, so there is nothing here to exhaust. That is the reason
/// this design was preferred over a table of issued nonces: an open route that
/// writes a row per call is a table any stranger can grow without limit. The
/// single-use record is taken on the way back in instead (`spendWalletNonce`),
/// where a valid signature has already been presented.
export async function POST(request: Request) {
  let body: unknown;
  try {
    body = await request.json();
  } catch {
    return Response.json({ error: "Body must be JSON" }, { status: 400 });
  }

  const { wallet } = body as { wallet?: unknown };
  if (typeof wallet !== "string" || !isAddress(wallet)) {
    return Response.json({ error: "A valid wallet is required" }, { status: 400 });
  }

  return Response.json({ nonce: mintWalletNonce(wallet), expiresIn: WALLET_NONCE_TTL_SECONDS });
}

import type { useSignMessage } from "@privy-io/react-auth";
import { handleOf, normalizeHandle } from "@/lib/handle";
import { now } from "@/lib/time";
import {
  claimStatement,
  identityProof,
  requestWalletNonce,
  signedProof,
  withNonce,
} from "@/lib/wallet-proof";

export type SignMessage = ReturnType<typeof useSignMessage>["signMessage"];

export interface ClaimProgress {
  handle: string;
  destination: string;
  codeVerified: boolean;
  cloudflareVerified: boolean;
}

/// Headers proving the wallet to the server, by whichever route this session
/// has. Privy's identity token already names the wallets it minted; a session
/// without one signs a statement saying what it is about to do. The address
/// alone proves nothing — it is public, and every gated sender is handed one.
///
/// The nonce is fetched before the wallet is prompted, not after, because it
/// is part of what gets signed: a signature collected without one answers no
/// particular request and is refused by every reader.
export async function walletProof(
  identityToken: string | null,
  signMessage: SignMessage,
  wallet: string,
  statement: (issuedAt: number) => string
): Promise<HeadersInit> {
  if (identityToken) return identityProof(identityToken);

  const nonce = await requestWalletNonce(wallet);
  const issuedAt = now();
  const { signature } = await signMessage(
    { message: withNonce(statement(issuedAt), nonce) },
    { address: wallet, uiOptions: { showWalletUIs: false } }
  );
  return signedProof(wallet, issuedAt, signature, nonce);
}

/// Best effort. A signature is the only proof for someone whose session cannot
/// be read from an identity token, and redundant for everyone else, so a wallet
/// that refuses to sign is not on its own a reason to stop.
///
/// A nonce that cannot be fetched lands in the same `catch` as a refused
/// prompt, and for the same reason: what comes back either proves the wallet
/// or does not, and half a proof is no more use to the caller than none.
export async function signClaim(
  signMessage: SignMessage,
  claim: { handle: string; destination: string; wallet: string }
): Promise<{ issuedAt: number; signature: string; nonce: string } | null> {
  try {
    const nonce = await requestWalletNonce(claim.wallet);
    const issuedAt = now();
    const statement = claimStatement(claim.handle, claim.destination, claim.wallet, issuedAt);
    const { signature } = await signMessage(
      { message: withNonce(statement, nonce) },
      { address: claim.wallet, uiOptions: { showWalletUIs: false } }
    );
    return { issuedAt, signature, nonce };
  } catch {
    return null;
  }
}

/// Turns an email address into the handle its owner would probably have
/// picked, so the field arrives filled in rather than empty. The normalising
/// rule itself — charset, length, no doubled dots — lives in `@/lib/handle`
/// as the same rule the server enforces; this only supplies the
/// email-to-local-part step that is specific to a suggestion.
export function suggestHandle(email: string | null): string {
  return email ? normalizeHandle(handleOf(email)) : "";
}

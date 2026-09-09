/// The contract a wallet proof travels under, in one place.
///
/// A request proves the wallet it names one of two ways, and both ride on
/// headers. Which headers, and what gets signed underneath them, is a single
/// agreement between the browser that writes a proof and the route that reads
/// it. A disagreement between the two ends is not a type error and not a failing
/// build — it is a sign-in that quietly stops working for everybody — so the
/// names are declared here once, the writers build from them, the readers parse
/// from them, and the statements are re-exported alongside because changing what
/// is signed without changing what is verified breaks exactly the same thing.
export { claimStatement, confirmStatement, readStatement } from "./statements";

/// Nothing here may reach for the database, the clock's server-side callers, or
/// `node:crypto`: `"use client"` components import this module to build a proof,
/// so anything Node-only added below is dragged toward the browser bundle.

/// Privy's identity token. It already names the wallets Privy minted for
/// whoever is signed in, so a session holding one has nothing left to sign.
export const IDENTITY_TOKEN_HEADER = "privy-id-token";

/// What a session without an identity token sends instead: the wallet it claims,
/// when it said so, and a signature over the statement naming both.
export const WALLET_HEADER = "x-postage-wallet";
export const ISSUED_AT_HEADER = "x-postage-issued";
export const SIGNATURE_HEADER = "x-postage-signature";

/// A proof as it arrives, before any of it has been believed. Every field is
/// caller-controlled — a wallet address is public, and the timestamp is whatever
/// the sender typed — so this says only what was offered, never that it holds
/// up. `provesWallet` is what decides.
export interface OfferedProof {
  identityToken: string | null;
  wallet: string | null;
  /// Already through `Number()`, so a header that was never sent reads as 0 and
  /// one carrying junk reads as NaN. Both sit outside the freshness window, and
  /// the reader refuses them there rather than here.
  issuedAt: number;
  signature: string | null;
}

/// The proof a session that holds an identity token offers.
export function identityProof(identityToken: string): Record<string, string> {
  return { [IDENTITY_TOKEN_HEADER]: identityToken };
}

/// The proof every other session offers, from a signature it has already
/// collected. Collecting one means prompting a wallet, which is kept out of
/// here so that this half of the contract stays callable from a test and a
/// route, neither of which has a wallet to prompt.
export function signedProof(
  wallet: string,
  issuedAt: number,
  signature: string
): Record<string, string> {
  return {
    [WALLET_HEADER]: wallet,
    [ISSUED_AT_HEADER]: String(issuedAt),
    [SIGNATURE_HEADER]: signature,
  };
}

/// The other end of the two above, and the only place a route should learn
/// these names from.
export function readProof(headers: Headers): OfferedProof {
  return {
    identityToken: headers.get(IDENTITY_TOKEN_HEADER),
    wallet: headers.get(WALLET_HEADER),
    issuedAt: Number(headers.get(ISSUED_AT_HEADER)),
    signature: headers.get(SIGNATURE_HEADER),
  };
}

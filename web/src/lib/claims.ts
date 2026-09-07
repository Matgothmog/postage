import { type InboxClaim, claimByHandle, clearClaim, createInbox } from "./db";

export interface ClaimState {
  claim: InboxClaim;
  codeVerified: boolean;
  live: boolean;
}

/// Promotes a claim to a real inbox once the code has come back. Reading the
/// code proves the claimer can read the address they are pointing the handle
/// at, and since Postage delivers the mail itself rather than asking Cloudflare
/// to forward it, that is the whole of what has to be true.
export async function settleClaim(handle: string): Promise<ClaimState | null> {
  const claim = await claimByHandle(handle);
  if (!claim) return null;

  const codeVerified = claim.code_verified_at !== null;
  if (codeVerified) {
    await createInbox(claim.handle, claim.destination, claim.wallet);
    await clearClaim(claim.handle);
  }

  return { claim, codeVerified, live: codeVerified };
}

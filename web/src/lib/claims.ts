import { destinationStatus } from "./cloudflare";
import {
  type InboxClaim,
  claimByHandle,
  clearClaim,
  createInbox,
  markCloudflareVerified,
} from "./db";

export interface ClaimState {
  claim: InboxClaim;
  codeVerified: boolean;
  cloudflareVerified: boolean;
  live: boolean;
}

/// Brings a claim up to date against Cloudflare and promotes it to a real inbox
/// once both confirmations are in. Until then the handle resolves to nothing and
/// mail sent to it is refused as an unknown address.
///
/// `expires_at` is not consulted, and that is the point rather than an omission:
/// it bounds the emailed code, which was already checked against it when it was
/// entered. Cloudflare's link carries no deadline we set, so a claim whose code
/// went in on time has to go live whenever its owner gets round to clicking.
export async function settleClaim(handle: string): Promise<ClaimState | null> {
  const claim = await claimByHandle(handle);
  if (!claim) return null;

  let cloudflareVerified = claim.cf_verified_at !== null;
  if (!cloudflareVerified && claim.cf_address_id) {
    const addressId = claim.cf_address_id;
    const current = await destinationStatus(addressId);
    if (current?.verifiedAt != null) {
      await markCloudflareVerified(claim.handle, addressId, current.verifiedAt);
      cloudflareVerified = true;
    }
  }

  const codeVerified = claim.code_verified_at !== null;
  const live = codeVerified && cloudflareVerified;
  if (live) {
    await createInbox(claim.handle, claim.destination, claim.wallet);
    await clearClaim(claim.handle);
  }

  return { claim, codeVerified, cloudflareVerified, live };
}

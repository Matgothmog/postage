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
export async function settleClaim(handle: string): Promise<ClaimState | null> {
  const claim = await claimByHandle(handle);
  if (!claim) return null;

  let cloudflareVerified = claim.cf_verified_at !== null;
  if (!cloudflareVerified && claim.cf_address_id) {
    const current = await destinationStatus(claim.cf_address_id);
    if (current?.verifiedAt != null) {
      await markCloudflareVerified(claim.handle, current.verifiedAt);
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

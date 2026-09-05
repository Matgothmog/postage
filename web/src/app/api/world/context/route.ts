import { signRequest } from "@worldcoin/idkit-server";
import { required } from "@/lib/env";

/// World ID 4.0 requires every proof request to carry an rp_context signed by
/// the relying party. The signing key stays on the server; the browser only
/// ever sees the resulting signature, nonce and validity window.
export async function POST() {
  const signature = signRequest({
    signingKeyHex: required("WORLD_RP_SIGNING_KEY"),
    action: required("WORLD_ACTION"),
  });

  return Response.json({
    rp_id: required("WORLD_RP_ID"),
    nonce: signature.nonce,
    created_at: signature.createdAt,
    expires_at: signature.expiresAt,
    signature: signature.sig,
  });
}

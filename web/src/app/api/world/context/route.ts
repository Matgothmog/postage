import { signRequest } from "@worldcoin/idkit-server";
import { challengeByToken } from "@/lib/db/challenges";
import { purgeExpiredContexts, recordIssuedContext } from "@/lib/db/issued-contexts";
import { required } from "@/lib/env";
import { RP_CONTEXT_TTL_SECONDS } from "@/lib/rp-context";

/// World ID 4.0 requires every proof request to carry an rp_context signed by
/// the relying party. The signing key stays on the server; the browser only
/// ever sees the resulting signature, nonce and validity window.
///
/// What the signature says is that *we* asked for this proof, for this action,
/// as this relying party — and it says nothing at all about who asked us. So
/// signing on demand for anyone who posts an empty body hands a stranger our
/// name: they put the context in front of their own visitors, collect genuine
/// Selfie Check proofs issued to us, and present them here as their own. The
/// challenge token is what closes that. The sender always holds one, it is
/// already the capability every other route on this path is judged against,
/// and it is issued by us to one address for one message, so a context can
/// only be minted by someone we already invited to prove something — a bounded
/// number of times, against a challenge that is still open.
///
/// The token is checked before the signing key is even read, so an anonymous
/// caller learns nothing about how this deployment is configured.
export async function POST(request: Request) {
  let body: unknown;
  try {
    body = await request.json();
  } catch {
    return Response.json({ error: "Body must be JSON" }, { status: 400 });
  }

  const { token } = body as { token?: unknown };
  if (typeof token !== "string" || token === "") {
    return Response.json({ error: "A challenge token is required" }, { status: 400 });
  }

  const challenge = await challengeByToken(token);
  if (!challenge) return Response.json({ error: "Unknown challenge" }, { status: 404 });

  // Refused here as well as at /api/world/verify, which is where the sender is
  // told why. Signing for mail no lane will deliver spends the key on a proof
  // request whose only possible use is somewhere else.
  if (challenge.tier === "dangerous") {
    return Response.json({ error: "This challenge cannot be verified" }, { status: 403 });
  }

  // Loose, like every other settled check in this tree: a column read back as
  // undefined rather than null must close the gate, not open it.
  if (challenge.resolved_at != null) {
    return Response.json({ error: "This challenge has already been answered" }, { status: 409 });
  }

  const signingKeyHex = required("WORLD_RP_SIGNING_KEY");
  const action = required("WORLD_ACTION");
  const rpId = required("WORLD_RP_ID");

  const signature = signRequest({ signingKeyHex, action, ttl: RP_CONTEXT_TTL_SECONDS });

  await purgeExpiredContexts();
  const issued = await recordIssuedContext(
    token,
    signature.nonce,
    signature.createdAt,
    signature.expiresAt
  );
  // The signature is discarded rather than returned: it was cheap to make and
  // is worth nothing to us, but it is a bearer credential to whoever holds it.
  if (!issued) {
    return Response.json(
      { error: "Too many verification attempts. Wait a moment and try again." },
      { status: 429 }
    );
  }

  return Response.json({
    rp_id: rpId,
    nonce: signature.nonce,
    created_at: signature.createdAt,
    expires_at: signature.expiresAt,
    signature: signature.sig,
    // Returned so the browser reads the action IDKit is told to use from the
    // same signed payload rather than an independently-set env var of its
    // own — two copies of one value is what let them drift apart before.
    action,
  });
}

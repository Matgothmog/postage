import { settleClaim } from "@/lib/claims";
import { claimByHandle, countClaimAttempt, markCodeVerified } from "@/lib/db";
import { MAX_ATTEMPTS, codeMatches } from "@/lib/verification";

/// Polled while the user is on the confirmation screen, so the Cloudflare half
/// ticks over the moment they click the link in its email.
export async function GET(request: Request) {
  const handle = new URL(request.url).searchParams.get("handle");
  if (!handle) return Response.json({ error: "handle is required" }, { status: 400 });

  const state = await settleClaim(handle);
  if (!state) return Response.json({ error: "Nothing is being claimed here" }, { status: 404 });

  return Response.json({
    codeVerified: state.codeVerified,
    cloudflareVerified: state.cloudflareVerified,
    live: state.live,
  });
}

/// Confirms the claimer can read the address they pointed the handle at.
export async function POST(request: Request) {
  const { handle, code } = (await request.json()) as { handle?: string; code?: string };
  if (!handle || !code) return Response.json({ error: "handle and code are required" }, { status: 400 });

  const claim = await claimByHandle(handle);
  if (!claim) return Response.json({ error: "Nothing is being claimed here" }, { status: 404 });

  if (claim.expires_at <= Math.floor(Date.now() / 1000)) {
    return Response.json({ error: "That code has expired. Start again to get a new one" }, { status: 410 });
  }
  if (claim.attempts >= MAX_ATTEMPTS) {
    return Response.json({ error: "Too many wrong codes. Start again to get a new one" }, { status: 429 });
  }

  if (!codeMatches(claim.handle, code.trim(), claim.code_hash)) {
    await countClaimAttempt(claim.handle);
    const left = MAX_ATTEMPTS - claim.attempts - 1;
    return Response.json(
      { error: left > 0 ? `That code is wrong. ${left} tries left` : "That was the last try. Start again" },
      { status: 400 }
    );
  }

  await markCodeVerified(claim.handle);

  const state = await settleClaim(claim.handle);
  return Response.json({
    codeVerified: true,
    cloudflareVerified: state?.cloudflareVerified ?? false,
    live: state?.live ?? false,
  });
}

import { holdsWallet } from "@/lib/auth";
import { settleClaim } from "@/lib/claims";
import { ensureDestination } from "@/lib/cloudflare";
import {
  attachDestination,
  claimByHandle,
  consumeAttempt,
  markCodeVerified,
} from "@/lib/db/claims";
import { now } from "@/lib/time";
import { MAX_ATTEMPTS, codeMatches } from "@/lib/verification";
import { confirmStatement } from "@/lib/wallet-proof";

/// Polled while the user is on the confirmation screen, so the Cloudflare half
/// ticks over the moment they click the link in its email.
///
/// Deliberately unauthenticated. This poll is the only thing in the app that
/// ever calls `settleClaim` — no cron, no worker, nothing else asks Cloudflare
/// whether the destination was confirmed — so a proof requirement here would
/// have to be attached by `FinishClaim.tsx` on mount, before the visitor has
/// clicked anything, which means a signature prompt before they have agreed to
/// anything. The response below is kept to exactly the fields that poll uses
/// (see `FinishClaim.tsx`'s `apply`); handing back nothing else is the actual
/// mitigation. The residual risk — guessing a handle to learn its status — is
/// bounded account-wide by `CF_CHECK_INTERVAL_SECONDS`/`CF_CHECK_BUDGET`
/// (`@/lib/db/claims`), not by who is asking.
export async function GET(request: Request) {
  const handle = new URL(request.url).searchParams.get("handle");
  if (!handle) return Response.json({ error: "handle is required" }, { status: 400 });

  let state;
  try {
    state = await settleClaim(handle);
  } catch {
    // Cloudflare being unreachable is a reason to keep waiting, not to report
    // progress the caller has already made as undone. The poller ignores this.
    return Response.json({ error: "Waiting on Cloudflare" }, { status: 503 });
  }
  if (!state) return Response.json({ error: "Nothing is being claimed here" }, { status: 404 });

  return Response.json({
    codeVerified: state.codeVerified,
    cloudflareVerified: state.cloudflareVerified,
    live: state.live,
    stalled: state.stalled,
  });
}

/// Confirms the claimer can read the address they pointed the handle at.
///
/// The code proves the address; it does not prove who is claiming. Both are
/// needed, because a claim names a wallet and that wallet ends up holding the
/// inbox's `earnings` and its `setFloorPrice`. Without this the code was the
/// whole of it, so anyone who could get a stranger to type a code they had been
/// sent unasked completed a claim on a wallet the sender of that code chose: the
/// mail would arrive at the victim, and the money would not.
///
/// So the caller has to hold the wallet the claim was started with. The person
/// who started it does; whoever tricked someone into reading a code does not.
export async function POST(request: Request) {
  const { handle, code } = (await request.json()) as { handle?: string; code?: string };
  if (!handle || !code) return Response.json({ error: "handle and code are required" }, { status: 400 });

  const claim = await claimByHandle(handle);
  if (!claim) return Response.json({ error: "Nothing is being claimed here" }, { status: 404 });

  // Before the attempt is counted, so failing to prove the wallet cannot burn
  // the real claimer's guesses.
  const proven = await holdsWallet(request, claim.wallet, (at) =>
    confirmStatement(claim.handle, claim.wallet, at)
  );
  if (!proven) {
    return Response.json(
      { error: "Sign in with the wallet that started this claim to confirm it" },
      { status: 401 }
    );
  }

  if (claim.expires_at <= now()) {
    return Response.json({ error: "That code has expired. Start again to get a new one" }, { status: 410 });
  }
  if (!(await consumeAttempt(claim.handle, MAX_ATTEMPTS))) {
    return Response.json({ error: "Too many wrong codes. Start again to get a new one" }, { status: 429 });
  }

  if (!codeMatches(claim.handle, code.trim(), claim.code_hash)) {
    const left = MAX_ATTEMPTS - claim.attempts - 1;
    return Response.json(
      { error: left > 0 ? `That code is wrong. ${left} tries left` : "That was the last try. Start again" },
      { status: 400 }
    );
  }

  await markCodeVerified(claim.handle);

  // Only now does Cloudflare hear about the address, and the claimer does
  // nothing to make that happen. An address the account already knows comes
  // back verified, and this was the whole of signing up.
  try {
    const destination = await ensureDestination(claim.destination);
    await attachDestination(claim.handle, destination.id, destination.verifiedAt);
  } catch {
    // Handled by the poller, which will try again.
  }

  // The code is accepted and recorded either way. If Cloudflare cannot be
  // reached right now the claim simply waits, rather than telling someone the
  // code they just got right was wrong.
  let state = null;
  try {
    state = await settleClaim(claim.handle);
  } catch {
    state = null;
  }

  return Response.json({
    codeVerified: true,
    cloudflareVerified: state?.cloudflareVerified ?? false,
    live: state?.live ?? false,
    stalled: state?.stalled ?? false,
  });
}

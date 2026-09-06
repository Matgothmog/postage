import { ensureDestination } from "@/lib/cloudflare";
import { claimByHandle, inboxByHandle, inboxByWallet, startClaim } from "@/lib/db";
import { sendVerificationCode } from "@/lib/mail";
import { CODE_TTL_SECONDS, generateCode, hashCode } from "@/lib/verification";
import { isAddress } from "viem";

const HANDLE = /^[a-z0-9][a-z0-9._-]{1,30}$/;
const EMAIL = /^[^@\s]+@[^@\s]+\.[^@\s]+$/;

/// Where a signed-in user sees their inbox, or nothing if they have not
/// finished claiming one.
export async function GET(request: Request) {
  const wallet = new URL(request.url).searchParams.get("wallet");
  if (!wallet || !isAddress(wallet)) {
    return Response.json({ error: "A valid wallet is required" }, { status: 400 });
  }
  return Response.json({ inbox: await inboxByWallet(wallet) });
}

/// Starts a claim on handle@usepostage.com. Nothing is forwarded yet: the
/// handle only becomes an inbox once whoever asked for it has shown they can
/// read the address they pointed it at, which stops anyone aiming a Postage
/// address at a stranger's mailbox.
///
/// Two confirmations are needed and they are not interchangeable. Cloudflare
/// carries the mail and refuses to forward to an address it has not verified.
/// Our own code is what ties this claim to this person, because Cloudflare
/// destinations are shared across the whole account — an address someone else
/// verified already reads as verified to us.
export async function POST(request: Request) {
  const { handle, destination, wallet } = (await request.json()) as {
    handle?: string;
    destination?: string;
    wallet?: string;
  };

  if (!handle || !HANDLE.test(handle.toLowerCase())) {
    return Response.json({ error: "Pick 2-31 characters: letters, digits, dot, dash" }, { status: 400 });
  }
  if (!destination || !EMAIL.test(destination)) {
    return Response.json({ error: "A valid destination address is required" }, { status: 400 });
  }
  if (!wallet || !isAddress(wallet)) {
    return Response.json({ error: "A valid wallet is required" }, { status: 400 });
  }

  const taken = await unavailableTo(handle, wallet);
  if (taken) return Response.json({ error: taken }, { status: 409 });

  let cloudflare;
  try {
    cloudflare = await ensureDestination(destination);
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : "Cloudflare rejected the address";
    return Response.json({ error: detail }, { status: 502 });
  }

  const code = generateCode();
  try {
    await sendVerificationCode(destination, handle.toLowerCase(), code);
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : "Could not send the code";
    return Response.json({ error: detail }, { status: 502 });
  }

  await startClaim({
    handle: handle.toLowerCase(),
    destination,
    wallet,
    code_hash: hashCode(handle, code),
    expires_at: Math.floor(Date.now() / 1000) + CODE_TTL_SECONDS,
    cf_address_id: cloudflare.id,
    cf_verified_at: cloudflare.verifiedAt,
  });

  return Response.json({
    status: "pending",
    handle: handle.toLowerCase(),
    destination,
    codeVerified: false,
    // Already true when someone else on the account verified this address
    // before. It still forwards nothing until the code confirms this claim.
    cloudflareVerified: cloudflare.verifiedAt !== null,
    expiresIn: CODE_TTL_SECONDS,
  });
}

/// A handle is free if nobody owns it and nobody else is part way through
/// claiming it. An abandoned claim releases it when the code expires.
async function unavailableTo(handle: string, wallet: string): Promise<string | null> {
  const owner = await inboxByHandle(handle);
  if (owner && owner.wallet !== wallet.toLowerCase()) return "That handle is taken";

  const claim = await claimByHandle(handle);
  if (!claim || claim.wallet === wallet.toLowerCase()) return null;
  if (claim.expires_at > Math.floor(Date.now() / 1000)) {
    return "Someone is claiming that handle right now. Try again in a few minutes";
  }
  return null;
}

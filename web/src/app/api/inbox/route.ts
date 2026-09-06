import { isAddress } from "viem";
import { claimStatement, provesWallet, readStatement } from "@/lib/auth";
import { ensureDestination } from "@/lib/cloudflare";
import {
  claimByHandle,
  inboxByHandle,
  inboxByWallet,
  recentClaimsTo,
  recordClaimSend,
  startClaim,
} from "@/lib/db";
import { sendVerificationCode } from "@/lib/mail";
import { CODE_TTL_SECONDS, generateCode, hashCode } from "@/lib/verification";

const HANDLE = /^[a-z0-9]([a-z0-9._-]*[a-z0-9])?$/;
const EMAIL = /^[^@\s]+@[^@\s]+\.[^@\s]+$/;
const OUR_DOMAIN = "usepostage.com";

/// Addresses the service itself relies on. A user holding `hello` would receive
/// every reply and bounce to our own verification mail, which is where other
/// people's codes are quoted; postmaster and abuse are required to reach us by
/// RFC 2142 rather than a user.
const RESERVED = new Set([
  "hello", "postmaster", "abuse", "admin", "administrator", "noreply", "no-reply",
  "support", "help", "info", "security", "billing", "mailer-daemon", "webmaster", "postage",
]);

/// One address can only be asked to confirm so often. Each claim mails it twice,
/// from two senders whose reputation we depend on, so without this an
/// unauthenticated loop is an email bomb aimed at anyone.
const MAX_CLAIMS_PER_DESTINATION = 3;
const THROTTLE_WINDOW_SECONDS = 60 * 60;

/// Where a signed-in user sees their inbox. The row holds the address they
/// actually read, so it is returned only to someone who can prove they hold the
/// wallet rather than to anyone who knows it.
export async function GET(request: Request) {
  const wallet = request.headers.get("x-postage-wallet");
  const issuedAt = Number(request.headers.get("x-postage-issued"));
  const signature = request.headers.get("x-postage-signature");

  if (!wallet || !isAddress(wallet)) {
    return Response.json({ error: "A valid wallet is required" }, { status: 400 });
  }
  if (!(await provesWallet(wallet, issuedAt, signature, (at) => readStatement(wallet, at)))) {
    return Response.json({ error: "Sign in again to read this inbox" }, { status: 401 });
  }

  return Response.json({ inbox: await inboxByWallet(wallet) });
}

/// Starts a claim on handle@usepostage.com. Nothing is forwarded yet, and the
/// handle only becomes an inbox once two separate things are true.
///
/// The signature proves who is asking. Without it the wallet in the body is
/// just a public string — they are indexed onchain and handed to every sender
/// we ever gated — and anyone could repoint a live inbox at themselves by
/// naming its owner's wallet.
///
/// The emailed code proves the claimer can read the address they are pointing
/// the handle at. Cloudflare's own verification cannot stand in for it, because
/// destinations are shared across the whole account: an address someone else
/// verified already reads as verified to us.
export async function POST(request: Request) {
  const { handle, destination, wallet, issuedAt, signature } = (await request.json()) as {
    handle?: string;
    destination?: string;
    wallet?: string;
    issuedAt?: number;
    signature?: string;
  };

  const rejection = validate(handle, destination, wallet);
  if (rejection) return Response.json({ error: rejection }, { status: 400 });

  const name = handle!.toLowerCase();
  const address = destination!.toLowerCase();

  const proved = await provesWallet(wallet!, Number(issuedAt), signature ?? null, (at) =>
    claimStatement(name, address, wallet!, at)
  );
  if (!proved) {
    return Response.json({ error: "Sign the request with the wallet you are claiming for" }, { status: 401 });
  }

  const taken = await unavailableTo(name, wallet!);
  if (taken) return Response.json({ error: taken }, { status: 409 });

  if ((await recentClaimsTo(address, THROTTLE_WINDOW_SECONDS)) >= MAX_CLAIMS_PER_DESTINATION) {
    return Response.json(
      { error: "That address has been asked to confirm too many times. Try again later" },
      { status: 429 }
    );
  }

  let cloudflare;
  try {
    cloudflare = await ensureDestination(address);
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : "Cloudflare rejected the address";
    return Response.json({ error: detail }, { status: 502 });
  }

  const code = generateCode();
  try {
    await sendVerificationCode(address, name, code);
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : "Could not send the code";
    return Response.json({ error: detail }, { status: 502 });
  }

  await recordClaimSend(address);
  await startClaim({
    handle: name,
    destination: address,
    wallet: wallet!,
    code_hash: hashCode(name, code),
    expires_at: Math.floor(Date.now() / 1000) + CODE_TTL_SECONDS,
    cf_address_id: cloudflare.id,
    cf_verified_at: cloudflare.verifiedAt,
  });

  return Response.json({
    status: "pending",
    handle: name,
    destination: address,
    codeVerified: false,
    // Already true when this address was verified on the account before. It
    // still forwards nothing until the code confirms this claim.
    cloudflareVerified: cloudflare.verifiedAt !== null,
    expiresIn: CODE_TTL_SECONDS,
  });
}

function validate(handle?: string, destination?: string, wallet?: string): string | null {
  const name = handle?.toLowerCase() ?? "";
  if (!name || name.length < 2 || name.length > 31 || !HANDLE.test(name)) {
    return "Pick 2-31 characters: letters, digits, dot, dash, not starting or ending with punctuation";
  }
  if (name.includes("..")) return "Two dots in a row is not a valid address";
  if (RESERVED.has(name)) return "That name is reserved";

  if (!destination || !EMAIL.test(destination)) return "A valid destination address is required";
  if (destination.toLowerCase().endsWith(`@${OUR_DOMAIN}`)) {
    return "Forward to an inbox you already read, not back to Postage";
  }

  if (!wallet || !isAddress(wallet)) return "A valid wallet is required";
  return null;
}

/// A handle is free if nobody owns it and nobody else is part way through
/// claiming it. An abandoned claim releases it, but one whose owner has already
/// answered the code does not — losing that would undo work they can see.
async function unavailableTo(handle: string, wallet: string): Promise<string | null> {
  const owner = await inboxByHandle(handle);
  if (owner && owner.wallet !== wallet.toLowerCase()) return "That handle is taken";

  const claim = await claimByHandle(handle);
  if (!claim || claim.wallet === wallet.toLowerCase()) return null;
  if (claim.code_verified_at !== null) return "That handle is taken";
  if (claim.expires_at > Math.floor(Date.now() / 1000)) {
    return "Someone is claiming that handle right now. Try again in a few minutes";
  }
  return null;
}

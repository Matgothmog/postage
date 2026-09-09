import { isAddress } from "viem";
import { provesWallet } from "@/lib/auth";
import { settleClaim } from "@/lib/claims";
import { ensureDestination } from "@/lib/cloudflare";
import {
  attachDestination,
  claimByHandle,
  markCodeVerified,
  purgeOldClaimSends,
  recentClaimsFrom,
  recentClaimsTo,
  recordClaimSend,
  startClaim,
} from "@/lib/db/claims";
import { inboxByHandle, inboxByWallet } from "@/lib/db/inboxes";
import { HANDLE_MAX_LENGTH, HANDLE_MIN_LENGTH, HANDLE_SHAPE, hasRepeatedDot, isOurs } from "@/lib/handle";
import { sendVerificationCode } from "@/lib/mail";
import { type PrivyIdentity, readIdentity } from "@/lib/privy";
import { now } from "@/lib/time";
import { CODE_TTL_SECONDS, generateCode, hashCode } from "@/lib/verification";
import { IDENTITY_TOKEN_HEADER, claimStatement, readProof, readStatement } from "@/lib/wallet-proof";

const EMAIL = /^[^@\s]+@[^@\s]+\.[^@\s]+$/;

/// Addresses the service itself relies on. A user holding `hello` would receive
/// every reply and bounce to our own verification mail, which is where other
/// people's codes are quoted; postmaster and abuse are required to reach us by
/// RFC 2142 rather than a user.
const RESERVED = new Set([
  "hello", "postmaster", "abuse", "admin", "administrator", "noreply", "no-reply",
  "support", "help", "info", "security", "billing", "mailer-daemon", "webmaster", "postage",
]);

/// One address can only be asked to confirm so often. Each claim makes at least
/// one stranger mail it, from a sender whose reputation we depend on, so without
/// this an unauthenticated loop is an email bomb aimed at anyone.
const MAX_CLAIMS_PER_DESTINATION = 3;

/// And one wallet can only start so many, whatever addresses it names. The
/// destination limit does not see this at all: naming a fresh address every time
/// passes it every time, and each claim that reaches a code registers a
/// Cloudflare destination — which the account has a hard cap on, which nothing
/// deletes, and which signup stops working for good on reaching.
///
/// Set above what anyone needs. Claiming a handle is one claim; getting the
/// address wrong twice and fixing it is three. It is a ceiling on farming, not a
/// budget anybody should feel.
const MAX_CLAIMS_PER_WALLET = 5;

const THROTTLE_WINDOW_SECONDS = 60 * 60;

/// A signed-in session that owns the wallet it is claiming for. This is the
/// short signup: Privy has already confirmed both the address and the wallet, so
/// neither has to be confirmed a second time.
async function session(request: Request, wallet: string): Promise<PrivyIdentity | null> {
  const identity = await readIdentity(request.headers.get(IDENTITY_TOKEN_HEADER));
  if (!identity?.wallets.includes(wallet.toLowerCase())) return null;
  return identity;
}

/// Where a signed-in user sees their inbox. The row holds the address they
/// actually read, so it is returned only to someone who can prove the wallet is
/// theirs rather than to anyone who knows it - wallets are public.
export async function GET(request: Request) {
  const offered = readProof(request.headers);

  const identity = await readIdentity(offered.identityToken);
  if (identity) {
    for (const wallet of identity.wallets) {
      const inbox = await inboxByWallet(wallet);
      if (inbox) return Response.json({ inbox });
    }
    return Response.json({ inbox: null });
  }

  const { wallet, issuedAt, signature, nonce } = offered;

  if (!wallet || !isAddress(wallet)) {
    return Response.json({ error: "A valid wallet is required" }, { status: 400 });
  }
  const proved = await provesWallet(
    wallet,
    issuedAt,
    signature,
    (at) => readStatement(wallet, at),
    nonce
  );
  if (!proved) {
    return Response.json({ error: "Sign in again to read this inbox" }, { status: 401 });
  }

  return Response.json({ inbox: await inboxByWallet(wallet) });
}

/// Starts a claim on the handle. Nothing is forwarded yet, and the
/// handle only becomes an inbox once two separate things are true.
///
/// Somebody has to prove they are asking for their own wallet, and that they can
/// read the address the handle will point at. Privy's identity token carries
/// both already - it names the wallet it minted and the address it confirmed at
/// sign-in - so a signed-in user claiming their own address answers nothing:
/// they pick a handle and the only thing left is Cloudflare's own link.
///
/// Everyone else takes the long way. A signature proves the wallet, because
/// otherwise the wallet in the body is a public string anyone could name, and an
/// emailed code proves the address, because Cloudflare's verification cannot
/// stand in for it: destinations are shared across the whole account, so one
/// somebody else verified already reads as verified to us.
export async function POST(request: Request) {
  const { handle, destination, wallet, issuedAt, signature, nonce } = (await request.json()) as {
    handle?: string;
    destination?: string;
    wallet?: string;
    issuedAt?: number;
    signature?: string;
    /// Carried in the body rather than a header on this one route, because the
    /// claim it belongs to is a body already — `PickHandle.tsx` spreads
    /// `signClaim`'s whole result in beside the handle it is claiming.
    nonce?: string;
  };

  if (!wallet || !isAddress(wallet)) {
    return Response.json({ error: "A valid wallet is required" }, { status: 400 });
  }

  const signedIn = await session(request, wallet);
  const name = handle?.toLowerCase() ?? "";
  const address = (destination ?? signedIn?.email ?? "").toLowerCase();

  const rejection = validate(name, address);
  if (rejection) return Response.json({ error: rejection }, { status: 400 });

  if (!signedIn) {
    const proved = await provesWallet(
      wallet,
      Number(issuedAt),
      signature ?? null,
      (at) => claimStatement(name, address, wallet, at),
      nonce ?? null
    );
    if (!proved) {
      return Response.json({ error: "Sign the request with the wallet you are claiming for" }, { status: 401 });
    }
  }

  const taken = await unavailableTo(name, wallet);
  if (taken) return Response.json({ error: taken }, { status: 409 });

  // Dropped on the way past, so the table both counts run over stays the size of
  // the window rather than growing a row per signup attempt forever.
  await purgeOldClaimSends(THROTTLE_WINDOW_SECONDS);

  if ((await recentClaimsTo(address, THROTTLE_WINDOW_SECONDS)) >= MAX_CLAIMS_PER_DESTINATION) {
    return Response.json(
      { error: "That address has been asked to confirm too many times. Try again later" },
      { status: 429 }
    );
  }
  if ((await recentClaimsFrom(wallet, THROTTLE_WINDOW_SECONDS)) >= MAX_CLAIMS_PER_WALLET) {
    return Response.json(
      { error: "That wallet has claimed too many addresses this hour. Try again later" },
      { status: 429 }
    );
  }

  // Counted before anything is sent rather than after. A claim that mails the
  // address and then fails has still mailed it, and a throttle that only counts
  // successes does not throttle that.
  await recordClaimSend(address, wallet);

  const alreadyRead = signedIn?.email === address;
  if (!alreadyRead) {
    const code = generateCode();
    try {
      await sendVerificationCode(address, name, code);
    } catch (cause) {
      const detail = cause instanceof Error ? cause.message : "Could not send the code";
      return Response.json({ error: detail }, { status: 502 });
    }

    await startClaim({
      handle: name,
      destination: address,
      wallet,
      code_hash: hashCode(name, code),
      expires_at: now() + CODE_TTL_SECONDS,
      // Cloudflare is not told about this address until the code comes back.
      // Registering now would make it send its own mail at the same moment as
      // ours, so the claimer would face two emails and two instructions at once.
      cf_address_id: null,
      cf_verified_at: null,
    });

    return Response.json({
      status: "pending",
      handle: name,
      destination: address,
      codeVerified: false,
      cloudflareVerified: false,
      live: false,
      expiresIn: CODE_TTL_SECONDS,
    });
  }

  await startClaim({
    handle: name,
    destination: address,
    wallet,
    // Not null, and a claim that never needed a code must still fill it. The
    // hash of one nobody was sent can never be matched, so the confirm route
    // stays shut rather than being left open to anything.
    code_hash: hashCode(name, generateCode()),
    expires_at: now() + CODE_TTL_SECONDS,
    cf_address_id: null,
    cf_verified_at: null,
  });
  await markCodeVerified(name);

  try {
    const registered = await ensureDestination(address);
    await attachDestination(name, registered.id, registered.verifiedAt);
  } catch {
    // Handled by the poller, which will try again.
  }

  const state = await settleClaim(name).catch(() => null);

  return Response.json({
    status: state?.live ? "live" : "pending",
    handle: name,
    destination: address,
    codeVerified: true,
    cloudflareVerified: state?.cloudflareVerified ?? false,
    live: state?.live ?? false,
  });
}

function validate(handle: string, destination: string): string | null {
  if (
    !handle ||
    handle.length < HANDLE_MIN_LENGTH ||
    handle.length > HANDLE_MAX_LENGTH ||
    !HANDLE_SHAPE.test(handle)
  ) {
    return "Pick 2-31 characters: letters, digits, dot, dash, not starting or ending with punctuation";
  }
  if (hasRepeatedDot(handle)) return "Two dots in a row is not a valid address";
  if (RESERVED.has(handle)) return "That name is reserved";

  if (!destination || !EMAIL.test(destination)) return "A valid destination address is required";
  if (isOurs(destination)) return "Forward to an inbox you already read, not back to Postage";
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
  if (claim.expires_at > now()) {
    return "Someone is claiming that handle right now. Try again in a few minutes";
  }
  return null;
}

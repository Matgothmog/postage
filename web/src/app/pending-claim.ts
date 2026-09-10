import type { ClaimProgress } from "./claim-inbox-helpers";

/// Where a claim in progress is remembered between page loads.
///
/// It used to live in component state alone, so reloading while waiting on
/// Cloudflare's email dropped the claimer back onto an empty form — and
/// starting again spends one of the five claims a wallet is allowed in an hour
/// (`MAX_CLAIMS_PER_WALLET`), after which signup stops working for them at all.
///
/// Nothing secret is kept here. The handle is about to be public, the
/// destination is the address whose owner is being asked to confirm it, and the
/// wallet below is a public identifier every gated sender is handed anyway. No
/// token, no signature, no session material.
export const PENDING_CLAIM_KEY = "postage.pending-claim";

/// The three methods of `localStorage` this module uses, and nothing else, so
/// the rules below can be exercised without a browser.
export interface ClaimStore {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/// A claim as it sits in storage. The wallet is the whole reason this shape
/// exists: there is one slot per browser and browsers get shared, so a claim
/// that does not say whose it is is a claim the next person to sign in here is
/// shown — someone else's handle, someone else's destination address, and a
/// code POST `/api/inbox/verify` refuses because the wallet does not match the
/// one the claim was started with.
export interface StoredClaim {
  wallet: string;
  claim: ClaimProgress;
}

/// `localStorage` is absent while rendering on the server and throws outright
/// in browsers set to refuse site data, so it is reached for once, here, and
/// every caller is handed a `null` it has to deal with instead of a throw it
/// would have to guard.
export function claimStore(): ClaimStore | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    // Site data refused. Nothing is remembered, and the claim still works for
    // as long as the tab stays open.
    return null;
  }
}

/// Addresses reach this file checksummed from Privy and lower-cased from the
/// server, and they name the same wallet either way. Nobody may lose their own
/// claim to a difference in capitalisation.
function sameWallet(left: string, right: string): boolean {
  return left.toLowerCase() === right.toLowerCase();
}

function isClaimProgress(value: unknown): value is ClaimProgress {
  if (typeof value !== "object" || value === null) return false;
  const claim = value as Partial<ClaimProgress>;
  return (
    typeof claim.handle === "string" &&
    claim.handle.length > 0 &&
    typeof claim.destination === "string" &&
    claim.destination.length > 0 &&
    typeof claim.codeVerified === "boolean" &&
    typeof claim.cloudflareVerified === "boolean"
  );
}

/// A record with no owner is refused rather than adopted by whoever is reading.
/// That is also what the shape written before claims were scoped looks like, so
/// an upgrade forgets one claim in progress — the cheap half of the trade
/// against showing it to a stranger.
function isStoredClaim(value: unknown): value is StoredClaim {
  if (typeof value !== "object" || value === null) return false;
  const stored = value as Partial<StoredClaim>;
  if (typeof stored.wallet !== "string" || stored.wallet.length === 0) return false;
  return isClaimProgress(stored.claim);
}

/// Anything that is not a claim this app wrote reads as no claim at all —
/// truncated JSON, a value from an older shape, a key some other script put
/// there. Rehydrating from half of one would show a strip for a claim the
/// server has never heard of.
export function readStoredClaim(store: ClaimStore | null): StoredClaim | null {
  if (!store) return null;
  try {
    const raw = store.getItem(PENDING_CLAIM_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (!isStoredClaim(parsed)) return null;
    return { wallet: parsed.wallet, claim: { ...parsed.claim } };
  } catch {
    // Unreadable or unparseable: the same as never having stored one.
    return null;
  }
}

/// The claim this session is allowed to see, which is only ever the one this
/// wallet stored. Anybody else's reads as absent — not shown, not polled, not
/// acted on — and a session with no wallet yet has nothing to match against, so
/// it sees nothing either.
export function claimFor(stored: StoredClaim | null, wallet: string | null): ClaimProgress | null {
  if (!stored || !wallet) return null;
  return sameWallet(stored.wallet, wallet) ? stored.claim : null;
}

export function writePendingClaim(
  store: ClaimStore | null,
  wallet: string | null,
  claim: ClaimProgress
): void {
  if (!store || !wallet) return;
  try {
    const stored: StoredClaim = { wallet, claim };
    store.setItem(PENDING_CLAIM_KEY, JSON.stringify(stored));
  } catch {
    // Out of quota, or site data refused. The claim survives in state for this
    // page load, which is exactly what it did before there was a store.
  }
}

export function clearPendingClaim(store: ClaimStore | null): void {
  if (!store) return;
  try {
    store.removeItem(PENDING_CLAIM_KEY);
  } catch {
    // A store that will not forget is one nothing was written to either.
  }
}

/// The one place the poll endpoint's URL is built. A handle is user input until
/// the server has taken it, and `isStoredClaim` only asks that it be a non-empty
/// string — one carrying `&` or `#` silently rewrites the query string it is
/// pasted into.
export function verifyClaimUrl(handle: string): string {
  return `/api/inbox/verify?handle=${encodeURIComponent(handle)}`;
}

/// What one answer from `/api/inbox/verify`'s GET means for the claim it was
/// asked about, from the status alone.
///
/// Both readers of that endpoint decide with this, because they used to
/// disagree: the reconcile below read a 404 as the claim being gone, while the
/// strip's four-second poll applied a 200 and dropped everything else on the
/// floor. A claim purged server-side when its code timed out therefore left a
/// strip on screen for as long as the tab stayed open.
///
/// "The server says this is gone" and "I could not reach the server" are the
/// two answers that must not be collapsed. Only the first is a reason to stop
/// showing a claim; forgetting one on the second costs its owner one of five
/// tries for a fault that is not theirs.
export type PollVerdict = "gone" | "read" | "wait";

export function pollVerdict(status: number): PollVerdict {
  if (status === 404) return "gone";
  return status >= 200 && status < 300 ? "read" : "wait";
}

/// Exactly the fields `/api/inbox/verify`'s GET answers with that a rehydrate
/// needs. `stalled` is deliberately not read here: the strip's own poll asks
/// again four seconds later and owns that state.
interface VerifyReply {
  codeVerified: boolean;
  cloudflareVerified: boolean;
  live: boolean;
}

export type PendingClaimOutcome =
  | { kind: "live" }
  | { kind: "pending"; claim: ClaimProgress }
  | { kind: "gone" }
  | { kind: "unknown" };

/// What became of a claim remembered from an earlier page load. The endpoint
/// the strip already polls answers it, so nothing new is asked of the API.
///
/// The four outcomes are four different things to do, and collapsing any two
/// loses something. `live` finished while the page was closed. `gone` was
/// never there, or has already been promoted and cleared, and must not leave a
/// strip on screen for it. `unknown` is a wobble — Cloudflare unreachable, the
/// network down, a gateway page instead of JSON — where forgetting the claim
/// would cost its owner one of five tries for a fault that is not theirs.
export async function verifyPendingClaim(stored: ClaimProgress): Promise<PendingClaimOutcome> {
  try {
    const response = await fetch(verifyClaimUrl(stored.handle));
    const verdict = pollVerdict(response.status);
    if (verdict === "gone") return { kind: "gone" };
    if (verdict === "wait") return { kind: "unknown" };

    const state = (await response.json()) as VerifyReply;
    if (state.live) return { kind: "live" };

    return {
      kind: "pending",
      claim: {
        ...stored,
        // Merged rather than replaced, the same way the strip's own poll merges
        // a tick: a step that has been confirmed cannot become unconfirmed, and
        // treating one as undone would put the code field back in front of
        // somebody who has already spent their code.
        codeVerified: stored.codeVerified || state.codeVerified,
        cloudflareVerified: stored.cloudflareVerified || state.cloudflareVerified,
      },
    };
  } catch {
    return { kind: "unknown" };
  }
}

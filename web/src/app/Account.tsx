"use client";

import { useIdentityToken, usePrivy, useSignMessage, useWallets } from "@privy-io/react-auth";
import Link from "next/link";
import {
  type ReactNode,
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Callout, Shell, primaryButton, quietButton } from "@/components/chrome";
import { causeMessage } from "@/lib/errors";
import { isValidHandle } from "@/lib/handle";
import { IDENTITY_TOKEN_HEADER, readStatement } from "@/lib/wallet-proof";
import {
  type ClaimProgress,
  type SignMessage,
  signClaim,
  suggestHandle,
  walletProof,
} from "./claim-inbox-helpers";
import { ClaimSetup, ClaimStrip, HandleField } from "./ClaimStrip";
import { type Inbox, InboxPanel } from "./InboxPanel";
import {
  type StoredClaim,
  claimFor,
  claimStore,
  clearPendingClaim,
  readStoredClaim,
  verifyPendingClaim,
  writePendingClaim,
} from "./pending-claim";

/// Decides which of the two things this address is: the page a stranger reads,
/// or the inbox its owner manages. The first arrives already rendered and is
/// simply passed through, so nobody waits on a wallet library to read a pitch.
export function Account({ landing }: { landing: ReactNode }) {
  const { ready, authenticated, login, logout, user } = usePrivy();
  const { identityToken } = useIdentityToken();
  const { signMessage } = useSignMessage();
  const { wallets } = useWallets();
  const wallet = wallets[0];

  const [inbox, setInbox] = useState<Inbox | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<InboxErrorKind | null>(null);

  // The row holds the address the user actually reads, so the server will not
  // hand it over on the strength of a wallet address alone - those are public.
  // Privy's identity token is that proof, already signed and already in hand;
  // signing a statement is the same proof made the long way, for a session that
  // has no identity token to offer.
  //
  // A non-ok response is never treated as "no inbox" - that would show someone
  // who already claimed one the claim flow again, inviting them to fight
  // themselves for their own handle. A refused proof needs a sign-in prompt;
  // anything else means we could not ask at all, which needs different words
  // and a retry.
  //
  // Nothing here clears a previous error up front, because a fetch that has not
  // answered yet says nothing about the last one. It is cleared on the answer
  // instead, and it has to be: a retry that succeeds for someone with no inbox
  // sets `inbox` to the null it already held, so leaving the error standing
  // would pin them on the error screen and keep the claim flow out of reach for
  // the rest of the session.
  //
  // A thrown fetch or signature error is left to propagate rather than caught
  // here, so every caller decides for itself what a failure means to it - the
  // mount effect below records it as "network", the same as a bad response.
  const refresh = useCallback(async () => {
    const address = wallet?.address;
    if (!address) return;
    try {
      const response = await fetch("/api/inbox", {
        headers: await walletProof(identityToken, signMessage, address, (at) =>
          readStatement(address, at)
        ),
      });
      if (response.ok) {
        setInbox(((await response.json()) as { inbox: Inbox | null }).inbox);
        setError(null);
      } else {
        setError(refusedTheProof(response.status) ? "unauthorized" : "network");
      }
    } finally {
      setLoaded(true);
    }
  }, [identityToken, wallet?.address, signMessage]);

  useEffect(() => {
    refresh().catch(() => setError("network"));
  }, [refresh]);

  const retry = useCallback(() => {
    refresh().catch(() => setError("network"));
  }, [refresh]);

  const claim = useClaimFlow({
    wallet: wallet?.address ?? null,
    email: user?.email?.address?.toLowerCase() ?? null,
    identityToken,
    signMessage,
    login,
    inbox,
    inboxError: error,
    loaded,
    onLive: retry,
  });

  if (!ready || !authenticated) {
    return (
      <ClaimIntentContext.Provider value={claim}>
        <Shell actions={<SignedOutActions login={login} disabled={!ready} />}>{landing}</Shell>
      </ClaimIntentContext.Provider>
    );
  }

  const panel = pickPanel(wallet, loaded, error, inbox, claim.progress);

  return (
    <Shell
      actions={
        <>
          <Link href="/network" className={quietButton}>
            Ledger
          </Link>
          <button
            onClick={() => {
              // One browser, one storage slot. A claim left in it is the claim
              // the next person to sign in on this machine would be shown —
              // handle, destination address and all.
              claim.restart();
              void logout();
            }}
            className={quietButton}
          >
            Sign out
          </button>
        </>
      }
    >
      {error && (panel === "inbox" || panel === "strip") && (
        <RefreshFailed kind={error} onRetry={retry} />
      )}
      {panel === "error" && error ? (
        <InboxError kind={error} onRetry={retry} />
      ) : panel === "inbox" && inbox && wallet ? (
        <InboxPanel inbox={inbox} wallet={wallet.address} />
      ) : panel === "strip" && claim.progress && wallet ? (
        <ClaimStrip
          claim={claim.progress}
          wallet={wallet.address}
          onClaim={claim.advance}
          onLive={claim.finish}
          onRestart={claim.restart}
        />
      ) : panel === "setup" ? (
        <ClaimSetup
          handle={claim.handle}
          onHandle={claim.setHandle}
          destination={claim.destination}
          onDestination={claim.setDestination}
          email={claim.email}
          busy={claim.busy}
          error={claim.error}
          onSubmit={claim.submit}
        />
      ) : (
        <Waiting>One moment.</Waiting>
      )}
    </Shell>
  );
}

type Panel = "waiting" | "error" | "inbox" | "strip" | "setup";

/// Which of the five things a signed-in page shows, in the order they take
/// precedence over each other. Lifted out of the render so that order is one
/// readable list, and so it can be checked without a browser.
///
/// A claim in progress sits behind `inbox` rather than in front of it: the two
/// are never both true for long, and if they ever are, the finished inbox is
/// the truer answer.
///
/// Both sit in front of `error`, and that order is the fix. `refresh()` re-runs
/// whenever its identity changes — which includes every Privy identity-token
/// rotation — and it never clears `inbox` on a failure, only sets `error`.
/// Deciding the error first meant one blip replaced a live inbox, whose rows
/// were still sitting in state, with a full-screen "Can't reach us.", and hid
/// a claim halfway through behind an unrelated fault. What we already read is
/// still the truest thing we have; the failure is a notice over it
/// (`RefreshFailed`) and a hard screen only when there is nothing behind it.
function pickPanel(wallet: unknown, loaded: boolean, error: unknown, inbox: unknown, claim: unknown): Panel {
  if (!wallet || !loaded) return "waiting";
  if (inbox) return "inbox";
  if (claim) return "strip";
  if (error) return "error";
  return "setup";
}

/// Everything the claim needs to survive the moment it is made: the handle is
/// typed before there is a session, the POST cannot go out until there is one,
/// and the whole thing has to be findable again after a reload.
interface ClaimFlow {
  handle: string;
  setHandle: (handle: string) => void;
  destination: string;
  setDestination: (destination: string) => void;
  email: string | null;
  progress: ClaimProgress | null;
  busy: boolean;
  error: string | null;
  /// The hero's one gesture: remember the handle, then open the sign-in that
  /// the claim needs before it can be sent.
  start: () => void;
  submit: () => void;
  advance: (claim: ClaimProgress) => void;
  restart: () => void;
  finish: () => void;
}

const ClaimIntentContext = createContext<ClaimFlow | null>(null);

interface ClaimReply extends ClaimProgress {
  live: boolean;
  error?: string;
}

/// Starts the claim server-side. Outside the hook so what it does with a reply
/// reads without React in the way, and so a caller that gets an error gets it
/// as a throw rather than as a field it might forget to check.
async function postClaim(
  identityToken: string | null,
  signMessage: SignMessage,
  claim: { handle: string; destination: string; wallet: string }
): Promise<ClaimReply> {
  const proof = await signClaim(signMessage, claim);
  if (!identityToken && !proof) {
    throw new Error("Could not confirm the wallet is yours. Sign in again and retry");
  }

  const response = await fetch("/api/inbox", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      ...(identityToken ? { [IDENTITY_TOKEN_HEADER]: identityToken } : {}),
    },
    body: JSON.stringify({ ...claim, ...proof }),
  });
  const result = (await response.json()) as ClaimReply;
  if (!response.ok) throw new Error(result.error ?? "Could not claim that address");
  return result;
}

/// A claim the page has been asked to send. It is set from the click that asks
/// for it and never from an effect, so the button it came from goes busy in the
/// same render and the effect below is only ever the sending.
///
/// `"hero"` is the landing page's one-gesture ask. It names no destination
/// because there is no session yet to read one from, and it resolves to the
/// address Privy confirmed as soon as there is one.
type Outgoing = "hero" | { handle: string; destination: string };

/// What an ask resolves to, or null when it cannot be sent as it stands. The
/// hero's cannot until Privy hands over an address — a passkey session never
/// gets one, so that user picks a destination in the setup panel instead, and
/// that interaction cannot be removed.
function resolveOutgoing(
  outgoing: Outgoing | null,
  handle: string,
  email: string | null
): { handle: string; destination: string } | null {
  if (!outgoing) return null;
  const ask = outgoing === "hero" ? { handle, destination: email ?? "" } : outgoing;
  const name = ask.handle.trim().toLowerCase();
  const to = ask.destination.trim().toLowerCase();
  if (!isValidHandle(name) || !to.includes("@")) return null;
  return { handle: name, destination: to };
}

function useClaimFlow(input: {
  wallet: string | null;
  email: string | null;
  identityToken: string | null;
  signMessage: SignMessage;
  login: () => void;
  inbox: Inbox | null;
  inboxError: InboxErrorKind | null;
  loaded: boolean;
  onLive: () => void;
}): ClaimFlow {
  const { wallet, email, identityToken, signMessage, login, inbox, inboxError, loaded, onLive } =
    input;

  /// What the user actually typed, or null while they have typed nothing.
  /// Kept apart from what is shown so the suggestion below can fill the field
  /// once Privy hands over an address — which happens after the hero has
  /// already been filled in — without overwriting anybody's own answer.
  const [typedHandle, setTypedHandle] = useState<string | null>(null);
  const [typedDestination, setTypedDestination] = useState<string | null>(null);

  /// The storage slot as it was found, owner and all. Read during the first
  /// render rather than from an effect, so a reload while waiting on Cloudflare
  /// shows the strip straight away instead of flashing the claim form at
  /// somebody who has already filled it in — and so the hero's queued claim
  /// cannot go out before the claim already on record has been read back.
  ///
  /// Whose it is decides whether it is anything at all: `progress` below is
  /// null for every wallet but the one that stored it, so a claim left behind
  /// by whoever used this browser last is neither shown nor acted on.
  const [record, setRecord] = useState<StoredClaim | null>(() => readStoredClaim(claimStore()));
  const [outgoing, setOutgoing] = useState<Outgoing | null>(null);
  const [error, setError] = useState<string | null>(null);
  const progress = claimFor(record, wallet);

  /// None of these is rendered, and each has to survive a render without
  /// causing one: a request already on its way must not be sent twice, and the
  /// claim that came back from storage — only that one, never a claim this page
  /// has just made — is worth checking with the server once per page load.
  const sending = useRef(false);
  const checked = useRef(false);
  const restored = useRef(record);

  /// The one state a claim goes out from, named by the same function the render
  /// picks its panel with rather than by a second list of the same conditions.
  const panel = pickPanel(wallet, loaded, inboxError, inbox, progress);

  const handle = typedHandle ?? suggestHandle(email);
  const destination = typedDestination ?? email ?? "";
  const target = useMemo(
    () => resolveOutgoing(outgoing, handle, email),
    [outgoing, handle, email]
  );

  const send = useCallback(
    async (name: string, to: string) => {
      if (!wallet) return;
      try {
        const reply = await postClaim(identityToken, signMessage, {
          handle: name,
          destination: to,
          wallet,
        });
        if (reply.live) {
          clearPendingClaim(claimStore());
          setRecord(null);
          onLive();
          return;
        }
        const started: ClaimProgress = {
          handle: reply.handle,
          destination: reply.destination,
          codeVerified: reply.codeVerified,
          cloudflareVerified: reply.cloudflareVerified,
        };
        writePendingClaim(claimStore(), wallet, started);
        setRecord({ wallet, claim: started });
      } catch (cause) {
        setError(causeMessage(cause));
      } finally {
        sending.current = false;
        setOutgoing(null);
      }
    },
    [identityToken, signMessage, wallet, onLive]
  );

  // The hero's button carried the handle and the sign-in together, so the claim
  // it asked for goes out the moment the session can answer for it. A second
  // click here would be asking the user to say the same thing twice.
  //
  // It goes out from one state only, and `pickPanel` already names it: a loaded
  // account, with a wallet, no inbox, no claim in progress and no failed read
  // behind it. `loaded` alone says the request finished, not that it answered —
  // a failed GET leaves it true with `inbox` null, which reads exactly like a
  // wallet with no inbox, and the claim used to go out behind the error screen.
  // That spends one of the five a wallet gets in an hour somewhere its own
  // error can never be read. The ask is kept rather than dropped, so a retry
  // that succeeds sends it.
  useEffect(() => {
    if (!target || sending.current || panel !== "setup") return;
    sending.current = true;
    void send(target.handle, target.destination);
  }, [target, panel, send]);

  // Claim progress used to live in component state alone, so a reload while
  // waiting on Cloudflare's email dropped the claimer back onto the empty form,
  // and claiming again spent one of the five a wallet gets in an hour. What was
  // stored is shown first and reconciled here, against the endpoint the strip
  // already polls — an answer we cannot get is not a reason to forget a claim.
  useEffect(() => {
    // Only ever this wallet's own. Somebody else's is not checked with the
    // server, not cleared, and not shown — it is simply not ours to touch.
    const stored = claimFor(restored.current, wallet);
    if (!loaded || !wallet || !stored || checked.current) return;
    checked.current = true;

    const store = claimStore();
    // The inbox is already real, so the stored claim is a leftover of the one
    // that made it. `pickPanel` shows the inbox over it either way; this stops
    // it outliving the session.
    if (inbox) {
      clearPendingClaim(store);
      return;
    }

    void verifyPendingClaim(stored).then((outcome) => {
      if (outcome.kind === "live") {
        clearPendingClaim(store);
        setRecord(null);
        onLive();
        return;
      }
      if (outcome.kind === "gone") {
        clearPendingClaim(store);
        setRecord(null);
        return;
      }
      if (outcome.kind === "pending") setRecord({ wallet, claim: outcome.claim });
      // "unknown" leaves what was stored on screen; the strip's own poll asks
      // again four seconds later.
    });
  }, [loaded, wallet, inbox, onLive]);

  const start = useCallback(() => {
    setError(null);
    setOutgoing("hero");
    login();
  }, [login]);

  const submit = useCallback(() => {
    setError(null);
    setOutgoing({ handle, destination });
  }, [handle, destination]);

  const advance = useCallback(
    (next: ClaimProgress) => {
      if (!wallet) return;
      writePendingClaim(claimStore(), wallet, next);
      setRecord({ wallet, claim: next });
    },
    [wallet]
  );

  /// The way out of a claim, wherever it is used from: the strip's own control,
  /// a poll that comes back saying the server has dropped it, and signing out.
  const restart = useCallback(() => {
    clearPendingClaim(claimStore());
    setRecord(null);
    setError(null);
  }, []);

  const finish = useCallback(() => {
    clearPendingClaim(claimStore());
    setRecord(null);
    onLive();
  }, [onLive]);

  return {
    handle,
    setHandle: setTypedHandle,
    destination,
    setDestination: setTypedDestination,
    email,
    progress,
    busy: target !== null,
    error,
    start,
    submit,
    advance,
    restart,
    finish,
  };
}

/// The one part of the landing page that needs a session, and the only click
/// the short path asks for. It reads the flow out of context rather than taking
/// props, because the page it sits in is server-rendered and reaches this
/// component as markup, not as a call this file makes.
export function ClaimHero() {
  const claim = useContext(ClaimIntentContext);
  if (!claim) return null;

  const name = claim.handle.trim().toLowerCase();
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        claim.start();
      }}
      className="mt-9 flex max-w-xl flex-col gap-3 sm:flex-row sm:items-start"
    >
      <div className="flex-1">
        <label htmlFor="hero-handle" className="sr-only">
          Your Postage address
        </label>
        <HandleField id="hero-handle" value={claim.handle} onChange={claim.setHandle} />
      </div>
      <button
        type="submit"
        disabled={claim.busy || !isValidHandle(name)}
        className={`${primaryButton} shrink-0 sm:mt-3`}
      >
        {claim.busy ? "Claiming…" : "Claim it"}
      </button>
    </form>
  );
}

function SignedOutActions({ login, disabled }: { login: () => void; disabled: boolean }) {
  return (
    <>
      <Link href="/network" className={quietButton}>
        Ledger
      </Link>
      <button onClick={login} disabled={disabled} className={quietButton}>
        Sign in
      </button>
    </>
  );
}

function Waiting({ children }: { children: ReactNode }) {
  return <main className="m-auto px-6 py-32 text-center text-sm text-faint">{children}</main>;
}

type InboxErrorKind = "unauthorized" | "network";

/// Whether the route turned down the proof rather than failing to read it.
/// 401 is the signature that did not verify; 400 is the proof that named no
/// wallet at all, which is what a session offering an expired identity token
/// and nothing else sends. Both are permanent until the user signs in again,
/// so neither may be dressed up as a transient fault with a retry that cannot
/// work.
function refusedTheProof(status: number): boolean {
  return status === 400 || status === 401;
}

/// `title`/`body` are the words for a page with nothing on it. `stale` is the
/// same fault with something real already on screen, where the news is not that
/// we failed but that what is showing is a moment behind.
const INBOX_ERROR_COPY: Record<InboxErrorKind, { title: string; body: string; stale: string }> = {
  unauthorized: {
    title: "That’s not your wallet.",
    body: "Sign out and back in.",
    stale: "Your session went stale. Sign out and back in to refresh this.",
  },
  network: {
    title: "Can’t reach us.",
    body: "Try again.",
    stale: "We couldn’t reach the server just now.",
  },
};

/// A refresh that failed with something already on screen. It replaces nothing:
/// the inbox or the claim below it is real, came from this same server, and a
/// rotated identity token or a dropped connection is not a reason to take it
/// away and offer a retry in its place. `InboxError` is the same fault with
/// nothing behind it, which is the only time a screen of its own is honest.
function RefreshFailed({ kind, onRetry }: { kind: InboxErrorKind; onRetry: () => void }) {
  return (
    <div className="mx-auto w-full max-w-3xl px-6 pt-6">
      <Callout tone="bad" title="Showing what we last read.">
        <p>{INBOX_ERROR_COPY[kind].stale}</p>
        <button onClick={onRetry} className={`${quietButton} -ml-3 mt-1 text-bad`}>
          Try again
        </button>
      </Callout>
    </div>
  );
}

function InboxError({ kind, onRetry }: { kind: InboxErrorKind; onRetry: () => void }) {
  const { title, body } = INBOX_ERROR_COPY[kind];
  return (
    <main className="rise mx-auto w-full max-w-xl px-6 py-16">
      <Callout tone="bad" title={title}>
        <p>{body}</p>
        <button onClick={onRetry} className={`${quietButton} -ml-3 mt-1 text-bad`}>
          Try again
        </button>
      </Callout>
    </main>
  );
}

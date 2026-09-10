"use client";

import { useIdentityToken, useSignMessage } from "@privy-io/react-auth";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  AddressCard,
  Callout,
  field,
  primaryButton,
  quietButton,
  secondaryButton,
} from "@/components/chrome";
import { causeMessage } from "@/lib/errors";
import { MAIL_DOMAIN, isValidHandle, keepHandleChars, postageAddress } from "@/lib/handle";
import { confirmStatement } from "@/lib/wallet-proof";
import { type ClaimProgress, walletProof } from "./claim-inbox-helpers";
import { pollVerdict, verifyClaimUrl } from "./pending-claim";

/// The last thing asked of somebody signing in without an address on file, and
/// the only place the destination is chosen. It is one panel rather than a
/// wizard: the handle came in from the hero, and the address it forwards to is
/// filled in already for every session Privy could read one from.
export function ClaimSetup({
  handle,
  onHandle,
  destination,
  onDestination,
  email,
  busy,
  error,
  onSubmit,
}: {
  handle: string;
  onHandle: (handle: string) => void;
  destination: string;
  onDestination: (destination: string) => void;
  email: string | null;
  busy: boolean;
  error: string | null;
  onSubmit: () => void;
}) {
  const name = handle.trim().toLowerCase();
  const to = destination.trim().toLowerCase();

  // The same predicate `/api/inbox`'s `validate()` enforces server-side,
  // composed from the same shape/length/repeated-dot rules in `@/lib/handle`
  // — so this cannot disable the button for a shorter list of reasons than
  // the server would reject on, or enable it for one the server accepts.
  const ready = isValidHandle(name) && to.includes("@");

  // `/api/inbox` skips the emailed code only when the destination is the one
  // Privy already verified for this session. Cloudflare destinations are
  // account-wide, so an address somebody else confirmed reads as confirmed to
  // us — every other address has to answer a code of ours first.
  const needsCode = to !== (email ?? "").trim().toLowerCase();

  return (
    <main className="rise mx-auto w-full max-w-xl px-6 py-16">
      <h1 className="text-3xl font-semibold tracking-[-0.03em] text-fg">Pick your address.</h1>
      <p className="mt-3 text-[15px] leading-relaxed text-muted">
        Mail sent to it is read, judged, and forwarded to the inbox you already use.
      </p>

      <form
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit();
        }}
        className="mt-9 rounded-2xl border border-line bg-surface p-6"
      >
        <label htmlFor="handle" className="text-sm font-medium text-fg">
          Your Postage address
        </label>
        <HandleField id="handle" value={handle} onChange={onHandle} />

        <label htmlFor="destination" className="mt-6 block text-sm font-medium text-fg">
          Forwards to
        </label>
        <input
          id="destination"
          value={destination}
          onChange={(event) => onDestination(event.target.value)}
          placeholder="you@gmail.com"
          inputMode="email"
          autoComplete="email"
          className={`${field} mt-3`}
        />
        <p className="mt-2 text-xs text-faint">
          {needsCode
            ? "We will email a code there first, to check you can read it."
            : "The address you signed in with, so there is no code to type."}
        </p>

        <button type="submit" disabled={busy || !ready} className={`${primaryButton} mt-6 w-full`}>
          {busy ? "Claiming…" : "Claim it"}
        </button>
        {error && <p className="mt-3 text-sm text-bad">{error}</p>}
      </form>

      <p className="mt-4 px-1 text-xs leading-relaxed text-faint">
        Cloudflare carries the mail and confirms the destination with its owner itself, so its one
        link is the last step.
      </p>
    </main>
  );
}

/// Exactly what `/api/inbox/verify` answers with, from either verb. `error` is
/// only ever present on a refusal, which the POST reads and the poll ignores.
interface ClaimState {
  codeVerified: boolean;
  cloudflareVerified: boolean;
  live: boolean;
  stalled?: boolean;
  error?: string;
}

/// What is left of the claim, over the dashboard it is turning into. The
/// waiting is real — only the person reading that mailbox can click
/// Cloudflare's link — so the wait happens in front of the thing being waited
/// for rather than on a screen of its own.
export function ClaimStrip({
  claim,
  wallet,
  onClaim,
  onLive,
  onRestart,
}: {
  claim: ClaimProgress;
  wallet: string;
  onClaim: (claim: ClaimProgress) => void;
  onLive: () => void;
  onRestart: () => void;
}) {
  const { identityToken } = useIdentityToken();
  const { signMessage } = useSignMessage();
  const [code, setCode] = useState("");
  /// The server has stopped asking Cloudflare about this claim, so polling it
  /// can only ever return the same answer.
  const [stalled, setStalled] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /// An attempt that came back wrong. Until one does, the sixth digit is the
  /// whole of what we are asking for and a button to confirm it asks twice.
  const [failed, setFailed] = useState(false);
  const inFlight = useRef<AbortController | null>(null);
  /// The code the last attempt went out with. Not rendered, and it must not
  /// cause a render: what it is for is deciding, inside the very keystroke that
  /// would send it, whether these six digits have already been sent.
  const lastSent = useRef<string | null>(null);

  /// Only writes back a step that actually moved. An unconditional write is a
  /// new object every four seconds, which re-runs the effect below and restarts
  /// the interval it just set — the poll then never reaches its own deadline.
  const apply = useCallback(
    (next: ClaimState) => {
      if (next.live) return onLive();
      if (next.stalled) setStalled(true);

      const codeVerified = claim.codeVerified || next.codeVerified;
      const cloudflareVerified = claim.cloudflareVerified || next.cloudflareVerified;
      if (codeVerified === claim.codeVerified && cloudflareVerified === claim.cloudflareVerified) {
        return;
      }
      onClaim({ ...claim, codeVerified, cloudflareVerified });
    },
    [claim, onClaim, onLive]
  );

  // Cloudflare's half turns green when the user clicks the link in its email,
  // which happens outside this page, so it has to be asked for.
  useEffect(() => {
    if (claim.cloudflareVerified || stalled) return;
    const poll = setInterval(async () => {
      inFlight.current?.abort();
      const controller = new AbortController();
      inFlight.current = controller;
      try {
        const response = await fetch(verifyClaimUrl(claim.handle), { signal: controller.signal });
        const verdict = pollVerdict(response.status);
        // The server is certain there is nothing here to wait for: the code
        // timed out, or the claim was promoted and cleared. The strip must not
        // outlive it — it used to, for as long as the tab stayed open, because
        // this poll dropped a 404 along with every other non-200.
        if (verdict === "gone") return onRestart();
        // Anything else that is not an answer says nothing about the claim, and
        // forgetting one on a wobble spends one of the five a wallet gets in an
        // hour.
        if (verdict === "wait") return;
        apply((await response.json()) as ClaimState);
      } catch {
        // A poll that fails is a poll that runs again in four seconds.
      }
    }, 4000);
    return () => {
      clearInterval(poll);
      inFlight.current?.abort();
    };
  }, [claim.handle, claim.cloudflareVerified, stalled, apply, onRestart]);

  async function submit(entered: string) {
    lastSent.current = entered;
    setBusy(true);
    setError(null);
    try {
      // The code says whoever holds this mailbox agreed; it does not say who is
      // claiming. The wallet the claim was started with says that, and both are
      // needed — it is the wallet the inbox's earnings accrue to.
      const proof = await walletProof(identityToken, signMessage, wallet, (at) =>
        confirmStatement(claim.handle, wallet, at)
      );

      const response = await fetch("/api/inbox/verify", {
        method: "POST",
        headers: { "Content-Type": "application/json", ...proof },
        body: JSON.stringify({ handle: claim.handle, code: entered.trim() }),
      });
      const result = (await response.json()) as ClaimState;
      // Wrong, expired, spent, or refused — the server's own words are better
      // than any we could pick from a status, and the way back to a new code is
      // on screen whichever it was.
      if (!response.ok) throw new Error(result.error ?? "Could not check that code");
      setFailed(false);
      apply(result);
    } catch (cause) {
      setFailed(true);
      setError(causeMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  function enter(raw: string) {
    const digits = raw.replace(/\D/g, "").slice(0, 6);
    setCode(digits);
    if (readyToSubmit(digits, busy, failed, lastSent.current)) void submit(digits);
  }

  return (
    <main className="rise mx-auto w-full max-w-3xl px-6 py-14">
      <div className="glow rounded-2xl border border-line-strong bg-surface p-6 sm:p-7">
        <p className="text-[11px] font-semibold uppercase tracking-[0.2em] text-accent">
          One click left
        </p>
        <h1 className="mt-3 font-mono text-xl break-all text-fg sm:text-2xl">
          {postageAddress(claim.handle)}
        </h1>

        {!claim.codeVerified && (
          <div className="mt-6">
            <label htmlFor="code" className="text-sm text-fg">
              Code from your email
            </label>
            <div className="mt-3 flex flex-wrap gap-2">
              <input
                id="code"
                value={code}
                onChange={(event) => enter(event.target.value)}
                inputMode="numeric"
                autoComplete="one-time-code"
                placeholder="000000"
                className="w-36 rounded-xl border border-line-strong bg-surface-2 px-4 py-3 text-center font-mono text-[15px] tracking-[0.3em] text-fg outline-none transition focus:border-accent"
              />
              {failed && (
                <button
                  onClick={() => void submit(code)}
                  disabled={busy || code.length !== 6}
                  className={secondaryButton}
                >
                  {busy ? "Checking" : "Confirm"}
                </button>
              )}
            </div>
          </div>
        )}

        <p className="mt-6 text-[15px] leading-relaxed text-muted">
          {claim.codeVerified ? (
            <>
              Cloudflare emailed <span className="font-mono text-fg">{claim.destination}</span>.
              Click its link and you’re live.
            </>
          ) : (
            <>
              We emailed a code to <span className="font-mono text-fg">{claim.destination}</span>.
              Cloudflare’s own link follows.
            </>
          )}
        </p>

        {claim.codeVerified &&
          !claim.cloudflareVerified &&
          (stalled ? (
            <p className="mt-2 text-sm text-muted">
              We stopped watching. Start over below — it’s quick.
            </p>
          ) : (
            <p className="mt-2 text-sm text-faint">Waiting on Cloudflare. Check spam.</p>
          ))}

        {error && (
          <div className="mt-5">
            <Callout tone="bad" title={error} />
          </div>
        )}

        <StartOver onRestart={onRestart} />
      </div>

      <PendingDashboard handle={claim.handle} destination={claim.destination} />
    </main>
  );
}

/// Whether the digits now in the field are a code to send. The field sends
/// itself on the sixth digit — there is nothing else on it to decide, and a
/// code just read off an email is not improved by a second gesture confirming
/// it was read correctly — but it slices to six, so a seventh keystroke leaves
/// those same six digits in place. Sending them again spends one of the
/// attempts this claim gets before its code is dead for good.
function readyToSubmit(digits: string, busy: boolean, failed: boolean, lastSent: string | null): boolean {
  if (digits.length !== 6) return false;
  if (busy || failed) return false;
  return digits !== lastSent;
}

/// The way out, from wherever the claim has got to.
///
/// Every route back to the form used to be gated: the code field on
/// `!claim.codeVerified`, one restart on a stalled Cloudflare, another on a 410
/// or 429. A claim sent to a mistyped address has none of those — its code
/// lands somewhere its owner cannot read, so `codeVerified` never turns — and
/// its owner had no way back at all short of clearing site data. Losing a
/// pending claim costs one of five tries in an hour; being held in one forever
/// costs the account.
function StartOver({ onRestart }: { onRestart: () => void }) {
  return (
    <div className="mt-7 border-t border-line pt-4">
      <button onClick={onRestart} className={`${quietButton} -ml-3`}>
        Start over
      </button>
      <p className="mt-1 px-1 text-xs text-faint">
        Wrong address, or no code arrived? This forgets the claim and takes you back to the form.
      </p>
    </div>
  );
}

/// The dashboard this claim is turning into, drawn from what the claim already
/// knows. Not `InboxPanel`: there is no inbox row yet and nothing on chain to
/// read for one, so there is nothing here to press either. Hidden from screen
/// readers because the strip above it has just said all of it.
function PendingDashboard({ handle, destination }: { handle: string; destination: string }) {
  return (
    <div className="mt-10 opacity-50" aria-hidden>
      <div className="flex flex-col gap-10 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <p className="text-[11px] font-semibold uppercase tracking-[0.2em] text-faint">Pending</p>
          <p className="mt-4 font-mono text-2xl break-all text-fg sm:text-[1.7rem]">
            {postageAddress(handle)}
          </p>
          <p className="mt-2 text-[15px] text-muted">
            → <span className="font-mono text-fg">{destination}</span>
          </p>
        </div>
        <div className="hidden shrink-0 sm:block">
          <AddressCard
            handle={postageAddress(handle)}
            price="—"
            caption="Yours the moment Cloudflare answers"
          />
        </div>
      </div>
    </div>
  );
}

/// The handle input and its domain, drawn as one field so the address reads as
/// a whole. Shared by the hero and the setup panel, which ask for exactly the
/// same thing in two different places.
export function HandleField({
  id,
  value,
  onChange,
}: {
  id: string;
  value: string;
  onChange: (handle: string) => void;
}) {
  return (
    <div className="mt-3 flex items-stretch overflow-hidden rounded-xl border border-line-strong bg-surface-2 transition focus-within:border-accent">
      <input
        id={id}
        value={value}
        onChange={(event) => onChange(keepHandleChars(event.target.value))}
        placeholder="you"
        autoComplete="off"
        spellCheck={false}
        maxLength={31}
        className="w-full min-w-0 bg-transparent px-4 py-3 font-mono text-[15px] text-fg outline-none placeholder:text-faint"
      />
      <span className="grid shrink-0 place-items-center pr-4 font-mono text-[15px] text-faint">
        @{MAIL_DOMAIN}
      </span>
    </div>
  );
}

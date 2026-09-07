"use client";

import { useIdentityToken, useSignMessage } from "@privy-io/react-auth";
import { useCallback, useEffect, useRef, useState } from "react";
import { Callout, field, primaryButton, quietButton, secondaryButton } from "@/components/chrome";
import { claimStatement } from "@/lib/statements";

export interface ClaimState {
  handle: string;
  destination: string;
  codeVerified: boolean;
  cloudflareVerified: boolean;
}

interface ClaimReply extends ClaimState {
  live: boolean;
  error?: string;
}

type SignMessage = ReturnType<typeof useSignMessage>["signMessage"];

/// Best effort. A signature is the only proof for someone whose session cannot
/// be read from an identity token, and redundant for everyone else, so a wallet
/// that refuses to sign is not on its own a reason to stop.
async function signClaim(
  signMessage: SignMessage,
  claim: { handle: string; destination: string; wallet: string }
): Promise<{ issuedAt: number; signature: string } | null> {
  try {
    const issuedAt = Math.floor(Date.now() / 1000);
    const { signature } = await signMessage(
      { message: claimStatement(claim.handle, claim.destination, claim.wallet, issuedAt) },
      { address: claim.wallet, uiOptions: { showWalletUIs: false } }
    );
    return { issuedAt, signature };
  } catch {
    return null;
  }
}

/// Turns an email address into the handle its owner would probably have picked,
/// so the field arrives filled in rather than empty.
function suggestHandle(email: string | null): string {
  const local = email?.split("@")[0]?.toLowerCase() ?? "";
  const cleaned = local
    .replace(/[^a-z0-9._-]/g, "")
    .replace(/\.{2,}/g, ".")
    .replace(/^[._-]+|[._-]+$/g, "");
  return cleaned.length >= 2 ? cleaned.slice(0, 31) : "";
}

export function ClaimInbox({
  wallet,
  email,
  onLive,
}: {
  wallet: string;
  email: string | null;
  onLive: () => void;
}) {
  const [claim, setClaim] = useState<ClaimState | null>(null);

  if (!claim) {
    return <PickHandle wallet={wallet} email={email} onStarted={setClaim} onLive={onLive} />;
  }
  return <FinishClaim claim={claim} onClaim={setClaim} onLive={onLive} onRestart={() => setClaim(null)} />;
}

/// The whole of signing up for someone whose address Privy already checked: a
/// name, and a button.
function PickHandle({
  wallet,
  email,
  onStarted,
  onLive,
}: {
  wallet: string;
  email: string | null;
  onStarted: (claim: ClaimState) => void;
  onLive: () => void;
}) {
  const { identityToken } = useIdentityToken();
  const { signMessage } = useSignMessage();
  const [handle, setHandle] = useState(() => suggestHandle(email));
  const [elsewhere, setElsewhere] = useState(!email);
  const [destination, setDestination] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const to = (elsewhere ? destination : (email ?? "")).trim().toLowerCase();
  const name = handle.trim().toLowerCase();

  async function create() {
    setSaving(true);
    setError(null);
    try {
      const proof = await signClaim(signMessage, { handle: name, destination: to, wallet });
      if (!identityToken && !proof) {
        throw new Error("Could not confirm the wallet is yours. Sign in again and retry");
      }

      const response = await fetch("/api/inbox", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...(identityToken ? { "privy-id-token": identityToken } : {}),
        },
        body: JSON.stringify({ handle: name, destination: to, wallet, ...proof }),
      });
      const result = (await response.json()) as ClaimReply;
      if (!response.ok) throw new Error(result.error ?? "Could not claim that address");
      if (result.live) return onLive();
      onStarted(result);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }

  const ready = name.length >= 2 && to.includes("@");

  return (
    <section className="rise mx-auto w-full max-w-xl px-6 py-16">
      <p className="text-[11px] font-semibold uppercase tracking-[0.2em] text-stamp">One step</p>
      <h1 className="mt-4 text-3xl font-semibold tracking-[-0.03em] text-ink">
        Pick the address you hand out.
      </h1>
      <p className="mt-3 text-[15px] leading-relaxed text-ink-soft">
        Mail sent to it is read, judged, and forwarded to the inbox you already use. You do not
        change email provider, and there is nothing to install.
      </p>

      <div className="mt-9 rounded-2xl border border-rule bg-card p-6">
        <label htmlFor="handle" className="text-sm font-medium text-ink">
          Your Postage address
        </label>
        <div className="mt-3 flex items-stretch overflow-hidden rounded-xl border border-rule bg-paper focus-within:border-ink">
          <input
            id="handle"
            value={handle}
            onChange={(event) => setHandle(event.target.value.replace(/[^A-Za-z0-9._-]/g, ""))}
            placeholder="you"
            autoComplete="off"
            spellCheck={false}
            maxLength={31}
            className="w-full bg-transparent px-4 py-3 font-mono text-[15px] text-ink outline-none placeholder:text-ink-faint"
          />
          <span className="grid shrink-0 place-items-center border-l border-rule px-4 font-mono text-[15px] text-ink-faint">
            @usepostage.com
          </span>
        </div>

        <div className="mt-5 border-t border-rule pt-5">
          {elsewhere ? (
            <>
              <label htmlFor="destination" className="text-sm font-medium text-ink">
                Forward it to
              </label>
              <input
                id="destination"
                value={destination}
                onChange={(event) => setDestination(event.target.value)}
                placeholder="you@gmail.com"
                inputMode="email"
                autoComplete="email"
                className={`${field} mt-3`}
              />
              <p className="mt-2 text-xs text-ink-faint">
                We will mail a code there to check you can read it.
              </p>
              {email && (
                <button onClick={() => setElsewhere(false)} className={`${quietButton} mt-1 -ml-3`}>
                  Use {email} instead
                </button>
              )}
            </>
          ) : (
            <div className="flex flex-wrap items-baseline justify-between gap-2">
              <p className="text-sm text-ink-soft">
                Forwards to <span className="font-mono text-ink">{email}</span>
              </p>
              <button onClick={() => setElsewhere(true)} className={`${quietButton} -mr-3`}>
                Somewhere else
              </button>
            </div>
          )}
        </div>

        <button onClick={create} disabled={saving || !ready} className={`${primaryButton} mt-6 w-full`}>
          {saving ? "Claiming" : `Claim ${name || "your"}@usepostage.com`}
        </button>
        {error && <p className="mt-3 text-sm text-stamp">{error}</p>}
      </div>

      <p className="mt-4 px-1 text-xs leading-relaxed text-ink-faint">
        Nothing is forwarded anywhere until Cloudflare, who carries the mail, has confirmed the
        destination with its owner. That is one click in one email, and it is the last thing anyone
        asks of you.
      </p>
    </section>
  );
}

/// What is left after the claim: the code, when one was needed, and Cloudflare's
/// own link, which no API can answer on the owner's behalf.
function FinishClaim({
  claim,
  onClaim,
  onLive,
  onRestart,
}: {
  claim: ClaimState;
  onClaim: (claim: ClaimState) => void;
  onLive: () => void;
  onRestart: () => void;
}) {
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [stuck, setStuck] = useState(false);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef<AbortController | null>(null);

  const apply = useCallback(
    (next: { codeVerified: boolean; cloudflareVerified: boolean; live: boolean }) => {
      if (next.live) return onLive();
      onClaim({
        ...claim,
        codeVerified: claim.codeVerified || next.codeVerified,
        cloudflareVerified: claim.cloudflareVerified || next.cloudflareVerified,
      });
    },
    [claim, onClaim, onLive]
  );

  // Cloudflare's half turns green when the user clicks the link in its email,
  // which happens outside this page, so it has to be asked for.
  useEffect(() => {
    if (claim.cloudflareVerified) return;
    const poll = setInterval(async () => {
      inFlight.current?.abort();
      const controller = new AbortController();
      inFlight.current = controller;
      try {
        const response = await fetch(`/api/inbox/verify?handle=${claim.handle}`, {
          signal: controller.signal,
        });
        if (response.ok) apply(await response.json());
      } catch {
        // A poll that fails is a poll that runs again in four seconds.
      }
    }, 4000);
    return () => {
      clearInterval(poll);
      inFlight.current?.abort();
    };
  }, [claim.handle, claim.cloudflareVerified, apply]);

  async function submit() {
    setBusy(true);
    setError(null);
    try {
      const response = await fetch("/api/inbox/verify", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ handle: claim.handle, code: code.trim() }),
      });
      const result = await response.json();
      if (!response.ok) {
        // Gone or spent. The code cannot be salvaged, so say so and offer the
        // only thing that works instead of leaving a dead screen.
        setStuck(response.status === 410 || response.status === 429);
        throw new Error(result.error ?? "Could not check that code");
      }
      apply(result);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="rise mx-auto w-full max-w-xl px-6 py-16">
      <p className="text-[11px] font-semibold uppercase tracking-[0.2em] text-stamp">
        Almost yours
      </p>
      <h1 className="mt-4 text-3xl font-semibold tracking-[-0.03em] text-ink">
        <span className="font-mono text-[1.6rem]">{claim.handle}@usepostage.com</span>
      </h1>
      <p className="mt-3 text-[15px] leading-relaxed text-ink-soft">
        Check <span className="font-mono text-ink">{claim.destination}</span>.{" "}
        {claim.codeVerified
          ? "Cloudflare carries the mail and has just sent a link of its own. Click it and you are done — often it is already sorted and this finishes on its own."
          : "We sent a code there, and Cloudflare will send a link of its own straight after."}
      </p>

      <div className="mt-9 space-y-4">
        {!claim.codeVerified && (
          <Step done={false} n={1} label="Enter the code we emailed you">
            <div className="mt-3 flex gap-2">
              <input
                value={code}
                onChange={(event) => setCode(event.target.value.replace(/\D/g, "").slice(0, 6))}
                inputMode="numeric"
                autoComplete="one-time-code"
                placeholder="000000"
                className="w-36 rounded-xl border border-rule bg-card px-4 py-3 text-center font-mono text-[15px] tracking-[0.3em] text-ink outline-none focus:border-ink"
              />
              <button
                onClick={submit}
                disabled={busy || code.length !== 6}
                className={secondaryButton}
              >
                {busy ? "Checking" : "Confirm"}
              </button>
            </div>
          </Step>
        )}

        <Step
          done={claim.cloudflareVerified}
          n={claim.codeVerified ? 1 : 2}
          label="Click the link Cloudflare emailed you"
        >
          {claim.cloudflareVerified ? null : claim.codeVerified ? (
            <p className="mt-2 text-sm leading-relaxed text-ink-soft">
              Waiting for it. Cloudflare will not carry mail to an address it has not checked
              itself, and only the person reading that mailbox can answer. It comes from
              cloudflare.com, so look in spam if it is not there.
            </p>
          ) : (
            <p className="mt-2 text-sm text-ink-faint">
              Nothing to do yet. We ask Cloudflare the moment your code goes in, so you deal with
              one email at a time.
            </p>
          )}
        </Step>
      </div>

      {error && (
        <div className="mt-6">
          <Callout tone="bad" title={error}>
            {stuck && (
              <button onClick={onRestart} className={`${quietButton} -ml-3 mt-1 text-stamp`}>
                Start again with a new code
              </button>
            )}
          </Callout>
        </div>
      )}
    </section>
  );
}

function Step({
  done,
  n,
  label,
  children,
}: {
  done: boolean;
  n: number;
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex gap-3.5 rounded-2xl border border-rule bg-card p-5">
      <span
        className={`mt-0.5 grid h-6 w-6 shrink-0 place-items-center rounded-full text-xs ${
          done ? "bg-good text-paper" : "border border-rule-strong text-ink-faint"
        }`}
      >
        {done ? "✓" : n}
      </span>
      <div className="min-w-0 flex-1">
        <p className={`text-[15px] ${done ? "text-ink-faint line-through" : "text-ink"}`}>{label}</p>
        {children}
      </div>
    </div>
  );
}

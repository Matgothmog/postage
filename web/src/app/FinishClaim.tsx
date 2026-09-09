"use client";

import { useIdentityToken, useSignMessage } from "@privy-io/react-auth";
import { useCallback, useEffect, useRef, useState } from "react";
import { Callout, quietButton, secondaryButton } from "@/components/chrome";
import { causeMessage } from "@/lib/errors";
import { postageAddress } from "@/lib/handle";
import { confirmStatement } from "@/lib/wallet-proof";
import { type ClaimProgress, walletProof } from "./claim-inbox-helpers";

/// What is left after the claim: the code, when one was needed, and Cloudflare's
/// own link, which no API can answer on the owner's behalf.
export function FinishClaim({
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
  const [stuck, setStuck] = useState(false);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef<AbortController | null>(null);

  /// Only writes back a step that actually moved. An unconditional write is a
  /// new object every four seconds, which re-runs the effect below and restarts
  /// the interval it just set — the poll then never reaches its own deadline.
  const apply = useCallback(
    (next: {
      codeVerified: boolean;
      cloudflareVerified: boolean;
      live: boolean;
      stalled?: boolean;
    }) => {
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
  }, [claim.handle, claim.cloudflareVerified, stalled, apply]);

  async function submit() {
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
      setError(causeMessage(cause));
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
        <span className="font-mono text-[1.6rem]">{postageAddress(claim.handle)}</span>
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
          {claim.cloudflareVerified ? null : stalled ? (
            <div className="mt-2">
              <p className="text-sm leading-relaxed text-ink-soft">
                We have stopped watching for it. Clicking the link Cloudflare sent is still worth
                doing and is remembered — but finishing from here means starting again, which is
                quick and skips nothing you have already done.
              </p>
              <button onClick={onRestart} className={`${quietButton} -ml-3 mt-1`}>
                Start again
              </button>
            </div>
          ) : claim.codeVerified ? (
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

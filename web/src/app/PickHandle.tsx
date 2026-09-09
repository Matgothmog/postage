"use client";

import { useIdentityToken, useSignMessage } from "@privy-io/react-auth";
import { useState } from "react";
import { field, primaryButton, quietButton } from "@/components/chrome";
import { causeMessage } from "@/lib/errors";
import { MAIL_DOMAIN, isValidHandle, keepHandleChars, postageAddress } from "@/lib/handle";
import { IDENTITY_TOKEN_HEADER } from "@/lib/wallet-proof";
import { type ClaimProgress, signClaim, suggestHandle } from "./claim-inbox-helpers";

interface ClaimReply extends ClaimProgress {
  live: boolean;
  error?: string;
}

/// The whole of signing up for someone whose address Privy already checked: a
/// name, and a button.
export function PickHandle({
  wallet,
  email,
  onStarted,
  onLive,
}: {
  wallet: string;
  email: string | null;
  onStarted: (claim: ClaimProgress) => void;
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
          ...(identityToken ? { [IDENTITY_TOKEN_HEADER]: identityToken } : {}),
        },
        body: JSON.stringify({ handle: name, destination: to, wallet, ...proof }),
      });
      const result = (await response.json()) as ClaimReply;
      if (!response.ok) throw new Error(result.error ?? "Could not claim that address");
      if (result.live) return onLive();
      onStarted(result);
    } catch (cause) {
      setError(causeMessage(cause));
    } finally {
      setSaving(false);
    }
  }

  // The same predicate `/api/inbox`'s `validate()` enforces server-side,
  // composed from the same shape/length/repeated-dot rules in `@/lib/handle`
  // — so this cannot disable the button for a shorter list of reasons than
  // the server would reject on, or enable it for one the server accepts.
  const ready = isValidHandle(name) && to.includes("@");

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
            onChange={(event) => setHandle(keepHandleChars(event.target.value))}
            placeholder="you"
            autoComplete="off"
            spellCheck={false}
            maxLength={31}
            className="w-full bg-transparent px-4 py-3 font-mono text-[15px] text-ink outline-none placeholder:text-ink-faint"
          />
          <span className="grid shrink-0 place-items-center border-l border-rule px-4 font-mono text-[15px] text-ink-faint">
            @{MAIL_DOMAIN}
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
          {saving ? "Claiming" : `Claim ${postageAddress(name || "your")}`}
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

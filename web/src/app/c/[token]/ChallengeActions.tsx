"use client";

import { usePrivy, useSendTransaction, useWallets } from "@privy-io/react-auth";
import { useState } from "react";
import { encodeFunctionData } from "viem";
import { Callout, field, primaryButton, quietButton } from "@/components/chrome";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import { formatUsdc } from "@/lib/format";
import { postageAddress } from "@/lib/handle";
import { tierIndexOf } from "@/lib/tiers";

interface Quote {
  messageId: string;
  inbox: string;
  tier: string;
  amount: string;
  expiresAt: number;
  signature: string;
}

/// Long enough to cover a block on Arc and the indexing behind it, short
/// enough that a genuine failure still surfaces while the sender is looking.
const SETTLEMENT_ATTEMPTS = 10;
const SETTLEMENT_INTERVAL_MS = 1_500;

type Outcome =
  | { kind: "cleared"; reason: string; delivered: boolean }
  | { kind: "charged" }
  | { kind: "error"; message: string };

export function ChallengeActions({
  token,
  quote,
  dangerous,
  handle,
  lane: initialLane,
}: {
  token: string;
  quote: Quote;
  dangerous: boolean;
  handle: string;
  /// Which answer they already gave. Both are links in the mail they were sent,
  /// so arriving here having chosen should not mean choosing again.
  lane: "choosing" | "paying";
}) {
  const [lane, setLane] = useState(dangerous ? "choosing" : initialLane);
  const [verifying, setVerifying] = useState(false);
  const [outcome, setOutcome] = useState<Outcome | null>(null);

  if (outcome?.kind === "charged") {
    return (
      <div className="mt-8">
        <Callout tone="bad" title="Charged, and still not delivered.">
          Paying is the penalty for this tier, not the price of getting through. Nothing was sent.
        </Callout>
      </div>
    );
  }

  if (outcome?.kind === "cleared") {
    // The held message went out on its own, so there is nothing left to ask of
    // the sender. Only a hold that ran out sends them back to the compose box.
    if (outcome.delivered) {
      return (
        <div className="mt-8">
          <Callout
            tone="good"
            title={outcome.reason === "human" ? "Verified, and delivered." : "Paid, and delivered."}
          >
            The message you already sent is in their inbox — the same bytes, the same signature, the
            same sender. You did not write it twice, and our copy is gone.
          </Callout>
        </div>
      );
    }
    return <Deliver token={token} handle={handle} reason={outcome.reason} />;
  }

  /// No wallet anywhere on this path. Proving personhood is not a payment, so
  /// it should not need an account to make one.
  async function verifyHuman() {
    setVerifying(true);
    setOutcome(null);
    try {
      const response = await fetch("/api/world/verify", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ token }),
      });
      const result = (await response.json()) as {
        status?: string;
        delivered?: boolean;
        error?: string;
      };
      if (result.status !== "cleared") throw new Error(result.error ?? "Verification failed");
      setOutcome({ kind: "cleared", reason: "human", delivered: result.delivered === true });
    } catch (cause) {
      setOutcome({ kind: "error", message: asMessage(cause) });
    } finally {
      setVerifying(false);
    }
  }

  if (lane === "paying") {
    return (
      <PayLane
        token={token}
        quote={quote}
        onSettled={setOutcome}
        onBack={() => setLane("choosing")}
      />
    );
  }

  return (
    <div className="mt-8 space-y-3">
      <button onClick={verifyHuman} disabled={verifying} className={`${primaryButton} w-full`}>
        {verifying ? "Verifying" : "A person wrote this"}
      </button>
      <p className="px-1 text-xs leading-relaxed text-ink-faint">
        Prove it with World ID and your message is delivered. Free, no wallet, nothing to install
        beyond the World app.
      </p>

      {!dangerous && (
        <>
          <button
            onClick={() => setLane("paying")}
            disabled={verifying}
            className="w-full rounded-xl border border-rule-strong bg-card px-5 py-3 text-sm font-medium text-ink transition hover:border-ink disabled:opacity-40"
          >
            A machine sent this — pay {formatUsdc(BigInt(quote.amount))}
          </button>
          <p className="px-1 text-xs leading-relaxed text-ink-faint">
            Automated mail pays the recipient for the attention. You will need somewhere to pay
            from, which takes an email address.
          </p>
        </>
      )}

      {outcome?.kind === "error" && <p className="text-sm text-stamp">{outcome.message}</p>}
    </div>
  );
}

/// Only this side of the fork needs an account, because only this side moves
/// money. A person who is simply a person never reaches it.
function PayLane({
  token,
  quote,
  onSettled,
  onBack,
}: {
  token: string;
  quote: Quote;
  onSettled: (outcome: Outcome) => void;
  onBack: () => void;
}) {
  const { ready, authenticated, login } = usePrivy();
  const { wallets } = useWallets();
  const { sendTransaction } = useSendTransaction();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const wallet = wallets[0];

  async function askOnce(): Promise<Outcome> {
    const response = await fetch("/api/challenge/resolve", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ token }),
    });
    const result = (await response.json()) as {
      status?: string;
      reason?: string;
      error?: string;
      delivered?: boolean;
    };
    if (result.status === "cleared") {
      return { kind: "cleared", reason: result.reason ?? "paid", delivered: result.delivered === true };
    }
    if (result.status === "charged") return { kind: "charged" };
    return { kind: "error", message: result.error ?? "Not cleared yet" };
  }

  /// A transaction that has been broadcast is not yet a transaction that has
  /// been mined. Asking once would tell most senders their payment failed a
  /// second after it succeeded.
  async function settle(): Promise<Outcome> {
    let outcome = await askOnce();
    for (let attempt = 0; attempt < SETTLEMENT_ATTEMPTS && outcome.kind === "error"; attempt += 1) {
      await new Promise((wake) => setTimeout(wake, SETTLEMENT_INTERVAL_MS));
      outcome = await askOnce();
    }
    return outcome;
  }

  async function pay() {
    setBusy(true);
    setError(null);
    try {
      await sendTransaction({
        to: POSTAGE_ESCROW,
        value: BigInt(quote.amount),
        data: encodeFunctionData({
          abi: escrowAbi,
          functionName: "payToSend",
          args: [
            quote.messageId as `0x${string}`,
            quote.inbox as `0x${string}`,
            tierIndexOf(quote.tier),
            BigInt(quote.amount),
            quote.expiresAt,
            quote.signature as `0x${string}`,
          ],
        }),
      });
      onSettled(await settle());
    } catch (cause) {
      setError(asMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  if (!ready) return <p className="mt-8 text-sm text-ink-faint">Loading</p>;

  return (
    <div className="mt-8 space-y-3">
      {!authenticated ? (
        <>
          <button onClick={login} className={`${primaryButton} w-full`}>
            Set up a way to pay
          </button>
          <p className="px-1 text-xs leading-relaxed text-ink-faint">
            An email address is enough. It creates a wallet for you on Arc; there is nothing to
            install.
          </p>
        </>
      ) : !wallet ? (
        <p className="text-sm text-ink-faint">Setting up your wallet</p>
      ) : (
        <button onClick={pay} disabled={busy} className={`${primaryButton} w-full`}>
          {busy ? "Paying" : `Pay ${formatUsdc(BigInt(quote.amount))} and deliver it`}
        </button>
      )}

      <button onClick={onBack} className={`${quietButton} w-full`}>
        Actually, a person wrote it
      </button>
      {error && <p className="text-sm text-stamp">{error}</p>}
    </div>
  );
}

/// The way back when the hold has already run out, or when the send failed. The
/// message it puts through is one the sender writes here, so it goes out under
/// our name rather than pretending to be theirs.
function Deliver({ token, handle, reason }: { token: string; handle: string; reason: string }) {
  const [subject, setSubject] = useState("");
  const [body, setBody] = useState("");
  const [sending, setSending] = useState(false);
  const [sent, setSent] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function deliver() {
    setSending(true);
    setError(null);
    try {
      const response = await fetch("/api/challenge/deliver", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ token, subject, body }),
      });
      const result = (await response.json()) as { error?: string };
      if (!response.ok) throw new Error(result.error ?? "Could not deliver it");
      setSent(true);
    } catch (cause) {
      setError(asMessage(cause));
    } finally {
      setSending(false);
    }
  }

  if (sent) {
    return (
      <div className="mt-8">
        <Callout tone="good" title="Delivered.">
          It is in their inbox now, and replying comes straight back to you.
        </Callout>
      </div>
    );
  }

  return (
    <div className="mt-8 space-y-4">
      <Callout tone="good" title={reason === "human" ? "Verified. That cost you nothing." : "Paid."}>
        The message itself is no longer here — the hold ran out before this was answered. Paste it
        below and we will deliver it now.
      </Callout>

      <input
        value={subject}
        onChange={(event) => setSubject(event.target.value)}
        placeholder="Subject"
        className={field}
      />
      <textarea
        value={body}
        onChange={(event) => setBody(event.target.value)}
        rows={7}
        placeholder={`Paste what you wrote to ${postageAddress(handle)}`}
        className={`${field} resize-y`}
      />
      <button
        onClick={deliver}
        disabled={sending || body.trim().length === 0}
        className={`${primaryButton} w-full`}
      >
        {sending ? "Delivering" : "Deliver it now"}
      </button>
      {error && <p className="text-sm text-stamp">{error}</p>}
    </div>
  );
}

function asMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

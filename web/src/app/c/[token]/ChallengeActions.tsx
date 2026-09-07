"use client";

import { usePrivy, useSendTransaction, useWallets } from "@privy-io/react-auth";
import { useState } from "react";
import { encodeFunctionData } from "viem";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import { formatUsdc } from "@/lib/format";

interface Quote {
  messageId: string;
  inbox: string;
  tier: string;
  amount: string;
  expiresAt: number;
  signature: string;
}

const TIER_INDEX: Record<string, number> = { human: 0, important: 1, commercial: 2, dangerous: 3 };

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
}: {
  token: string;
  quote: Quote;
  dangerous: boolean;
  handle: string;
}) {
  const { ready, authenticated, login } = usePrivy();
  const { wallets } = useWallets();
  const { sendTransaction } = useSendTransaction();

  const [busy, setBusy] = useState<"human" | "pay" | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);

  const wallet = wallets[0];

  if (!ready) return null;
  if (authenticated && !wallet) return <p className={note}>Setting up your wallet</p>;
  if (!authenticated) {
    return (
      <button onClick={login} className={primary}>
        Continue
      </button>
    );
  }

  if (outcome?.kind === "charged") {
    return (
      <div className="mt-8 rounded-xl border border-red-200 bg-red-50 p-4 text-sm text-red-800">
        <p className="font-medium">Charged, and still not delivered.</p>
        <p className="mt-1">
          Paying is the penalty for this tier, not the price of getting through. Nothing was sent.
        </p>
      </div>
    );
  }

  if (outcome?.kind === "cleared") {
    // The held message went out on its own, so there is nothing left to ask of
    // the sender. Only a hold that ran out sends them back to the compose box.
    if (outcome.delivered) {
      return (
        <div className="mt-8 rounded-xl border border-green-200 bg-green-50 p-4 text-sm text-green-800">
          <p className="font-medium">
            {outcome.reason === "human" ? "Verified, and delivered." : "Paid, and delivered."}
          </p>
          <p className="mt-1">
            The message you already sent is in their inbox. You did not have to write it twice, and
            we have erased our copy.
          </p>
        </div>
      );
    }
    return <Deliver token={token} handle={handle} reason={outcome.reason} />;
  }

  async function askOnce(): Promise<Outcome> {
    const response = await fetch("/api/challenge/resolve", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ token, wallet: wallet.address }),
    });
    const result = (await response.json()) as {
      status?: string;
      reason?: string;
      error?: string;
      delivered?: boolean;
    };
    if (result.status === "cleared") {
      return { kind: "cleared", reason: result.reason ?? "human", delivered: result.delivered === true };
    }
    if (result.status === "charged") return { kind: "charged" };
    return { kind: "error", message: result.error ?? "Not cleared yet" };
  }

  /// The gate opens on what the chain says, and a transaction that has been
  /// broadcast is not yet a transaction that has been mined. Asking once would
  /// tell most senders their payment failed a second after it succeeded.
  async function resolve(): Promise<Outcome> {
    let outcome = await askOnce();
    for (let attempt = 0; attempt < SETTLEMENT_ATTEMPTS && outcome.kind === "error"; attempt += 1) {
      await new Promise((wake) => setTimeout(wake, SETTLEMENT_INTERVAL_MS));
      outcome = await askOnce();
    }
    return outcome;
  }

  async function verifyHuman() {
    setBusy("human");
    setOutcome(null);
    try {
      const response = await fetch("/api/world/verify", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ wallet: wallet.address }),
      });
      const attestation = (await response.json()) as { sponsored?: boolean; error?: string };
      if (!attestation.sponsored) throw new Error(attestation.error ?? "Verification failed");
      setOutcome(await resolve());
    } catch (cause) {
      setOutcome({ kind: "error", message: asMessage(cause) });
    } finally {
      setBusy(null);
    }
  }

  async function pay() {
    setBusy("pay");
    setOutcome(null);
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
            TIER_INDEX[quote.tier] ?? 2,
            BigInt(quote.amount),
            quote.expiresAt,
            quote.signature as `0x${string}`,
          ],
        }),
      });
      setOutcome(await resolve());
    } catch (cause) {
      setOutcome({ kind: "error", message: asMessage(cause) });
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="mt-8 space-y-3">
      <button onClick={verifyHuman} disabled={busy !== null} className={primary}>
        {busy === "human" ? "Verifying" : "I'm a person - free, no gas"}
      </button>
      {!dangerous && (
        <button onClick={pay} disabled={busy !== null} className={secondary}>
          {busy === "pay" ? "Paying" : `Pay ${formatUsdc(BigInt(quote.amount))} instead`}
        </button>
      )}
      {outcome?.kind === "error" && <p className="text-sm text-red-600">{outcome.message}</p>}
    </div>
  );
}

/// Postage never kept the message that was refused, so the way through is
/// either to send it again from their mail client or to paste it here. This is
/// the second, which saves the trip without anyone storing mail.
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
      <div className="mt-8 rounded-xl border border-green-200 bg-green-50 p-4 text-sm text-green-800">
        <p className="font-medium">Delivered.</p>
        <p className="mt-1">It is in their inbox now, and replying comes straight back to you.</p>
      </div>
    );
  }

  return (
    <div className="mt-8 space-y-4">
      <div className="rounded-xl border border-green-200 bg-green-50 p-4 text-sm text-green-800">
        <p className="font-medium">
          {reason === "human" ? "Verified. That cost you nothing." : "Paid."}
        </p>
        <p className="mt-1">
          The hold on your message ran out before this was cleared, so it is gone. Paste it below
          and we will deliver it now.
        </p>
      </div>

      <input
        value={subject}
        onChange={(event) => setSubject(event.target.value)}
        placeholder="Subject"
        className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
      />
      <textarea
        value={body}
        onChange={(event) => setBody(event.target.value)}
        rows={7}
        placeholder={`Paste what you wrote to ${handle}@usepostage.com`}
        className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
      />
      <button onClick={deliver} disabled={sending || body.trim().length === 0} className={primary}>
        {sending ? "Delivering" : "Deliver it now"}
      </button>
      {error && <p className="text-sm text-red-600">{error}</p>}
    </div>
  );
}

function asMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

const primary =
  "w-full rounded-lg bg-neutral-900 px-5 py-3 text-sm font-medium text-white hover:bg-neutral-700 disabled:opacity-50";
const secondary =
  "w-full rounded-lg border border-neutral-300 bg-white px-5 py-3 text-sm font-medium hover:bg-neutral-50 disabled:opacity-50";
const note = "mt-8 text-sm text-neutral-500";

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
  const [lane, setLane] = useState<"choosing" | "paying">("choosing");
  const [busy, setBusy] = useState<"human" | "pay" | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);

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

  /// No wallet anywhere on this path. Proving personhood is not a payment, so
  /// it should not need an account to make one.
  async function verifyHuman() {
    setBusy("human");
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
      setBusy(null);
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
      <button onClick={verifyHuman} disabled={busy !== null} className={primary}>
        {busy === "human" ? "Verifying" : "A person wrote this"}
      </button>
      <p className="px-1 text-xs text-neutral-500">
        Prove it with World ID and your message is delivered. Free, no wallet, nothing to install
        beyond the World app.
      </p>

      {!dangerous && (
        <>
          <button onClick={() => setLane("paying")} disabled={busy !== null} className={secondary}>
            A machine sent this — pay {formatUsdc(BigInt(quote.amount))}
          </button>
          <p className="px-1 text-xs text-neutral-500">
            Automated mail pays the recipient for the attention. You will need somewhere to pay
            from, which takes an email address.
          </p>
        </>
      )}

      {outcome?.kind === "error" && <p className="text-sm text-red-600">{outcome.message}</p>}
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
      body: JSON.stringify({ token, wallet: wallet.address }),
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
            TIER_INDEX[quote.tier] ?? 2,
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

  if (!ready) return <p className={note}>Loading</p>;

  return (
    <div className="mt-8 space-y-3">
      {!authenticated ? (
        <>
          <button onClick={login} className={primary}>
            Set up a way to pay
          </button>
          <p className="px-1 text-xs text-neutral-500">
            An email address is enough. It creates a wallet for you on Arc; there is nothing to
            install.
          </p>
        </>
      ) : !wallet ? (
        <p className={note}>Setting up your wallet</p>
      ) : (
        <button onClick={pay} disabled={busy} className={primary}>
          {busy ? "Paying" : `Pay ${formatUsdc(BigInt(quote.amount))} and deliver it`}
        </button>
      )}

      <button
        onClick={onBack}
        className="w-full px-5 py-2 text-sm text-neutral-500 hover:text-neutral-900"
      >
        Actually, a person wrote it
      </button>
      {error && <p className="text-sm text-red-600">{error}</p>}
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

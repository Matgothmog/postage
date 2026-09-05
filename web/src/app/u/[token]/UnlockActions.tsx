"use client";

import { usePrivy, useSendTransaction, useWallets } from "@privy-io/react-auth";
import { useCallback, useEffect, useState } from "react";
import { encodeFunctionData } from "viem";
import { HUMAN_REGISTRY, POSTAGE_ESCROW, escrowAbi, registryAbi } from "@/lib/contracts";
import { formatUsdc } from "@/lib/format";

interface Props {
  token: string;
  messageHash: string;
  recipientWallet: string;
  price: string;
}

interface PriceQuote {
  price: string;
  multiplierBps: number;
  free: boolean;
  reasons: string[];
}

type Outcome = { kind: "delivered"; reason: string } | { kind: "error"; message: string };

export function UnlockActions({ token, messageHash, recipientWallet, price }: Props) {
  const { ready, authenticated, login } = usePrivy();
  const { wallets } = useWallets();
  const { sendTransaction } = useSendTransaction();

  const [busy, setBusy] = useState<"human" | "stamp" | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [quote, setQuote] = useState<PriceQuote | null>(null);

  const wallet = wallets[0];
  const address = wallet?.address;

  // What this particular sender pays is only knowable once we know who they
  // are, so the quote is fetched after the wallet connects rather than
  // rendered with the page.
  const loadQuote = useCallback(async () => {
    if (!address) return;
    const response = await fetch(`/api/price?token=${token}&wallet=${address}`);
    if (response.ok) setQuote((await response.json()) as PriceQuote);
  }, [address, token]);

  useEffect(() => {
    loadQuote().catch(() => setQuote(null));
  }, [loadQuote]);

  if (!ready) return null;

  if (!authenticated) {
    return (
      <button onClick={login} className={primaryButton}>
        Continue
      </button>
    );
  }

  if (outcome?.kind === "delivered") {
    return (
      <p className="mt-8 rounded-xl border border-green-200 bg-green-50 p-4 text-sm text-green-800">
        Delivered.{" "}
        {outcome.reason === "human"
          ? "You verified, so it cost you nothing."
          : "Your postage is in escrow and comes back if they mark the message legitimate."}
      </p>
    );
  }

  async function unlock(): Promise<Outcome> {
    const response = await fetch("/api/mail/unlock", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ token, wallet: wallet.address }),
    });
    const result = (await response.json()) as { status?: string; reason?: string; error?: string };
    if (result.status === "delivered") {
      return { kind: "delivered", reason: result.reason ?? "human" };
    }
    return { kind: "error", message: result.error ?? "Still held" };
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
      const attestation = (await response.json()) as {
        nullifierHash?: string;
        expiresAt?: number;
        signature?: string;
        error?: string;
      };
      if (!attestation.signature) throw new Error(attestation.error ?? "Verification failed");

      await sendTransaction({
        to: HUMAN_REGISTRY,
        data: encodeFunctionData({
          abi: registryAbi,
          functionName: "attest",
          args: [
            wallet.address as `0x${string}`,
            attestation.nullifierHash as `0x${string}`,
            attestation.expiresAt!,
            attestation.signature as `0x${string}`,
          ],
        }),
      });

      setOutcome(await unlock());
    } catch (cause) {
      setOutcome({ kind: "error", message: asMessage(cause) });
    } finally {
      setBusy(null);
    }
  }

  async function payPostage() {
    setBusy("stamp");
    setOutcome(null);
    try {
      await sendTransaction({
        to: POSTAGE_ESCROW,
        value: BigInt(quote?.price ?? price),
        data: encodeFunctionData({
          abi: escrowAbi,
          functionName: "postStamp",
          args: [messageHash as `0x${string}`, recipientWallet as `0x${string}`],
        }),
      });
      setOutcome(await unlock());
    } catch (cause) {
      setOutcome({ kind: "error", message: asMessage(cause) });
    } finally {
      setBusy(null);
    }
  }

  const stampPrice = BigInt(quote?.price ?? price);

  return (
    <div className="mt-8 space-y-3">
      {quote && !quote.free && (
        <div className="rounded-xl border border-neutral-200 bg-white p-4 text-sm">
          <p className="font-medium">
            Your postage is {formatUsdc(stampPrice)}
            {quote.multiplierBps !== 10_000 && (
              <span className="ml-1 font-normal text-neutral-500">
                ({(quote.multiplierBps / 10_000).toFixed(2)}x this inbox&apos;s base)
              </span>
            )}
          </p>
          <ul className="mt-2 space-y-1 text-neutral-600">
            {quote.reasons.map((reason) => (
              <li key={reason}>{reason}</li>
            ))}
          </ul>
        </div>
      )}
      <button onClick={verifyHuman} disabled={busy !== null} className={primaryButton}>
        {busy === "human" ? "Verifying" : "I'm a person — send for free"}
      </button>
      <button onClick={payPostage} disabled={busy !== null} className={secondaryButton}>
        {busy === "stamp" ? "Paying" : `Attach ${formatUsdc(stampPrice)} postage instead`}
      </button>
      {outcome?.kind === "error" && (
        <p className="text-sm text-red-600">{outcome.message}</p>
      )}
    </div>
  );
}

function asMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

const primaryButton =
  "w-full rounded-lg bg-neutral-900 px-5 py-3 text-sm font-medium text-white hover:bg-neutral-700 disabled:opacity-50";
const secondaryButton =
  "w-full rounded-lg border border-neutral-300 bg-white px-5 py-3 text-sm font-medium hover:bg-neutral-50 disabled:opacity-50";

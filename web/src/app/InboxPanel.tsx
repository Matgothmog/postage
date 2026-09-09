"use client";

import { useSendTransaction } from "@privy-io/react-auth";
import Link from "next/link";
import { useCallback, useEffect, useState } from "react";
import { encodeFunctionData, formatUnits } from "viem";
import { StampCard, field, primaryButton, secondaryButton } from "@/components/chrome";
import { publicClient } from "@/lib/client";
import { POSTAGE_ESCROW, USDC_DECIMALS, escrowAbi } from "@/lib/contracts";
import { causeMessage } from "@/lib/errors";
import { formatUsdc, parseUsdc, shortAddress } from "@/lib/format";
import { postageAddress } from "@/lib/handle";

export interface Inbox {
  handle: string;
  destination: string;
}

interface Standing {
  earned: bigint;
  floor: bigint;
  /// False while the owner has never called setFloorPrice, in which case `floor`
  /// is the default the escrow charges on their behalf.
  chosen: boolean;
}

/// Everything the panel shows, read from the chain rather than from our own
/// copy of it. Outside the component so the reads stay testable and so nothing
/// touches React state until the answers are in.
async function readStanding(owner: `0x${string}`): Promise<Standing> {
  const call = (functionName: "earnings" | "floorPrice" | "effectiveFloor") =>
    publicClient.readContract({ address: POSTAGE_ESCROW, abi: escrowAbi, functionName, args: [owner] });

  const [earned, chosen, floor] = await Promise.all([
    call("earnings"),
    call("floorPrice"),
    call("effectiveFloor"),
  ]);
  return { earned, floor, chosen: chosen !== 0n };
}

export function InboxPanel({ inbox, wallet }: { inbox: Inbox; wallet: string }) {
  const { sendTransaction } = useSendTransaction();
  const [standing, setStanding] = useState<Standing | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState<"price" | "claim" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const apply = useCallback((next: Standing) => {
    setStanding(next);
    setDraft(formatUnits(next.floor, USDC_DECIMALS));
  }, []);

  useEffect(() => {
    let live = true;
    void (async () => {
      try {
        const next = await readStanding(wallet as `0x${string}`);
        if (live) apply(next);
      } catch (cause) {
        if (live) setError(causeMessage(cause));
      }
    })();
    return () => {
      live = false;
    };
  }, [wallet, apply]);

  async function send(action: "price" | "claim") {
    setBusy(action);
    setError(null);
    try {
      const data =
        action === "price"
          ? encodeFunctionData({ abi: escrowAbi, functionName: "setFloorPrice", args: [parseUsdc(draft)] })
          : encodeFunctionData({ abi: escrowAbi, functionName: "claimEarnings", args: [wallet as `0x${string}`] });
      await sendTransaction({ to: POSTAGE_ESCROW, data });
      apply(await readStanding(wallet as `0x${string}`));
    } catch (cause) {
      setError(causeMessage(cause));
    } finally {
      setBusy(null);
    }
  }

  return (
    <main className="rise mx-auto w-full max-w-3xl px-6 py-14">
      <div className="flex flex-col gap-10 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <p className="text-[11px] font-semibold uppercase tracking-[0.2em] text-stamp">Live</p>
          <h1 className="mt-4 font-mono text-2xl break-all text-ink sm:text-[1.7rem]">
            {postageAddress(inbox.handle)}
          </h1>
          <p className="mt-2 text-[15px] text-ink-soft">
            Forwards to <span className="font-mono text-ink">{inbox.destination}</span>, untouched.
          </p>
          <p className="mt-1 text-xs text-ink-faint">Paid into {shortAddress(wallet)}</p>
        </div>
        <div className="hidden shrink-0 sm:block">
          <StampCard
            handle={postageAddress(inbox.handle)}
            price={standing ? formatUsdc(standing.floor) : "—"}
            caption="Hand this out instead of your own"
          />
        </div>
      </div>

      <div className="mt-12 grid gap-4 sm:grid-cols-2">
        <section className="rounded-2xl border border-rule bg-card p-6">
          <p className="text-xs uppercase tracking-[0.14em] text-ink-faint">Earned from senders</p>
          <p className="mt-2 font-mono text-3xl tabular-nums text-ink">
            {standing === null ? "—" : formatUsdc(standing.earned)}
          </p>
          <button
            onClick={() => send("claim")}
            disabled={busy !== null || standing === null || standing.earned === 0n}
            className={`${primaryButton} mt-5 w-full`}
          >
            {busy === "claim" ? "Claiming" : "Withdraw to my wallet"}
          </button>
          <p className="mt-3 text-xs leading-relaxed text-ink-faint">
            Verified people and anything urgent reach you free and never appear here. This is what
            the rest paid.
          </p>
        </section>

        <section className="rounded-2xl border border-rule bg-card p-6">
          <label htmlFor="floor" className="text-xs uppercase tracking-[0.14em] text-ink-faint">
            What a stranger pays
          </label>
          <div className="mt-2 flex gap-2">
            <input
              id="floor"
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              inputMode="decimal"
              placeholder="0.01"
              className={`${field} font-mono`}
            />
            <button
              onClick={() => send("price")}
              disabled={busy !== null || draft.trim().length === 0}
              className={secondaryButton}
            >
              {busy === "price" ? "Saving" : "Set"}
            </button>
          </div>
          <p className="mt-3 text-xs leading-relaxed text-ink-faint">
            {standing && !standing.chosen
              ? `Already charging ${formatUsdc(standing.floor)} by default, so there is nothing you have to do here.`
              : "Marketing pays this. Anything trying to deceive you pays ten times it, and still does not arrive."}
          </p>
        </section>
      </div>

      {error && <p className="mt-6 text-sm text-stamp">{error}</p>}

      <p className="mt-10 text-sm text-ink-soft">
        Every payment into this inbox is public.{" "}
        <Link
          href="/network"
          className="text-ink underline decoration-rule-strong underline-offset-4 hover:decoration-ink"
        >
          Watch it settle on the network
        </Link>
        .
      </p>
    </main>
  );
}

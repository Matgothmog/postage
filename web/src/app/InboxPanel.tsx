"use client";

import { useSendTransaction } from "@privy-io/react-auth";
import Link from "next/link";
import { useCallback, useEffect, useState } from "react";
import { encodeFunctionData, formatUnits } from "viem";
import { AddressCard, Callout, field, primaryButton, secondaryButton } from "@/components/chrome";
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

/// The one-click price options. `amount` is the decimal string parseUsdc
/// expects (and setFloorPrice already sends) — never a pre-formatted label,
/// so the chip's active state can compare bigints instead of strings.
const PRICE_PRESETS = [
  { label: "1¢", amount: "0.01" },
  { label: "5¢", amount: "0.05" },
  { label: "25¢", amount: "0.25" },
  { label: "$1", amount: "1" },
] as const;

const priceChipBase =
  "rounded-full border px-4 py-2 text-sm font-medium transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-hi focus-visible:ring-offset-2 focus-visible:ring-offset-bg disabled:opacity-40";
const priceChipActive = "border-accent bg-accent text-on-accent";
const priceChipInactive = "border-line-strong bg-surface-2 text-fg hover:border-accent-hi";

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
  const [customOpen, setCustomOpen] = useState(false);
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

  // `amount` overrides `draft` for the one-click preset chips; the custom
  // field's "Set" button still falls back to `draft`. Either way this is the
  // same setFloorPrice call, same units, same encoding — only the string
  // source changes.
  async function send(action: "price" | "claim", amount?: string) {
    setBusy(action);
    setError(null);
    try {
      const data =
        action === "price"
          ? encodeFunctionData({ abi: escrowAbi, functionName: "setFloorPrice", args: [parseUsdc(amount ?? draft)] })
          : encodeFunctionData({ abi: escrowAbi, functionName: "claimEarnings", args: [wallet as `0x${string}`] });
      await sendTransaction({ to: POSTAGE_ESCROW, data });
      apply(await readStanding(wallet as `0x${string}`));
    } catch (cause) {
      setError(causeMessage(cause));
    } finally {
      setBusy(null);
    }
  }

  // A defaulted floor (the owner never called setFloorPrice) is not a chosen
  // price, so no chip — preset or Custom — should render as active for it.
  const chosenFloor = standing?.chosen ? standing.floor : null;
  const activePreset =
    chosenFloor !== null
      ? (PRICE_PRESETS.find((preset) => parseUsdc(preset.amount) === chosenFloor)?.amount ?? null)
      : null;
  const isCustomActive = chosenFloor !== null && activePreset === null;

  return (
    <main className="rise mx-auto w-full max-w-3xl px-6 py-14">
      <div className="flex flex-col gap-10 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <p className="text-[11px] font-semibold uppercase tracking-[0.2em] text-accent">Live</p>
          <h1 className="mt-4 font-mono text-2xl break-all text-fg sm:text-[1.7rem]">
            {postageAddress(inbox.handle)}
          </h1>
          <p className="mt-2 text-[15px] text-muted">
            → <span className="font-mono text-fg">{inbox.destination}</span>
          </p>
          <p className="mt-1 text-xs text-faint">Paid into {shortAddress(wallet)}</p>
        </div>
        <div className="hidden shrink-0 sm:block">
          <AddressCard
            handle={postageAddress(inbox.handle)}
            price={standing ? formatUsdc(standing.floor) : "—"}
            caption="Hand this out instead of your own"
          />
        </div>
      </div>

      <div className="mt-12 grid gap-4 sm:grid-cols-2">
        <section className="rounded-2xl border border-line-strong bg-surface p-6">
          <p className="text-xs uppercase tracking-[0.14em] text-faint">Earned</p>
          <p className="mt-2 font-mono text-3xl tabular-nums text-fg">
            {standing === null ? "—" : formatUsdc(standing.earned)}
          </p>
          <button
            onClick={() => send("claim")}
            disabled={busy !== null || standing === null || standing.earned === 0n}
            className={`${primaryButton} mt-5 w-full`}
          >
            {busy === "claim" ? "Cashing out…" : "Cash out"}
          </button>
          <p className="mt-3 text-xs leading-relaxed text-faint">Only machines pay. Humans are free.</p>
        </section>

        <section className="rounded-2xl border border-line-strong bg-surface p-6">
          <p className="text-xs uppercase tracking-[0.14em] text-faint">Your price</p>
          <div className="mt-2 flex flex-wrap gap-2">
            {PRICE_PRESETS.map((preset) => (
              <button
                key={preset.amount}
                onClick={() => {
                  setCustomOpen(false);
                  send("price", preset.amount);
                }}
                disabled={busy !== null}
                className={`${priceChipBase} ${activePreset === preset.amount ? priceChipActive : priceChipInactive}`}
              >
                {preset.label}
              </button>
            ))}
            <button
              onClick={() => setCustomOpen((open) => !open)}
              disabled={busy !== null}
              className={`${priceChipBase} ${customOpen || isCustomActive ? priceChipActive : priceChipInactive}`}
            >
              Custom
            </button>
          </div>
          {(customOpen || isCustomActive) && (
            <div className="mt-3 flex gap-2">
              <input
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                inputMode="decimal"
                placeholder="0.01"
                aria-label="Custom price"
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
          )}
          <p className="mt-3 text-xs leading-relaxed text-faint">
            {standing && !standing.chosen
              ? `Charging ${formatUsdc(standing.floor)} by default.`
              : "Bots pay this. Phishing pays 10×, and still bounces."}
          </p>
        </section>
      </div>

      {error && (
        <div className="mt-6">
          <Callout tone="bad" title={error} />
        </div>
      )}

      <p className="mt-10 text-sm text-muted">
        Every cent is public.{" "}
        <Link
          href="/network"
          className="text-fg underline decoration-line-strong underline-offset-4 hover:decoration-fg"
        >
          See the ledger →
        </Link>
      </p>
    </main>
  );
}

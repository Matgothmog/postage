"use client";

import { usePrivy, useSendTransaction, useWallets } from "@privy-io/react-auth";
import { useCallback, useEffect, useState } from "react";
import { encodeFunctionData, formatUnits } from "viem";
import { publicClient } from "@/lib/client";
import { POSTAGE_ESCROW, USDC_DECIMALS, escrowAbi } from "@/lib/contracts";
import { formatUsdc, parseUsdc, shortAddress } from "@/lib/format";

interface Inbox {
  handle: string;
  destination: string;
}

export default function Home() {
  const { ready, authenticated, login, logout } = usePrivy();
  const { wallets } = useWallets();
  const wallet = wallets[0];

  const [inbox, setInbox] = useState<Inbox | null>(null);
  const [loaded, setLoaded] = useState(false);

  const refresh = useCallback(async () => {
    if (!wallet?.address) return;
    const response = await fetch(`/api/inbox?wallet=${wallet.address}`);
    if (response.ok) setInbox(((await response.json()) as { inbox: Inbox | null }).inbox);
    setLoaded(true);
  }, [wallet?.address]);

  useEffect(() => {
    refresh().catch(() => setLoaded(true));
  }, [refresh]);

  if (!ready) return <Centered>Loading</Centered>;

  if (!authenticated) {
    return (
      <Centered>
        <h1 className="text-2xl font-semibold tracking-tight">Postage</h1>
        <p className="mt-2 max-w-md text-neutral-600">
          Keep the inbox you already have. Put a filter in front of it that reads what arrives,
          lets the real mail through, and charges whoever sends the rest.
        </p>
        <button
          onClick={login}
          className="mt-6 rounded-lg bg-neutral-900 px-5 py-2.5 text-sm font-medium text-white hover:bg-neutral-700"
        >
          Get an address
        </button>
      </Centered>
    );
  }

  if (!wallet) return <Centered>Setting up your wallet</Centered>;

  return (
    <main className="mx-auto w-full max-w-2xl px-6 py-12">
      <header className="flex items-baseline justify-between">
        <h1 className="text-xl font-semibold tracking-tight">Postage</h1>
        <div className="flex items-baseline gap-4 text-sm">
          <a href="/network" className="text-neutral-500 hover:text-neutral-900">
            Network
          </a>
          <button onClick={logout} className="text-neutral-500 hover:text-neutral-900">
            Sign out
          </button>
        </div>
      </header>

      {loaded && !inbox ? (
        <CreateInbox wallet={wallet.address} onCreated={refresh} />
      ) : inbox ? (
        <InboxPanel inbox={inbox} wallet={wallet.address} />
      ) : null}
    </main>
  );
}

function CreateInbox({ wallet, onCreated }: { wallet: string; onCreated: () => void }) {
  const [handle, setHandle] = useState("");
  const [destination, setDestination] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  async function create() {
    setSaving(true);
    setError(null);
    try {
      const response = await fetch("/api/inbox", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ handle: handle.trim().toLowerCase(), destination: destination.trim(), wallet }),
      });
      const result = (await response.json()) as { error?: string };
      if (!response.ok) throw new Error(result.error ?? "Could not create that inbox");
      onCreated();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="mt-8 rounded-xl border border-neutral-200 bg-white p-5">
      <h2 className="text-sm font-medium">Pick an address to hand out</h2>
      <p className="mt-1 text-sm text-neutral-600">
        Mail sent here is read, judged, and forwarded to the inbox you actually use. You never
        change email provider.
      </p>

      <div className="mt-4 space-y-3">
        <div className="flex items-center gap-2">
          <input
            value={handle}
            onChange={(event) => setHandle(event.target.value)}
            placeholder="you"
            className="w-40 rounded-lg border border-neutral-300 px-3 py-2 text-sm"
          />
          <span className="text-sm text-neutral-500">@usepostage.com</span>
        </div>
        <input
          value={destination}
          onChange={(event) => setDestination(event.target.value)}
          placeholder="forward it to you@gmail.com"
          className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
        />
        <button
          onClick={create}
          disabled={saving || handle.trim().length < 2 || !destination.includes("@")}
          className="rounded-lg bg-neutral-900 px-4 py-2 text-sm font-medium text-white hover:bg-neutral-700 disabled:opacity-50"
        >
          {saving ? "Creating" : "Create"}
        </button>
      </div>
      {error && <p className="mt-3 text-sm text-red-600">{error}</p>}
    </section>
  );
}

function InboxPanel({ inbox, wallet }: { inbox: Inbox; wallet: string }) {
  const { sendTransaction } = useSendTransaction();
  const [earnings, setEarnings] = useState<bigint | null>(null);
  const [floor, setFloor] = useState<{ amount: bigint; chosen: boolean } | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState<"price" | "claim" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const read = useCallback(async () => {
    const inboxAddress = wallet as `0x${string}`;
    const [earned, chosen, effective] = await Promise.all([
      publicClient.readContract({
        address: POSTAGE_ESCROW,
        abi: escrowAbi,
        functionName: "earnings",
        args: [inboxAddress],
      }),
      publicClient.readContract({
        address: POSTAGE_ESCROW,
        abi: escrowAbi,
        functionName: "floorPrice",
        args: [inboxAddress],
      }),
      publicClient.readContract({
        address: POSTAGE_ESCROW,
        abi: escrowAbi,
        functionName: "effectiveFloor",
        args: [inboxAddress],
      }),
    ]);

    setEarnings(earned);
    setFloor({ amount: effective, chosen: chosen !== 0n });
    setDraft(formatUnits(effective, USDC_DECIMALS));
  }, [wallet]);

  useEffect(() => {
    read().catch((cause: unknown) => setError(String(cause)));
  }, [read]);

  async function send(action: "price" | "claim") {
    setBusy(action);
    setError(null);
    try {
      const data =
        action === "price"
          ? encodeFunctionData({ abi: escrowAbi, functionName: "setFloorPrice", args: [parseUsdc(draft)] })
          : encodeFunctionData({ abi: escrowAbi, functionName: "claimEarnings", args: [wallet as `0x${string}`] });
      await sendTransaction({ to: POSTAGE_ESCROW, data });
      await read();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="mt-8 space-y-6">
      <div className="rounded-xl border border-neutral-200 bg-white p-5">
        <p className="font-mono text-lg">{inbox.handle}@usepostage.com</p>
        <p className="mt-1 text-sm text-neutral-500">forwards to {inbox.destination}</p>
      </div>

      <div className="rounded-xl border border-neutral-200 bg-white p-5">
        <div className="flex items-baseline justify-between">
          <span className="text-sm text-neutral-500">Earned from senders</span>
          <span className="font-mono text-lg">
            {earnings === null ? "..." : formatUsdc(earnings)}
          </span>
        </div>
        <button
          onClick={() => send("claim")}
          disabled={busy !== null || earnings === null || earnings === 0n}
          className="mt-4 w-full rounded-lg bg-neutral-900 px-4 py-2 text-sm font-medium text-white hover:bg-neutral-700 disabled:opacity-50"
        >
          {busy === "claim" ? "Claiming" : "Claim to my wallet"}
        </button>
        <p className="mt-2 text-xs text-neutral-500">
          Wallet {shortAddress(wallet)}. Verified people and anything urgent reach you free; this
          is what the rest paid.
        </p>
      </div>

      <div className="rounded-xl border border-neutral-200 bg-white p-5">
        <label htmlFor="floor" className="text-sm font-medium">
          Minimum a stranger pays
        </label>
        <div className="mt-3 flex gap-2">
          <input
            id="floor"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            inputMode="decimal"
            placeholder="0.01"
            className="w-28 rounded-lg border border-neutral-300 px-3 py-2 text-sm"
          />
          <button
            onClick={() => send("price")}
            disabled={busy !== null || draft.trim().length === 0}
            className="rounded-lg border border-neutral-300 px-4 py-2 text-sm font-medium hover:bg-neutral-50 disabled:opacity-50"
          >
            {busy === "price" ? "Saving" : "Update"}
          </button>
        </div>
        <p className="mt-2 text-xs text-neutral-500">
          {floor && !floor.chosen
            ? `Already charging ${formatUsdc(floor.amount)} by default, so there is nothing you have to do here.`
            : "Marketing pays this. Anything trying to deceive you pays ten times it, and still does not arrive."}
        </p>
        {error && <p className="mt-3 text-sm text-red-600">{error}</p>}
      </div>
    </section>
  );
}

function Centered({ children }: { children: React.ReactNode }) {
  return (
    <main className="m-auto flex flex-col items-center px-6 py-24 text-center">{children}</main>
  );
}

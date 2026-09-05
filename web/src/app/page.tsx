"use client";

import { usePrivy, useSendTransaction, useWallets } from "@privy-io/react-auth";
import { useCallback, useEffect, useState } from "react";
import { encodeFunctionData } from "viem";
import { publicClient } from "@/lib/client";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import { formatUsdc, parseUsdc, shortAddress } from "@/lib/format";

export default function Home() {
  const { ready, authenticated, login, logout } = usePrivy();

  if (!ready) return <Centered>Loading</Centered>;

  if (!authenticated) {
    return (
      <Centered>
        <h1 className="text-2xl font-semibold tracking-tight">Postage</h1>
        <p className="mt-2 max-w-sm text-neutral-600">
          An inbox where attention has a price, and being human makes it free.
        </p>
        <button
          onClick={login}
          className="mt-6 rounded-lg bg-neutral-900 px-5 py-2.5 text-sm font-medium text-white hover:bg-neutral-700"
        >
          Sign in
        </button>
      </Centered>
    );
  }

  return (
    <main className="mx-auto w-full max-w-2xl px-6 py-12">
      <header className="flex items-baseline justify-between">
        <h1 className="text-xl font-semibold tracking-tight">Postage</h1>
        <button onClick={logout} className="text-sm text-neutral-500 hover:text-neutral-900">
          Sign out
        </button>
      </header>
      <InboxSettings />
    </main>
  );
}

function InboxSettings() {
  const { wallets } = useWallets();
  const { sendTransaction } = useSendTransaction();
  const wallet = wallets[0];

  const [price, setPrice] = useState<bigint | null>(null);
  const [balance, setBalance] = useState<bigint | null>(null);
  const [draft, setDraft] = useState("0.01");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const address = wallet?.address as `0x${string}` | undefined;

  const refresh = useCallback(async () => {
    if (!address) return;
    const [onchainPrice, onchainBalance] = await Promise.all([
      publicClient.readContract({
        address: POSTAGE_ESCROW,
        abi: escrowAbi,
        functionName: "price",
        args: [address],
      }),
      publicClient.getBalance({ address }),
    ]);
    setPrice(onchainPrice);
    setBalance(onchainBalance);
  }, [address]);

  useEffect(() => {
    refresh().catch((cause: unknown) => setError(String(cause)));
  }, [refresh]);

  async function save() {
    if (!address) return;
    setPending(true);
    setError(null);
    try {
      await sendTransaction({
        to: POSTAGE_ESCROW,
        data: encodeFunctionData({
          abi: escrowAbi,
          functionName: "setPrice",
          args: [parseUsdc(draft)],
        }),
      });
      await refresh();
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setPending(false);
    }
  }

  if (!wallet) return <p className="mt-8 text-sm text-neutral-500">Creating your wallet</p>;

  return (
    <section className="mt-8 space-y-6">
      <div className="rounded-xl border border-neutral-200 bg-white p-5">
        <Row label="Wallet" value={shortAddress(wallet.address)} />
        <Row
          label="Balance"
          value={balance === null ? "..." : `${formatUsdc(balance)} USDC`}
        />
        <Row
          label="Postage price"
          value={price === null ? "..." : formatUsdc(price)}
        />
      </div>

      <div className="rounded-xl border border-neutral-200 bg-white p-5">
        <label htmlFor="price" className="text-sm font-medium">
          What should a stranger pay to reach you?
        </label>
        <p className="mt-1 text-sm text-neutral-600">
          Verified humans always send free. This is what everyone else escrows, and you give it
          back when the message turns out to be worth reading.
        </p>
        <div className="mt-4 flex gap-2">
          <input
            id="price"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            inputMode="decimal"
            className="w-32 rounded-lg border border-neutral-300 px-3 py-2 text-sm"
          />
          <button
            onClick={save}
            disabled={pending}
            className="rounded-lg bg-neutral-900 px-4 py-2 text-sm font-medium text-white hover:bg-neutral-700 disabled:opacity-50"
          >
            {pending ? "Saving" : "Save"}
          </button>
        </div>
        {error && <p className="mt-3 text-sm text-red-600">{error}</p>}
      </div>
    </section>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex justify-between border-b border-neutral-100 py-2 text-sm last:border-0">
      <span className="text-neutral-500">{label}</span>
      <span className="font-mono">{value}</span>
    </div>
  );
}

function Centered({ children }: { children: React.ReactNode }) {
  return (
    <main className="m-auto flex flex-col items-center px-6 py-24 text-center">{children}</main>
  );
}

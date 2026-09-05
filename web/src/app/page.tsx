"use client";

import { usePrivy, useSendTransaction, useWallets } from "@privy-io/react-auth";
import { useCallback, useEffect, useState } from "react";
import { encodeFunctionData } from "viem";
import { publicClient } from "@/lib/client";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import { formatUsdc, parseUsdc, shortAddress } from "@/lib/format";
import { ClaimInbox } from "./ClaimInbox";
import { type InboxMessage, MessageList } from "./MessageList";

export default function Home() {
  const { ready, authenticated, login, logout } = usePrivy();
  const { wallets } = useWallets();
  const wallet = wallets[0];

  const [localPart, setLocalPart] = useState<string | null>(null);
  const [messages, setMessages] = useState<InboxMessage[]>([]);
  const [loaded, setLoaded] = useState(false);

  const address = wallet?.address;

  const refresh = useCallback(async () => {
    if (!address) return;
    const response = await fetch(`/api/inbox?wallet=${address}`);
    if (!response.ok) return;
    const data = (await response.json()) as {
      localPart: string | null;
      messages: InboxMessage[];
    };
    setLocalPart(data.localPart);
    setMessages(data.messages);
    setLoaded(true);
  }, [address]);

  useEffect(() => {
    refresh().catch(() => setLoaded(true));
  }, [refresh]);

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

  if (!wallet) return <Centered>Setting up your wallet</Centered>;

  return (
    <main className="mx-auto w-full max-w-2xl px-6 py-12">
      <header className="flex items-baseline justify-between">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Postage</h1>
          {localPart && (
            <p className="mt-0.5 font-mono text-sm text-neutral-500">
              {localPart}@usepostage.com
            </p>
          )}
        </div>
        <div className="flex items-baseline gap-4 text-sm">
          <a href="/network" className="text-neutral-500 hover:text-neutral-900">
            Network
          </a>
          <button onClick={logout} className="text-neutral-500 hover:text-neutral-900">
            Sign out
          </button>
        </div>
      </header>

      {loaded && !localPart ? (
        <ClaimInbox wallet={wallet.address} onClaimed={refresh} />
      ) : (
        <>
          <MessageList messages={messages} onSettled={refresh} />
          <InboxSettings address={wallet.address} />
        </>
      )}
    </main>
  );
}

function InboxSettings({ address }: { address: string }) {
  const { sendTransaction } = useSendTransaction();
  const [price, setPrice] = useState<bigint | null>(null);
  const [balance, setBalance] = useState<bigint | null>(null);
  const [draft, setDraft] = useState("0.01");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const read = useCallback(async () => {
    const [onchainPrice, onchainBalance] = await Promise.all([
      publicClient.readContract({
        address: POSTAGE_ESCROW,
        abi: escrowAbi,
        functionName: "price",
        args: [address as `0x${string}`],
      }),
      publicClient.getBalance({ address: address as `0x${string}` }),
    ]);
    setPrice(onchainPrice);
    setBalance(onchainBalance);
  }, [address]);

  useEffect(() => {
    read().catch((cause: unknown) => setError(String(cause)));
  }, [read]);

  async function save() {
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
      await read();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setPending(false);
    }
  }

  return (
    <section className="mt-10 rounded-xl border border-neutral-200 bg-white p-5">
      <div className="flex justify-between text-sm">
        <span className="text-neutral-500">Wallet</span>
        <span className="font-mono">{shortAddress(address)}</span>
      </div>
      <div className="mt-2 flex justify-between text-sm">
        <span className="text-neutral-500">Balance</span>
        <span className="font-mono">
          {balance === null ? "..." : `${formatUsdc(balance)} USDC`}
        </span>
      </div>
      <div className="mt-2 flex justify-between text-sm">
        <span className="text-neutral-500">Postage from strangers</span>
        <span className="font-mono">{price === null ? "..." : formatUsdc(price)}</span>
      </div>

      <div className="mt-4 flex gap-2 border-t border-neutral-100 pt-4">
        <input
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          inputMode="decimal"
          className="w-28 rounded-lg border border-neutral-300 px-3 py-2 text-sm"
        />
        <button
          onClick={save}
          disabled={pending}
          className="rounded-lg border border-neutral-300 px-4 py-2 text-sm font-medium hover:bg-neutral-50 disabled:opacity-50"
        >
          {pending ? "Saving" : "Update price"}
        </button>
      </div>
      <p className="mt-2 text-xs text-neutral-500">
        Verified people always send free. Reputation decides what everyone else pays, from this
        price up to five times it.
      </p>
      {error && <p className="mt-3 text-sm text-red-600">{error}</p>}
    </section>
  );
}

function Centered({ children }: { children: React.ReactNode }) {
  return (
    <main className="m-auto flex flex-col items-center px-6 py-24 text-center">{children}</main>
  );
}

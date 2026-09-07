"use client";

import { usePrivy, useSendTransaction, useSignMessage, useWallets } from "@privy-io/react-auth";
import { useCallback, useEffect, useState } from "react";
import { encodeFunctionData, formatUnits } from "viem";
import { publicClient } from "@/lib/client";
import { POSTAGE_ESCROW, USDC_DECIMALS, escrowAbi } from "@/lib/contracts";
import { formatUsdc, parseUsdc, shortAddress } from "@/lib/format";
import { claimStatement, readStatement } from "@/lib/statements";

interface Inbox {
  handle: string;
  destination: string;
}

export default function Home() {
  const { ready, authenticated, login, logout } = usePrivy();
  const { wallets } = useWallets();
  const { signMessage } = useSignMessage();
  const wallet = wallets[0];

  const [inbox, setInbox] = useState<Inbox | null>(null);
  const [loaded, setLoaded] = useState(false);

  // The row holds the address the user actually reads, so the server will not
  // hand it over on the strength of a wallet address alone - those are public.
  const refresh = useCallback(async () => {
    const address = wallet?.address;
    if (!address) return;
    try {
      const issuedAt = Math.floor(Date.now() / 1000);
      const { signature } = await signMessage(
        { message: readStatement(address, issuedAt) },
        { address, uiOptions: { showWalletUIs: false } }
      );
      const response = await fetch("/api/inbox", {
        headers: {
          "x-postage-wallet": address,
          "x-postage-issued": String(issuedAt),
          "x-postage-signature": signature,
        },
      });
      if (response.ok) setInbox(((await response.json()) as { inbox: Inbox | null }).inbox);
    } finally {
      setLoaded(true);
    }
  }, [wallet?.address, signMessage]);

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

interface ClaimState {
  handle: string;
  destination: string;
  codeVerified: boolean;
  cloudflareVerified: boolean;
}

function CreateInbox({ wallet, onCreated }: { wallet: string; onCreated: () => void }) {
  const [claim, setClaim] = useState<ClaimState | null>(null);

  if (!claim) return <StartClaim wallet={wallet} onStarted={setClaim} />;
  return <ConfirmClaim claim={claim} onClaim={setClaim} onLive={onCreated} />;
}

function StartClaim({ wallet, onStarted }: { wallet: string; onStarted: (claim: ClaimState) => void }) {
  const { signMessage } = useSignMessage();
  const [handle, setHandle] = useState("");
  const [destination, setDestination] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  async function create() {
    setSaving(true);
    setError(null);
    try {
      const name = handle.trim().toLowerCase();
      const to = destination.trim().toLowerCase();
      const issuedAt = Math.floor(Date.now() / 1000);
      const { signature } = await signMessage(
        { message: claimStatement(name, to, wallet, issuedAt) },
        { address: wallet, uiOptions: { showWalletUIs: false } }
      );

      const response = await fetch("/api/inbox", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ handle: name, destination: to, wallet, issuedAt, signature }),
      });
      const result = (await response.json()) as ClaimState & { error?: string };
      if (!response.ok) throw new Error(result.error ?? "Could not claim that address");
      onStarted(result);
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
          {saving ? "Sending" : "Create"}
        </button>
        <p className="text-xs text-neutral-500">
          We will email that address to check you can read it. Nothing is forwarded anywhere until
          you confirm.
        </p>
      </div>
      {error && <p className="mt-3 text-sm text-red-600">{error}</p>}
    </section>
  );
}

/// Both confirmations are real and neither substitutes for the other.
/// Cloudflare will not forward to an address it has not verified; our own code
/// is what ties this claim to whoever is making it, because a Cloudflare
/// destination someone else verified already reads as verified to us.
function ConfirmClaim({
  claim,
  onClaim,
  onLive,
}: {
  claim: ClaimState;
  onClaim: (claim: ClaimState) => void;
  onLive: () => void;
}) {
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const apply = useCallback(
    (next: { codeVerified: boolean; cloudflareVerified: boolean; live: boolean }) => {
      if (next.live) {
        onLive();
        return;
      }
      onClaim({
        ...claim,
        codeVerified: claim.codeVerified || next.codeVerified,
        cloudflareVerified: claim.cloudflareVerified || next.cloudflareVerified,
      });
    },
    [claim, onClaim, onLive]
  );

  // Cloudflare's half turns green when the user clicks the link in its email,
  // which happens outside this page, so it has to be asked for.
  useEffect(() => {
    if (claim.cloudflareVerified) return;
    const poll = setInterval(async () => {
      const response = await fetch(`/api/inbox/verify?handle=${claim.handle}`);
      if (response.ok) apply(await response.json());
    }, 4000);
    return () => clearInterval(poll);
  }, [claim.handle, claim.cloudflareVerified, apply]);

  async function submit() {
    setBusy(true);
    setError(null);
    try {
      const response = await fetch("/api/inbox/verify", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ handle: claim.handle, code: code.trim() }),
      });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error ?? "Could not check that code");
      apply(result);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="mt-8 rounded-xl border border-neutral-200 bg-white p-5">
      <h2 className="text-sm font-medium">Check {claim.destination}</h2>
      <p className="mt-1 text-sm text-neutral-600">
        {claim.codeVerified
          ? "Cloudflare carries the mail and has just sent you a link of its own. Click it and you are done — often it is already sorted and this finishes on its own."
          : `Enter the code we sent, and ${claim.handle}@usepostage.com is nearly yours.`}
      </p>

      <div className="mt-5 space-y-5">
        <Step done={claim.codeVerified} n={1} label="Enter the code from Postage">
          {claim.codeVerified ? null : (
            <div className="mt-2 flex gap-2">
              <input
                value={code}
                onChange={(event) => setCode(event.target.value)}
                inputMode="numeric"
                maxLength={6}
                placeholder="000000"
                className="w-32 rounded-lg border border-neutral-300 px-3 py-2 font-mono text-sm tracking-widest"
              />
              <button
                onClick={submit}
                disabled={busy || code.trim().length !== 6}
                className="rounded-lg bg-neutral-900 px-4 py-2 text-sm font-medium text-white hover:bg-neutral-700 disabled:opacity-50"
              >
                {busy ? "Checking" : "Confirm"}
              </button>
            </div>
          )}
        </Step>

        <Step done={claim.cloudflareVerified} n={2} label="Click the link from Cloudflare">
          {claim.cloudflareVerified ? null : claim.codeVerified ? (
            <p className="mt-1 text-xs text-neutral-500">
              Waiting. Cloudflare will not carry mail to an address it has not checked itself, and
              only the person reading that mailbox can answer it. Sent from cloudflare.com, so look
              in spam if it is not there.
            </p>
          ) : (
            <p className="mt-1 text-xs text-neutral-400">
              Nothing to do yet. We ask Cloudflare for this the moment your code goes in, so you
              only ever deal with one email at a time.
            </p>
          )}
        </Step>
      </div>

      {error && <p className="mt-4 text-sm text-red-600">{error}</p>}
    </section>
  );
}

function Step({
  done,
  n,
  label,
  children,
}: {
  done: boolean;
  n: number;
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex gap-3">
      <span
        className={`mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full text-xs ${
          done ? "bg-green-600 text-white" : "border border-neutral-300 text-neutral-500"
        }`}
      >
        {done ? "\u2713" : n}
      </span>
      <div className="min-w-0 flex-1">
        <p className={`text-sm ${done ? "text-neutral-400 line-through" : "text-neutral-800"}`}>{label}</p>
        {children}
      </div>
    </div>
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

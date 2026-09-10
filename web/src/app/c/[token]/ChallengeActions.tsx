"use client";

import { usePrivy, useSendTransaction, useWallets } from "@privy-io/react-auth";
import { useEffect, useMemo, useRef, useState } from "react";
import { create as createQrCode } from "qrcode";
import { encodeFunctionData } from "viem";
import { Callout, field, primaryButton, quietButton, secondaryButton } from "@/components/chrome";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import { causeMessage } from "@/lib/errors";
import { formatUsdc } from "@/lib/format";
import { postageAddress } from "@/lib/handle";
import type { QuoteFields } from "@/lib/quote-types";
import { tierIndexOf } from "@/lib/tiers";
import {
  fetchRpContext,
  openSelfieCheckWithIDKit,
  postWorldVerify,
  runSelfieCheck,
} from "@/lib/world-id";

/// Long enough to cover a block on Arc and the indexing behind it, short
/// enough that the sender is not left watching a spinner. Running out is not
/// a verdict on the payment — see `settle` — only the point at which the
/// waiting stops being automatic and becomes theirs to repeat.
const SETTLEMENT_ATTEMPTS = 10;
const SETTLEMENT_INTERVAL_MS = 1_500;

/// Modules of quiet zone around the QR code, matching the library's own
/// default margin — enough for a phone camera to lock on without hunting.
const QR_MARGIN_MODULES = 4;

type Outcome =
  | { kind: "cleared"; reason: string; delivered: boolean }
  | { kind: "charged" }
  | { kind: "error"; message: string };

/// The only outcomes the pay lane may hand back up, and deliberately narrower
/// than `Outcome`: `ChallengeActions` returns `<PayLane>` before it ever
/// reaches its own error render, so an error passed up there is shown to
/// nobody. Anything the sender still has to act on stays inside `PayLane`,
/// next to the controls that can act on it.
type SettledOutcome = Exclude<Outcome, { kind: "error" }>;

export function ChallengeActions({
  token,
  quote,
  dangerous,
  handle,
  lane: initialLane,
  identityMode,
}: {
  token: string;
  quote: QuoteFields;
  dangerous: boolean;
  handle: string;
  /// Which answer they already gave, if any — all three are links the mail
  /// they were sent can carry, so arriving here having chosen should not mean
  /// choosing again. "human" and "paying" each collapse the choice below to
  /// the one full-width button for that answer; "choosing" shows both.
  lane: "choosing" | "human" | "paying";
  /// Decided server-side and handed down as a prop, the same way `dangerous`
  /// and `lane` are — not read from a public env var, which could desync from
  /// the `IDENTITY_MODE` `/api/world/verify` actually enforces.
  identityMode: "live" | "mock";
}) {
  const [lane, setLane] = useState(dangerous ? "choosing" : initialLane);
  const [verifying, setVerifying] = useState(false);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  /// Set only under live mode, once IDKit has a request ready to be answered.
  /// Lets the sender open World App while `pollUntilCompletion` is still
  /// waiting on them — otherwise they are looking at a spinner with no way to
  /// finish it. Rendered below as a code to scan and a link to tap, and
  /// nothing here navigates on the sender's behalf: the poll that finishes
  /// this verification is running on this page, so an automatic redirect
  /// would unload the very thing waiting for the answer. A sender without
  /// World App installed would land on a fallthrough page with the check
  /// already dead behind them, and no way back to the code they never saw.
  const [worldConnectorUri, setWorldConnectorUri] = useState<string | null>(null);

  if (outcome?.kind === "charged") {
    return (
      <div className="mt-8">
        <Callout tone="bad" title="Charged. Still blocked.">
          Paying is the penalty here, not a price.
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
          <Callout tone="good" title="Sent.">
            Same words, same sender. Already in their inbox.
          </Callout>
        </div>
      );
    }
    return <Deliver token={token} handle={handle} />;
  }

  /// No wallet anywhere on this path. Proving personhood is not a payment, so
  /// it should not need an account to make one.
  ///
  /// Under mock mode this posts a bare token, exactly as it always has —
  /// that path is what the one test suite that runs on this machine actually
  /// exercises, so it must stay byte-for-byte the same. Under live mode it
  /// first gets a Selfie Check proof from IDKit before posting at all.
  async function verifyHuman() {
    setVerifying(true);
    setOutcome(null);
    setWorldConnectorUri(null);
    try {
      if (identityMode === "mock") {
        const { delivered } = await postWorldVerify(token);
        setOutcome({ kind: "cleared", reason: "human", delivered });
        return;
      }

      const selfieCheck = await runSelfieCheck({
        appId: process.env.NEXT_PUBLIC_WORLD_APP_ID,
        signal: token,
        fetchRpContext,
        openSelfieCheck: openSelfieCheckWithIDKit,
        onConnectorReady: setWorldConnectorUri,
      });
      if (!selfieCheck.ok) {
        setOutcome({ kind: "error", message: selfieCheck.message });
        return;
      }

      const { delivered } = await postWorldVerify(token, selfieCheck.proof);
      setOutcome({ kind: "cleared", reason: "human", delivered });
    } catch (cause) {
      setOutcome({ kind: "error", message: causeMessage(cause) });
    } finally {
      setVerifying(false);
      setWorldConnectorUri(null);
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

  const payAmount = formatUsdc(BigInt(quote.amount));

  return (
    <div className="mt-8 space-y-3">
      <button onClick={verifyHuman} disabled={verifying} className={`${primaryButton} w-full`}>
        {verifying ? "Checking…" : "I'm human — free"}
      </button>
      <p className="px-1 text-xs leading-relaxed text-faint">World ID. No wallet, no account.</p>

      {worldConnectorUri && (
        <div className="space-y-3 rounded-2xl border border-line bg-surface p-5">
          <a
            href={worldConnectorUri}
            target="_blank"
            rel="noreferrer"
            className={`${primaryButton} w-full`}
          >
            Open World App
          </a>
          <p className="text-center text-xs leading-relaxed text-faint">
            Leave this page open — it finishes on its own when you do.
          </p>
          <div className="mx-auto w-fit rounded-xl bg-white p-3">
            <WorldIdQr uri={worldConnectorUri} />
          </div>
          <p className="text-center text-xs leading-relaxed text-faint">
            {"No World App on this device? Scan this with the phone that has it."}
          </p>
        </div>
      )}

      {!dangerous && lane !== "human" && (
        <>
          <button
            onClick={() => setLane("paying")}
            disabled={verifying}
            className={`${secondaryButton} w-full`}
          >
            {`I'm a bot — pay ${payAmount}`}
          </button>
          <p className="px-1 text-xs leading-relaxed text-faint">Goes to them, not us.</p>
        </>
      )}

      {outcome?.kind === "error" && <p className="text-sm text-bad">{outcome.message}</p>}

      {/* The way back out of the human lane, symmetric with the "I'm human"
          button PayLane offers. Arriving from the mail's human link collapses
          the choice to one button, but it must not lock the other door: World
          ID fails for senders who have no World App, decline it, or hit an
          outage, and paying is the only thing left between them and a hold
          that erases the message. Quiet while the free path is still worth a
          try, a real button once a verification has actually failed. */}
      {!dangerous && lane === "human" && (
        <button
          onClick={() => setLane("paying")}
          disabled={verifying}
          className={`${outcome?.kind === "error" ? secondaryButton : quietButton} w-full`}
        >
          {`Pay ${payAmount} instead`}
        </button>
      )}
    </div>
  );
}

/// The World App connector URI, drawn as a scannable code instead of a link
/// to leave the page for. `qrcode`'s `create()` is a synchronous, pure data
/// transform — no canvas, no Promise — so this renders in the same pass as
/// the URI that produced it, with no loading state of its own to manage.
/// Fixed black-on-white regardless of theme: a QR code's contrast is a
/// scanning requirement, not a themable surface.
function WorldIdQr({ uri }: { uri: string }) {
  const modules = useMemo(() => createQrCode(uri, { errorCorrectionLevel: "M" }).modules, [uri]);
  const dimension = modules.size + QR_MARGIN_MODULES * 2;

  return (
    <svg
      viewBox={`0 0 ${dimension} ${dimension}`}
      className="mx-auto h-44 w-44"
      role="img"
      aria-label="World ID QR code — scan with the World App"
      shapeRendering="crispEdges"
    >
      <rect width={dimension} height={dimension} fill="#ffffff" />
      {Array.from({ length: modules.size }, (_, row) =>
        Array.from({ length: modules.size }, (_, col) =>
          modules.get(row, col) ? (
            <rect
              key={`${row}-${col}`}
              x={col + QR_MARGIN_MODULES}
              y={row + QR_MARGIN_MODULES}
              width={1}
              height={1}
              fill="#000000"
            />
          ) : null
        )
      )}
    </svg>
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
  quote: QuoteFields;
  onSettled: (outcome: SettledOutcome) => void;
  onBack: () => void;
}) {
  const { ready, authenticated, login } = usePrivy();
  const { wallets } = useWallets();
  const { sendTransaction } = useSendTransaction();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /// The hash of a payment that has been broadcast but not yet been seen to
  /// settle. Its presence is what replaces the Pay button with a confirmation
  /// view: an indexer can always lag past whatever window we poll for, and a
  /// sender who is shown "Pay" again after their money has already moved will
  /// reasonably click it and pay twice. The hash is kept because it is the
  /// one thing they can point at while they wait.
  const [broadcastHash, setBroadcastHash] = useState<`0x${string}` | null>(null);
  /// Set by the pay button's own click and consumed by the effect below, so
  /// `login` — which only opens Privy's modal and returns immediately — can
  /// still end in a payment without a second click once the wallet Privy
  /// creates on login actually exists. A ref rather than state: the intent to
  /// pay is not something any render needs to reflect on its own, only
  /// something the next one needs to check.
  const wantsToPayRef = useRef(false);

  const wallet = wallets[0];

  /// Never throws. A resolve endpoint that is unreachable, or that answers
  /// with something that is not JSON, is one more "not yet" for `settle` to
  /// retry — not an exception thrown out from under a payment that has
  /// already succeeded.
  async function askOnce(): Promise<Outcome> {
    try {
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
    } catch (cause) {
      return { kind: "error", message: causeMessage(cause) };
    }
  }

  /// A transaction that has been broadcast is not yet a transaction that has
  /// been mined. Asking once would tell most senders their payment failed a
  /// second after it succeeded.
  ///
  /// Resolves to `null` when the window runs out with the payment still
  /// unaccounted for. That is an answer we do not have yet, not a failure, and
  /// it must never be reported as one: the money has already moved, and no
  /// window is long enough to outlast an indexer having a bad afternoon.
  async function settle(): Promise<SettledOutcome | null> {
    let outcome = await askOnce();
    for (let attempt = 0; attempt < SETTLEMENT_ATTEMPTS && outcome.kind === "error"; attempt += 1) {
      await new Promise((wake) => setTimeout(wake, SETTLEMENT_INTERVAL_MS));
      outcome = await askOnce();
    }
    return outcome.kind === "error" ? null : outcome;
  }

  async function pay() {
    setBusy(true);
    setError(null);
    try {
      const { hash } = await sendTransaction({
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
      // The money has moved. Everything past this line is about telling the
      // sender what happened to it, and this sender is never offered a Pay
      // button again.
      setBroadcastHash(hash);
      const settled = await settle();
      if (settled) onSettled(settled);
    } catch (cause) {
      setError(causeMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  /// The way on from a payment that broadcast but has not been seen to settle:
  /// one more pass of the same polling, on the sender's own timing. It moves
  /// no money — the only control offered once a payment exists must not be
  /// able to make a second one.
  async function recheck() {
    setBusy(true);
    setError(null);
    try {
      const settled = await settle();
      if (settled) {
        onSettled(settled);
        return;
      }
      setError("Still nothing on our side. It can take a few minutes — check again shortly.");
    } finally {
      setBusy(false);
    }
  }

  /// Continues a pay click into the payment itself once login has produced a
  /// wallet, so "Pay" is one click rather than "log in" then "pay". Never
  /// fires on its own: `wantsToPayRef` only ever becomes true inside
  /// `handlePay`, which only ever runs from the button's own click. `pay`
  /// itself is deliberately left out of the dependency list — it is a plain
  /// function redeclared every render, not memoized, and this behaviour must
  /// stay exactly what it already was before this file touched it; the guard
  /// above already makes re-running this effect on an unrelated render a
  /// no-op.
  useEffect(() => {
    if (!wantsToPayRef.current || busy || broadcastHash || !authenticated || !wallet) return;
    wantsToPayRef.current = false;
    void pay();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- pay is unmemoized on purpose, see comment above
  }, [authenticated, wallet, busy, broadcastHash]);

  function handlePay() {
    // Already able to pay right now: fire it directly and leave the ref
    // untouched, or the effect above would see `busy` return to false once
    // this finishes and fire a second, redundant payment.
    if (authenticated && wallet) {
      void pay();
      return;
    }
    wantsToPayRef.current = true;
    if (!authenticated) login();
  }

  /// Ahead of the `ready` gate below on purpose: once a payment exists, its
  /// hash is the most important thing on this page and nothing about Privy's
  /// own readiness should be able to replace it with a spinner.
  if (broadcastHash) {
    return (
      <div className="mt-8 space-y-3">
        <Callout tone="good" title="Paid. Confirming.">
          {"It's on the chain. Confirming can take a few minutes, and paying again would charge you twice."}
        </Callout>
        <p className="px-1 font-mono text-xs break-all text-faint">{broadcastHash}</p>
        <button onClick={recheck} disabled={busy} className={`${primaryButton} w-full`}>
          {busy ? "Checking…" : "Check again"}
        </button>
        {error && <p className="text-sm text-warn">{error}</p>}
      </div>
    );
  }

  if (!ready) return <p className="mt-8 text-sm text-faint">Loading</p>;

  const payLabel = busy
    ? "Paying…"
    : authenticated && !wallet
      ? "Setting up your wallet…"
      : `Pay ${formatUsdc(BigInt(quote.amount))}`;

  return (
    <div className="mt-8 space-y-3">
      <button
        onClick={handlePay}
        disabled={busy || (authenticated && !wallet)}
        className={`${primaryButton} w-full`}
      >
        {payLabel}
      </button>

      {/* Shut while a transaction is in flight: leaving unmounts this lane,
          and with it the record of a payment that may already have been
          broadcast a moment later. */}
      <button onClick={onBack} disabled={busy} className={`${quietButton} w-full`}>
        {"I'm human"}
      </button>
      {error && <p className="text-sm text-bad">{error}</p>}
    </div>
  );
}

/// The way back when the hold has already run out, or when the send failed. The
/// message it puts through is one the sender writes here, so it goes out under
/// our name rather than pretending to be theirs.
function Deliver({ token, handle }: { token: string; handle: string }) {
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
      setError(causeMessage(cause));
    } finally {
      setSending(false);
    }
  }

  if (sent) {
    return (
      <div className="mt-8">
        <Callout tone="good" title="Sent.">
          Replies come straight to you.
        </Callout>
      </div>
    );
  }

  return (
    <div className="mt-8 space-y-4">
      <Callout tone="good" title="Cleared.">
        The hold expired. Paste it again.
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
        {sending ? "Sending…" : "Send"}
      </button>
      {error && <p className="text-sm text-bad">{error}</p>}
    </div>
  );
}

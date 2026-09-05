"use client";

import { useSendTransaction } from "@privy-io/react-auth";
import { useState } from "react";
import { encodeFunctionData } from "viem";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import { formatUsdc } from "@/lib/format";

export interface InboxMessage {
  id: string;
  message_hash: string;
  sender: string;
  subject: string;
  body: string;
  status: "held" | "delivered";
  released_by: "human" | "stamp" | "known" | null;
  received_at: number;
  stampStatus: "None" | "Held" | "Released" | "Claimed" | "Expired";
  stampAmount: string;
}

export function MessageList({
  messages,
  onSettled,
}: {
  messages: InboxMessage[];
  onSettled: () => void;
}) {
  if (messages.length === 0) {
    return (
      <p className="mt-8 rounded-xl border border-dashed border-neutral-300 p-8 text-center text-sm text-neutral-500">
        Nothing yet. Mail sent to your address will appear here.
      </p>
    );
  }

  return (
    <ul className="mt-8 space-y-3">
      {messages.map((message) => (
        <Message key={message.id} message={message} onSettled={onSettled} />
      ))}
    </ul>
  );
}

function Message({ message, onSettled }: { message: InboxMessage; onSettled: () => void }) {
  const { sendTransaction } = useSendTransaction();
  const [busy, setBusy] = useState<"release" | "claim" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const settleable = message.stampStatus === "Held";

  async function settle(action: "release" | "claim") {
    setBusy(action);
    setError(null);
    try {
      await sendTransaction({
        to: POSTAGE_ESCROW,
        data: encodeFunctionData({
          abi: escrowAbi,
          functionName: action,
          args: [message.message_hash as `0x${string}`],
        }),
      });
      onSettled();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }

  return (
    <li className="rounded-xl border border-neutral-200 bg-white p-4">
      <div className="flex items-baseline justify-between gap-3">
        <p className="truncate text-sm font-medium">{message.subject}</p>
        <Badge message={message} />
      </div>
      <p className="mt-1 truncate text-sm text-neutral-500">{message.sender}</p>

      {message.status === "delivered" ? (
        <p className="mt-3 whitespace-pre-wrap text-sm text-neutral-700">{message.body}</p>
      ) : (
        <p className="mt-3 text-sm text-neutral-500">
          Waiting for the sender to verify or attach postage.
        </p>
      )}

      {settleable && (
        <div className="mt-4 flex flex-wrap items-center gap-2 border-t border-neutral-100 pt-3">
          <p className="mr-auto text-sm text-neutral-600">
            {formatUsdc(BigInt(message.stampAmount))} in escrow
          </p>
          <button
            onClick={() => settle("release")}
            disabled={busy !== null}
            className="rounded-lg border border-neutral-300 px-3 py-1.5 text-sm hover:bg-neutral-50 disabled:opacity-50"
          >
            {busy === "release" ? "Refunding" : "Worth reading — refund"}
          </button>
          <button
            onClick={() => settle("claim")}
            disabled={busy !== null}
            className="rounded-lg bg-neutral-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-neutral-700 disabled:opacity-50"
          >
            {busy === "claim" ? "Claiming" : "Spam — keep it"}
          </button>
        </div>
      )}
      {error && <p className="mt-2 text-sm text-red-600">{error}</p>}
    </li>
  );
}

function Badge({ message }: { message: InboxMessage }) {
  const label =
    message.status === "held"
      ? "held"
      : message.released_by === "human"
        ? "verified sender"
        : message.released_by === "stamp"
          ? "paid postage"
          : "known sender";

  const tone =
    message.status === "held"
      ? "bg-amber-50 text-amber-700"
      : message.released_by === "human"
        ? "bg-green-50 text-green-700"
        : "bg-neutral-100 text-neutral-600";

  return <span className={`shrink-0 rounded-full px-2 py-0.5 text-xs ${tone}`}>{label}</span>;
}

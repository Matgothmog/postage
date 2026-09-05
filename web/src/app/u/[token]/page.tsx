import { notFound } from "next/navigation";
import { publicClient } from "@/lib/client";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import { messageByToken, walletForInbox } from "@/lib/db";
import { formatUsdc } from "@/lib/format";
import { UnlockActions } from "./UnlockActions";

export default async function UnlockPage({ params }: PageProps<"/u/[token]">) {
  const { token } = await params;

  const message = messageByToken(token);
  if (!message) notFound();

  const recipientWallet = walletForInbox(message.recipient_local);
  if (!recipientWallet) notFound();

  const price = await publicClient.readContract({
    address: POSTAGE_ESCROW,
    abi: escrowAbi,
    functionName: "price",
    args: [recipientWallet as `0x${string}`],
  });

  if (message.status === "delivered") {
    return (
      <Shell>
        <h1 className="text-xl font-semibold">Already delivered</h1>
        <p className="mt-2 text-neutral-600">
          This message reached {message.recipient_local}. Nothing further to do.
        </p>
      </Shell>
    );
  }

  return (
    <Shell>
      <p className="text-sm text-neutral-500">Held message</p>
      <h1 className="mt-1 text-xl font-semibold">{message.subject}</h1>
      <dl className="mt-4 rounded-xl border border-neutral-200 bg-white p-4 text-sm">
        <Row label="From" value={message.sender} />
        <Row label="To" value={message.recipient_local} />
        <Row label="Postage" value={formatUsdc(price)} />
      </dl>

      <p className="mt-6 text-sm text-neutral-600">
        {message.recipient_local} does not know you yet, so this message is waiting. Prove there
        is a person behind it and it goes through for nothing. Otherwise attach postage, which
        you get back the moment they decide the message was worth reading.
      </p>

      <UnlockActions
        token={token}
        messageHash={message.message_hash}
        recipientWallet={recipientWallet}
        price={price.toString()}
      />
    </Shell>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex justify-between border-b border-neutral-100 py-2 last:border-0">
      <dt className="text-neutral-500">{label}</dt>
      <dd className="font-mono">{value}</dd>
    </div>
  );
}

function Shell({ children }: { children: React.ReactNode }) {
  return <main className="mx-auto w-full max-w-lg px-6 py-16">{children}</main>;
}

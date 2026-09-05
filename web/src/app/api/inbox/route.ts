import { type Hex, isAddress } from "viem";
import { publicClient } from "@/lib/client";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import { type HeldMessage, inboxForWallet, inboxMessages } from "@/lib/db";

const STATUS_NAMES = ["None", "Held", "Released", "Claimed", "Expired"] as const;

interface InboxMessage extends HeldMessage {
  stampStatus: (typeof STATUS_NAMES)[number];
  stampAmount: string;
}

/// Everything the signed-in owner needs to render their inbox in one call,
/// including whether each stamp is still sitting in escrow waiting to be
/// released or claimed.
export async function GET(request: Request) {
  const wallet = new URL(request.url).searchParams.get("wallet");
  if (!wallet || !isAddress(wallet)) {
    return Response.json({ error: "A valid wallet is required" }, { status: 400 });
  }

  const localPart = await inboxForWallet(wallet);
  if (!localPart) return Response.json({ localPart: null, messages: [] });

  const messages = await inboxMessages(localPart);
  const enriched = await Promise.all(messages.map(withStampStatus));

  return Response.json({ localPart, messages: enriched });
}

async function withStampStatus(message: HeldMessage): Promise<InboxMessage> {
  try {
    const stamp = await publicClient.readContract({
      address: POSTAGE_ESCROW,
      abi: escrowAbi,
      functionName: "stamps",
      args: [message.message_hash as Hex],
    });
    return {
      ...message,
      stampStatus: STATUS_NAMES[stamp[4]] ?? "None",
      stampAmount: stamp[2].toString(),
    };
  } catch {
    // A chain hiccup should grey out one row, not blank the whole inbox.
    return { ...message, stampStatus: "None", stampAmount: "0" };
  }
}

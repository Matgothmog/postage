import { type Hex, isAddress } from "viem";
import { publicClient } from "@/lib/client";
import {
  HUMAN_REGISTRY,
  POSTAGE_ESCROW,
  escrowAbi,
  registryAbi,
} from "@/lib/contracts";
import { deliver, messageByToken, rememberSender } from "@/lib/db";

const STATUS_HELD = 1;

/// Both routes out of quarantine are checked against the chain rather than
/// trusted from the browser: either the sender holds a live attestation, or
/// their stamp is actually sitting in escrow for this exact message.
export async function POST(request: Request) {
  const { token, wallet } = (await request.json()) as { token?: string; wallet?: string };
  if (!token || !wallet || !isAddress(wallet)) {
    return Response.json({ error: "token and a valid wallet are required" }, { status: 400 });
  }

  const message = messageByToken(token);
  if (!message) return Response.json({ error: "Unknown message" }, { status: 404 });
  if (message.status === "delivered") return Response.json({ status: "delivered" });

  if (await isHuman(wallet)) {
    deliver(token, "human");
    rememberSender(message.recipient_local, message.sender);
    return Response.json({ status: "delivered", reason: "human" });
  }

  if (await hasStamp(message.message_hash as Hex)) {
    deliver(token, "stamp");
    return Response.json({ status: "delivered", reason: "stamp" });
  }

  return Response.json({ status: "held" });
}

async function isHuman(wallet: string): Promise<boolean> {
  return publicClient.readContract({
    address: HUMAN_REGISTRY,
    abi: registryAbi,
    functionName: "isHuman",
    args: [wallet as Hex],
  });
}

async function hasStamp(messageHash: Hex): Promise<boolean> {
  const stamp = await publicClient.readContract({
    address: POSTAGE_ESCROW,
    abi: escrowAbi,
    functionName: "stamps",
    args: [messageHash],
  });
  return stamp[4] === STATUS_HELD;
}

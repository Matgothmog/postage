import { type Hex, isAddress } from "viem";
import { publicClient } from "@/lib/client";
import { HUMAN_REGISTRY, POSTAGE_ESCROW, escrowAbi, registryAbi } from "@/lib/contracts";
import { allowlist, challengeByToken, linkSenderWallet, resolveChallenge } from "@/lib/db";

/// A sender clears the gate one of two ways, and both are checked against the
/// chain rather than taken on the browser's word: they hold a live proof of
/// personhood, or their payment for this exact message has settled.
export async function POST(request: Request) {
  const { token, wallet } = (await request.json()) as { token?: string; wallet?: string };
  if (!token || !wallet || !isAddress(wallet)) {
    return Response.json({ error: "token and a valid wallet are required" }, { status: 400 });
  }

  const challenge = await challengeByToken(token);
  if (!challenge) return Response.json({ error: "Unknown challenge" }, { status: 404 });

  if (await isHuman(wallet)) {
    await allowlist(challenge.handle, challenge.sender, "human");
    await resolveChallenge(token);
    return Response.json({ status: "cleared", reason: "human" });
  }

  if (await hasPaid(challenge.message_id as Hex)) {
    await allowlist(challenge.handle, challenge.sender, "paid");
    // Remembering which wallet paid is what lets the next message from this
    // sender be priced on their record instead of from scratch.
    await linkSenderWallet(challenge.sender, wallet);
    await resolveChallenge(token);
    return Response.json({ status: "cleared", reason: "paid" });
  }

  return Response.json({ status: "pending" });
}

async function isHuman(wallet: string): Promise<boolean> {
  return publicClient.readContract({
    address: HUMAN_REGISTRY,
    abi: registryAbi,
    functionName: "isHuman",
    args: [wallet as Hex],
  });
}

async function hasPaid(messageId: Hex): Promise<boolean> {
  return publicClient.readContract({
    address: POSTAGE_ESCROW,
    abi: escrowAbi,
    functionName: "settled",
    args: [messageId],
  });
}

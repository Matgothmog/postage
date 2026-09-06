import { type Hex, isAddress } from "viem";
import { publicClient } from "@/lib/client";
import { HUMAN_REGISTRY, POSTAGE_ESCROW, escrowAbi, registryAbi } from "@/lib/contracts";
import { challengeByToken, grantPass, linkSenderWallet, resolveChallenge } from "@/lib/db";

/// A credential lasts 90 days onchain, so simply holding one proves a person
/// verified at some point — not that anyone is here now. Requiring the
/// attestation to be nearly its full length makes it evidence of the last few
/// minutes, which is what a gate in front of an inbox actually needs.
const CREDENTIAL_LIFETIME_SECONDS = 90 * 24 * 60 * 60;
const PROOF_FRESHNESS_SECONDS = 10 * 60;

/// A sender clears the gate one of two ways, and both are checked against the
/// chain rather than taken on the browser's word: they proved personhood just
/// now, or their payment for this exact message has settled.
///
/// Clearing it grants a pass that runs out. Personhood opens a short window,
/// because a proof describes a moment rather than an address. Paying buys one
/// delivery, because that is what was paid for.
export async function POST(request: Request) {
  const { token, wallet } = (await request.json()) as { token?: string; wallet?: string };
  if (!token || !wallet || !isAddress(wallet)) {
    return Response.json({ error: "token and a valid wallet are required" }, { status: 400 });
  }

  const challenge = await challengeByToken(token);
  if (!challenge) return Response.json({ error: "Unknown challenge" }, { status: 404 });

  const dangerous = challenge.tier === "dangerous";

  // Deception is not something being a person excuses, so this route stays shut
  // however convincingly the sender verifies.
  if (!dangerous && (await verifiedJustNow(wallet))) {
    await grantPass(challenge.handle, challenge.sender, "human", null);
    await resolveChallenge(token);
    return Response.json({ status: "cleared", reason: "human" });
  }

  if (await hasPaid(challenge.message_id as Hex)) {
    await linkSenderWallet(challenge.sender, wallet);
    await resolveChallenge(token);

    if (dangerous) {
      // The money is taken and the message still does not arrive. Paying here
      // is a penalty, not a price.
      return Response.json({ status: "charged", reason: "dangerous" });
    }

    await grantPass(challenge.handle, challenge.sender, "paid", 1);
    return Response.json({ status: "cleared", reason: "paid" });
  }

  return Response.json({ status: "pending" });
}

async function verifiedJustNow(wallet: string): Promise<boolean> {
  const humanUntil = await publicClient.readContract({
    address: HUMAN_REGISTRY,
    abi: registryAbi,
    functionName: "humanUntil",
    args: [wallet as Hex],
  });

  const now = Math.floor(Date.now() / 1000);
  return Number(humanUntil) > now + CREDENTIAL_LIFETIME_SECONDS - PROOF_FRESHNESS_SECONDS;
}

async function hasPaid(messageId: Hex): Promise<boolean> {
  return publicClient.readContract({
    address: POSTAGE_ESCROW,
    abi: escrowAbi,
    functionName: "settled",
    args: [messageId],
  });
}

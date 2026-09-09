import { randomUUID } from "node:crypto";
import type { Hex } from "viem";
import type { Verdict } from "@/lib/classify";
import { publicClient } from "@/lib/client";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import { HOLD_SECONDS, createChallenge, purgeExpiredHolds } from "@/lib/db/challenges";
import { walletForSender } from "@/lib/db/sender-wallets";
import { quote } from "@/lib/pricing";
import { messageIdFor, signQuote, type SignedQuote } from "@/lib/quote";
import { gatherSignals } from "@/lib/reputation";
import { now } from "@/lib/time";

/// What a sender is asked for, and where they are sent to answer it.
export interface IssuedChallenge {
  token: string;
  price: bigint;
  reasons: string[];
  quote: SignedQuote;
  challengeUrl: string;
  /// Null when nothing is being held. Dangerous mail is never delivered by any
  /// route, so there is nothing to hold and no reason to keep what it said.
  heldUntil: number | null;
}

/// Prices one message and records the challenge that stands between it and the
/// inbox.
///
/// Nothing about the message is stored. The worker keeps the original bytes so
/// that releasing a hold puts the message that was sent on the wire rather than
/// a copy of it; this records who wrote to whom and the price.
export async function issueChallenge(mail: {
  handle: string;
  sender: string;
  subject: string;
  verdict: Verdict;
  wallet: Hex;
  appUrl: string;
}): Promise<IssuedChallenge> {
  const { handle, sender, subject, verdict, wallet, appUrl } = mail;

  // A sender who has paid before is priced on that history rather than as a
  // stranger, which is the whole point of indexing payments.
  const senderWallet = await walletForSender(sender);
  const signals = senderWallet ? await gatherSignals(senderWallet) : null;

  // The floor comes from the chain, never from our own database. The escrow
  // reverts on anything below it, so a cached copy that drifts out of date
  // produces quotes nobody can pay. `effectiveFloor` rather than `floorPrice`,
  // so an inbox whose owner never picked a price is still charged for.
  const floor = await publicClient.readContract({
    address: POSTAGE_ESCROW,
    abi: escrowAbi,
    functionName: "effectiveFloor",
    args: [wallet],
  });
  const priced = quote(floor, verdict.tier, signals, verdict.degraded);

  const token = randomUUID().replaceAll("-", "");
  const receivedAt = now();
  const messageId = messageIdFor(sender, handle, subject, receivedAt);
  const signed = await signQuote(messageId, wallet, verdict.tier, priced.amount);

  const heldUntil = verdict.tier === "dangerous" ? null : receivedAt + HOLD_SECONDS;
  await purgeExpiredHolds();

  await createChallenge({
    token,
    handle,
    sender,
    message_id: messageId,
    tier: verdict.tier,
    amount: priced.amount.toString(),
    quote_json: JSON.stringify({ ...signed, reasons: priced.reasons }),
    held_until: heldUntil,
    created_at: receivedAt,
  });

  return {
    token,
    price: priced.amount,
    reasons: priced.reasons,
    quote: signed,
    challengeUrl: `${appUrl}/c/${token}`,
    heldUntil,
  };
}

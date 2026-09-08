import { type Hex } from "viem";
import { publicClient } from "@/lib/client";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import { challengeByToken, grantPass, linkSenderWallet, resolveChallenge } from "@/lib/db";
import { releaseHeldMessage } from "@/lib/hold";

const ZERO_ADDRESS = "0x0000000000000000000000000000000000000000";

/// Settles the paying half of a challenge. Personhood is not handled here:
/// proving it needs no wallet and lives in `/api/world/verify`, which checks a
/// proof rather than reading a credential off an address the caller named.
///
/// Paying buys one delivery, because that is what was paid for.
export async function POST(request: Request) {
  const { token } = (await request.json()) as { token?: string };
  if (!token) return Response.json({ error: "A challenge token is required" }, { status: 400 });

  const challenge = await challengeByToken(token);
  if (!challenge) return Response.json({ error: "Unknown challenge" }, { status: 404 });

  // A settled message stays settled onchain forever, so without this one
  // payment could be redeemed for a fresh pass as often as it was asked for.
  if (challenge.resolved_at) {
    return Response.json(
      { status: "spent", error: "This challenge has already been settled" },
      { status: 409 }
    );
  }

  const payer = await payerOf(challenge.message_id as Hex);
  if (!payer) return Response.json({ status: "pending" });

  // Taken from the escrow rather than the request, because whoever calls this
  // could otherwise name any wallet and inherit its reputation. The contract
  // recorded who actually paid.
  await linkSenderWallet(challenge.sender, payer);

  if (challenge.tier === "dangerous") {
    // The money is taken and the message still does not arrive. Paying here is
    // a penalty, not a price.
    await resolveChallenge(token);
    return Response.json({ status: "charged", reason: "dangerous" });
  }

  await grantPass(challenge.handle, challenge.sender, "paid", 1);

  // Marked settled only once the pass exists. Stamping it first would mean a
  // failure anywhere below left the challenge closed with nothing granted: the
  // payment is already onchain and cannot be made again, so every retry would
  // answer "already settled" and the sender would have paid for silence.
  await resolveChallenge(token);

  return Response.json({
    status: "cleared",
    reason: "paid",
    ...(await releaseHeldMessage(token, challenge.handle)),
  });
}

/// The address the escrow recorded as having paid, or null if nobody has.
///
/// A chain that cannot be read right now is a payment we cannot see yet, which
/// is what "pending" means. Throwing here would tell a sender who had just paid
/// that something was wrong with them.
async function payerOf(messageId: Hex): Promise<string | null> {
  try {
    const [, , payer] = await publicClient.readContract({
      address: POSTAGE_ESCROW,
      abi: escrowAbi,
      functionName: "settlementOf",
      args: [messageId],
    });
    return payer && payer !== ZERO_ADDRESS ? payer : null;
  } catch (cause) {
    // A wrong address or a stale ABI reads exactly like nobody having paid, and
    // would leave every sender staring at "not cleared yet" with nothing said
    // anywhere. Unreachable is a fine reason to wait; misconfigured is not.
    console.error("Could not read the escrow settlement", cause);
    return null;
  }
}

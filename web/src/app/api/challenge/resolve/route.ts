import { type Hex } from "viem";
import { publicClient } from "@/lib/client";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import {
  challengeByToken,
  claimChallenge,
  grantPass,
  hasLivePass,
  linkSenderWallet,
  releaseChallengeClaim,
} from "@/lib/db";
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
  // Answering a settled challenge with its own outcome rather than a bare
  // refusal. The sender may simply have lost the first reply, and telling them
  // nothing happened would be worse than telling them what did.
  if (challenge.resolved_at) return Response.json(await settledOutcome(challenge));

  const payer = await payerOf(challenge.message_id as Hex);
  if (!payer) return Response.json({ status: "pending" });

  // One statement, so two tabs cannot both believe they are the one settling
  // this. Whoever loses is told the outcome rather than an error.
  if (!(await claimChallenge(token))) return Response.json(await settledOutcome(challenge));

  try {
    // Taken from the escrow rather than the request, because whoever calls this
    // could otherwise name any wallet and inherit its reputation. The contract
    // recorded who actually paid.
    await linkSenderWallet(challenge.sender, payer);

    if (challenge.tier === "dangerous") {
      // The money is taken and the message still does not arrive. Paying here
      // is a penalty, not a price.
      return Response.json({ status: "charged", reason: "dangerous" });
    }

    // Released first, and a pass granted only if it did not go. One payment
    // buys one delivery: granting a use as well as delivering the held message
    // would hand the sender a second, free message through the paste box.
    const released = await releaseHeldMessage(token, challenge.handle);
    if (!released.delivered) {
      await grantPass(challenge.handle, challenge.sender, "paid", 1);
    }

    return Response.json({ status: "cleared", reason: "paid", ...released });
  } catch (cause) {
    // The payment is onchain and cannot be made a second time, so a challenge
    // claimed for work that then failed has to be openable again. Otherwise the
    // sender has bought silence.
    await releaseChallengeClaim(token);
    throw cause;
  }
}

/// A read that failed because of what we asked, rather than whether we could
/// reach anyone to ask it.
function looksLikeMisconfiguration(cause: unknown): boolean {
  const name = cause instanceof Error ? cause.name : "";
  return /Abi|ContractFunction|Decode/i.test(name);
}

/// What a challenge that is already settled should say. Dangerous mail was
/// charged and not delivered. Anything else was cleared, and whether the held
/// message went is read off the pass rather than guessed: a live one is the
/// way back in that only exists when delivery failed.
async function settledOutcome(challenge: { tier: string; handle: string; sender: string }) {
  if (challenge.tier === "dangerous") return { status: "charged", reason: "dangerous" };
  const pending = await hasLivePass(challenge.handle, challenge.sender);
  return { status: "cleared", reason: "paid", delivered: !pending };
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
    // Unreachable is a fine reason to wait. Misconfigured is not: a wrong
    // address or an ABI that drifted from the deployed contract would otherwise
    // read exactly like nobody having paid, and every sender whose money had
    // already left their wallet would be told to keep waiting.
    if (looksLikeMisconfiguration(cause)) throw cause;
    console.error("Could not read the escrow settlement", cause);
    return null;
  }
}

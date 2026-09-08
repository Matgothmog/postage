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
  if (challenge.resolved_at) return Response.json(settledOutcome(challenge));

  const payer = await payerOf(challenge.message_id as Hex);
  if (!payer) return Response.json({ status: "pending" });

  // One statement, so two tabs cannot both believe they are the one settling
  // this. Whoever loses is told the outcome rather than an error.
  if (!(await claimChallenge(token, "paid"))) {
    // Re-read: this row was loaded before the claim, so whatever the winner
    // recorded while we were asking is not in it yet.
    return Response.json(settledOutcome((await challengeByToken(token)) ?? challenge));
  }

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
      // Never downgrades. grantPass overwrites the row, so a sender who proved
      // personhood minutes ago would trade an unlimited window for one use by
      // paying for a second message.
      if (!(await hasLivePass(challenge.handle, challenge.sender))) {
        await grantPass(challenge.handle, challenge.sender, "paid", 1);
      }
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

/// Whether a failed read was the network rather than the request.
///
/// viem wraps every `readContract` failure in a ContractFunctionExecutionError,
/// so the outermost name tells us nothing at all — an unreachable node and a
/// stale ABI arrive under the same one. What separates them is further down the
/// cause chain.
function looksTransient(cause: unknown): boolean {
  for (let error = cause, depth = 0; error instanceof Error && depth < 8; depth += 1) {
    if (/HttpRequest|Timeout|SocketClosed|Connection|Fetch|RpcError|LimitExceeded/i.test(error.name)) {
      return true;
    }
    error = error.cause;
  }
  return false;
}

/// What a challenge that is already settled should say. Dangerous mail was
/// charged and not delivered; anything else was cleared, and whether the held
/// message actually went is read off the challenge rather than inferred. Pass
/// state cannot answer it: a human pass earned on an earlier message looks
/// exactly like one granted because delivery failed, and reading it during
/// another request's work answers about a moment that has already passed.
function settledOutcome(challenge: {
  tier: string;
  delivered_at: number | null;
  settled_by: string | null;
}) {
  if (challenge.tier === "dangerous") return { status: "charged", reason: "dangerous" };
  return {
    status: "cleared",
    // Which lane opened it, not an assumption. A token cleared for free by
    // proving personhood would otherwise be reported back as "Paid".
    reason: challenge.settled_by ?? "paid",
    delivered: challenge.delivered_at !== null,
  };
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
    if (!looksTransient(cause)) throw cause;
    console.error("Could not read the escrow settlement", cause);
    return null;
  }
}

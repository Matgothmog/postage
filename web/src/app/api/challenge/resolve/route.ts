import { type Hex } from "viem";
import { publicClient } from "@/lib/client";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import { challengeByToken } from "@/lib/db/challenges";
import { linkSenderWallet } from "@/lib/db/sender-wallets";
import { openGate } from "@/lib/gate";

const ZERO_ADDRESS = "0x0000000000000000000000000000000000000000";

/// Settles the paying half of a challenge: prove the payment landed, then hand
/// off. What clearing actually does lives in `lib/gate`, so that both lanes
/// answer to one description of it rather than two that drift.
///
/// Personhood is not handled here. Proving it needs no wallet and lives in
/// `/api/world/verify`, which checks a proof rather than reading a credential
/// off an address the caller named.
export async function POST(request: Request) {
  const { token } = (await request.json()) as { token?: string };
  if (!token) return Response.json({ error: "A challenge token is required" }, { status: 400 });

  const challenge = await challengeByToken(token);
  if (!challenge) return Response.json({ error: "Unknown challenge" }, { status: 404 });

  const payer = await payerOf(challenge.message_id as Hex);

  // Recorded even when the challenge turns out to have been settled some other
  // way. The money left their wallet either way, and forgetting it prices their
  // next message as a stranger's.
  if (payer) await linkSenderWallet(challenge.sender, payer);

  // No payment, no paid lane — settled or not. Letting an already-settled
  // challenge through here would hand a delivery to anyone who knew a token
  // that had been answered, without paying for anything.
  if (!payer) return Response.json({ status: "pending" });

  const result = await openGate(token, "paid");
  if (result.status === "unknown") {
    return Response.json({ error: "Unknown challenge" }, { status: 404 });
  }
  return Response.json(result);
}

/// Whether a failed read was the network rather than the request.
///
/// viem wraps every `readContract` failure in a ContractFunctionExecutionError,
/// so the outermost name tells us nothing at all — an unreachable node and a
/// stale ABI arrive under the same one. What separates them is further down the
/// cause chain.
const TRANSIENT = new Set([
  "HttpRequestError",
  "TimeoutError",
  "SocketClosedError",
  "RpcRequestError",
  "LimitExceededRpcError",
  "InternalRpcError",
  "ResourceUnavailableRpcError",
  "ResourceNotFoundRpcError",
]);

function looksTransient(cause: unknown): boolean {
  for (let error = cause, depth = 0; error instanceof Error && depth < 8; depth += 1) {
    // Named rather than pattern-matched on a suffix: "RpcError" also covers
    // InvalidParams, MethodNotFound and UnknownRpcError, which are exactly the
    // misconfiguration this is supposed to raise instead of wait on.
    if (TRANSIENT.has(error.name)) return true;
    error = error.cause;
  }
  return false;
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

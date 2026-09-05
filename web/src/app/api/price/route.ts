import { isAddress } from "viem";
import { publicClient } from "@/lib/client";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import { messageByToken, walletForInbox } from "@/lib/db";
import { quote } from "@/lib/pricing";
import { gatherSignals } from "@/lib/reputation";

/// Quotes what a specific sender pays to reach a specific inbox. The inbox
/// price is the floor set onchain; reputation from The Graph decides the
/// multiple of it this particular sender is asked for.
export async function GET(request: Request) {
  const params = new URL(request.url).searchParams;
  const token = params.get("token");
  const wallet = params.get("wallet");

  if (!token || !wallet || !isAddress(wallet)) {
    return Response.json({ error: "token and a valid wallet are required" }, { status: 400 });
  }

  const message = await messageByToken(token);
  if (!message) return Response.json({ error: "Unknown message" }, { status: 404 });

  const recipient = await walletForInbox(message.recipient_local);
  if (!recipient) return Response.json({ error: "Unknown inbox" }, { status: 404 });

  const basePrice = await publicClient.readContract({
    address: POSTAGE_ESCROW,
    abi: escrowAbi,
    functionName: "price",
    args: [recipient as `0x${string}`],
  });

  const signals = await gatherSignals(wallet);
  const result = quote(basePrice, signals);

  return Response.json({
    price: result.price.toString(),
    basePrice: result.basePrice.toString(),
    multiplierBps: result.multiplierBps,
    free: result.free,
    reasons: result.reasons,
    signals,
  });
}

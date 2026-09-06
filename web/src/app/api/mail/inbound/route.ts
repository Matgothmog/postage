import { randomUUID } from "node:crypto";
import type { Hex } from "viem";
import { publicClient } from "@/lib/client";
import { classify, extractUrls, type MailFacts } from "@/lib/classify";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import {
  allowlist,
  createChallenge,
  inboxByHandle,
  isAllowlisted,
  walletForSender,
} from "@/lib/db";
import { required } from "@/lib/env";
import { quote } from "@/lib/pricing";
import { messageIdFor, signQuote } from "@/lib/quote";
import { gatherSignals } from "@/lib/reputation";

interface InboundPayload {
  from: string;
  to: string;
  subject: string;
  body: string;
  spf?: string;
  dkim?: string;
  dmarc?: string;
}

/// Whether the receiving MTA could confirm the envelope sender is who it says.
/// The allowlist is keyed on that address, so letting an unauthenticated
/// message skip the gate would let anyone through by writing someone else's
/// name on the envelope.
function senderIsAuthenticated(payload: Partial<InboundPayload>): boolean {
  if (payload.dmarc === "pass") return true;
  return payload.spf === "pass" && payload.dkim !== "fail";
}

/// Called by the mail worker for every inbound message. Decides whether it is
/// forwarded, and if not, what it would cost to change that.
///
/// Nothing about the message is stored. The challenge row records who wrote to
/// whom and the price, never the subject or the body.
export async function POST(request: Request) {
  if (request.headers.get("x-postage-secret") !== required("MAIL_WEBHOOK_SECRET")) {
    return Response.json({ error: "Bad secret" }, { status: 401 });
  }

  const appUrl = required("APP_URL");
  const payload = (await request.json()) as Partial<InboundPayload>;
  const { from, to } = payload;
  if (!from || !to) return Response.json({ error: "from and to are required" }, { status: 400 });

  const handle = to.split("@")[0]?.toLowerCase() ?? "";
  const inbox = await inboxByHandle(handle);
  if (!inbox) return Response.json({ action: "reject", reason: "unknown_inbox" }, { status: 404 });

  if (!inbox.wallet) {
    return Response.json({ error: "Inbox has no wallet to be paid at" }, { status: 409 });
  }

  const sender = from.toLowerCase();

  // Someone who already cleared the gate never sees it again, as long as the
  // envelope they cleared it with is the one they are writing from now.
  if (senderIsAuthenticated(payload) && (await isAllowlisted(handle, sender))) {
    return Response.json({ action: "forward", to: inbox.destination, reason: "known_sender" });
  }

  const facts: MailFacts = {
    from: sender,
    to,
    subject: payload.subject ?? "",
    body: payload.body ?? "",
    spf: payload.spf ?? null,
    dkim: payload.dkim ?? null,
    dmarc: payload.dmarc ?? null,
    urls: extractUrls(payload.body ?? ""),
  };

  const verdict = await classify(facts);

  if (verdict.tier === "human" || verdict.tier === "important") {
    // A person who wrote once will write again; an OTP sender likewise. Both
    // skip the gate from here on.
    await allowlist(handle, sender, verdict.tier);
    return Response.json({
      action: "forward",
      to: inbox.destination,
      reason: verdict.tier,
      verdict,
    });
  }

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
    args: [inbox.wallet as Hex],
  });
  const priced = quote(floor, verdict.tier, signals, verdict.degraded);

  const token = randomUUID().replaceAll("-", "");
  const receivedAt = Math.floor(Date.now() / 1000);
  const messageId = messageIdFor(sender, handle, facts.subject, receivedAt);
  const signed = await signQuote(messageId, inbox.wallet as Hex, verdict.tier, priced.amount);

  await createChallenge({
    token,
    handle,
    sender,
    message_id: messageId,
    tier: verdict.tier,
    amount: priced.amount.toString(),
    quote_json: JSON.stringify({ ...signed, reasons: priced.reasons }),
    created_at: receivedAt,
  });

  return Response.json({
    // Dangerous mail is refused whether or not anyone pays. The link is still
    // offered so a misclassified sender has a route back.
    action: "reject",
    reason: verdict.tier,
    verdict,
    price: priced.amount.toString(),
    reasons: priced.reasons,
    challenge_url: `${appUrl}/c/${token}`,
    quote: signed,
  });
}

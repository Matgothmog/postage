import { randomUUID } from "node:crypto";
import type { Hex } from "viem";
import { challengeMail } from "@/lib/challenge-email";
import { classify, classifyFromHeaders, extractUrls, type MailFacts } from "@/lib/classify";
import { publicClient } from "@/lib/client";
import { POSTAGE_ESCROW, escrowAbi } from "@/lib/contracts";
import {
  HOLD_SECONDS,
  claimClassification,
  createChallenge,
  inboxByHandle,
  purgeExpiredHolds,
  purgeOldClassifications,
  spendPass,
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
///
/// Two things hang on this. The allowlist is keyed on that address, so letting
/// an unauthenticated message skip the gate would let anyone through by writing
/// someone else's name on the envelope. And it decides whether we write back at
/// all: answering a forged sender means mailing whoever was impersonated, which
/// is backscatter, so an unauthenticated sender is told inside the SMTP session
/// instead and nothing leaves the building.
function senderIsAuthenticated(payload: Partial<InboundPayload>): boolean {
  if (payload.dmarc === "pass") return true;
  return payload.spf === "pass" && payload.dkim !== "fail";
}

/// Called by the mail worker for every inbound message.
///
/// Every stranger is held. The classifier does not decide whether to hold, it
/// decides who pays to get through:
///
///   important   delivered at once, free. A login code nobody can pay for is a
///               login code that never arrives, so this tier is never held.
///   human       held. Proving personhood clears it for nothing.
///   commercial  held. Assumed to be a machine, so it pays.
///   dangerous   never delivered, whatever anyone does. Charged as a penalty
///               if a wallet is attached, and proving personhood does not
///               clear it - a real person can still be phishing.
///
/// Nothing about the message is stored here. The worker keeps the original
/// bytes so that releasing a hold puts the message that was sent on the wire
/// rather than a copy of it; this side records who wrote to whom and the price.
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
  const authenticated = senderIsAuthenticated(payload);

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

  // Read before any pass is honoured. A pass says this sender got through the
  // gate a few minutes ago; it says nothing about what they have written since,
  // and a person who proved they were a person can still be phishing. Skipping
  // the classifier here would have made a single proof a fifteen minute licence
  // to deliver anything at all.
  // Past the hourly budget the model is not asked at all and the message is
  // held as ordinary automated mail. Held rather than delivered, because
  // failing open here would make a flood the way through the gate rather than
  // merely the way to run up a bill.
  await purgeOldClassifications();
  const overBudget = !(await claimClassification(handle, sender));
  const verdict = overBudget ? classifyFromHeaders(facts) : await classify(facts);

  // Checked before any pass is spent. This tier is free and grants nothing, so
  // taking a paid use for it would charge someone twice for one delivery.
  //
  // Held to a higher bar when the verdict is degraded. The header fallback
  // calls anything transactional-sounding important as long as authentication
  // did not outright fail, and "no authentication at all" clears that — so a
  // stranger writing "your verification code" would be delivered free during an
  // outage. Requiring the sender to actually be who they say closes that
  // without holding real login codes, which is the one thing this tier exists
  // to prevent.
  if (verdict.tier === "important" && (!verdict.degraded || authenticated)) {
    return Response.json({
      action: "forward",
      to: inbox.destination,
      reason: verdict.tier,
      verdict,
    });
  }

  // A live pass, and the envelope it was earned with. Passes run out, so this
  // is a sender who cleared the gate minutes ago rather than ever.
  //
  // Budget exhaustion narrows it; a classifier outage does not. The difference
  // is who caused it, and what the pass can be used for. An unlimited window
  // plus a spent budget is a licence to deliver anything unread, so that one
  // shuts. A single paid use cannot flood by construction — it is one message,
  // already paid for — and refusing it would take the money and issue a fresh
  // demand for the same message.
  if (authenticated && verdict.tier !== "dangerous") {
    const pass = await spendPass(handle, sender, { countedOnly: overBudget });
    if (pass) {
      return Response.json({
        action: "forward",
        to: inbox.destination,
        reason: pass.reason,
        verdict,
      });
    }
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
  const challengeUrl = `${appUrl}/c/${token}`;

  // Dangerous mail is never delivered by any route, so there is nothing to hold
  // and no reason to keep what it said.
  const holding = verdict.tier !== "dangerous";
  const heldUntil = receivedAt + HOLD_SECONDS;
  await purgeExpiredHolds();

  await createChallenge({
    token,
    handle,
    sender,
    message_id: messageId,
    tier: verdict.tier,
    amount: priced.amount.toString(),
    quote_json: JSON.stringify({ ...signed, reasons: priced.reasons }),
    held_until: holding ? heldUntil : null,
    created_at: receivedAt,
  });

  const common = {
    reason: verdict.tier,
    verdict,
    price: priced.amount.toString(),
    reasons: priced.reasons,
    challenge_url: challengeUrl,
    quote: signed,
  };

  if (!holding) {
    return Response.json({
      ...common,
      action: "reject",
      bounce: `Not delivered: this looks like an attempt to deceive the recipient, and paying will not change that. If it is a mistake, say so at ${challengeUrl}`,
    });
  }

  return Response.json({
    ...common,
    action: "hold",
    token,
    held_until: heldUntil,
    // Present only when we can write back without mailing a stranger whose name
    // was borrowed. Without it the worker refuses the message instead, and the
    // link travels in the bounce the sender's own server writes them.
    notice: authenticated
      ? challengeMail({
          handle,
          subject: facts.subject,
          tier: verdict.tier,
          amount: priced.amount,
          reasons: priced.reasons,
          challengeUrl,
          appUrl,
          heldUntil,
        })
      : null,
    bounce: `Held, not lost: say whether a person or a machine wrote this and we deliver the message you already sent - ${challengeUrl}`,
  });
}

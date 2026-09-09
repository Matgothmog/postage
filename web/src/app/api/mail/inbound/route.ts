import type { Hex } from "viem";
import { challengeMail } from "@/lib/challenge-email";
import { classify, classifyFromHeaders, extractUrls, type MailFacts } from "@/lib/classify";
import {
  type BudgetState,
  claimClassification,
  purgeOldClassifications,
} from "@/lib/db/classifications";
import { inboxByHandle } from "@/lib/db/inboxes";
import { required } from "@/lib/env";
import { handleOf, isOurs } from "@/lib/handle";
// `web/` and `worker/` are separate packages with no workspace tooling between
// them, so the wire shape this route answers with is declared once outside
// both and reached by relative path rather than duplicated.
import type { GatewayVerdict } from "../../../../../../shared/gateway-verdict";
import { issueChallenge } from "./challenge";
import { forwardWithoutChallenge } from "./forwarding";

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

/// The message as the classifier is given it: the envelope, whatever the
/// receiving server made of the authentication, and the links in the body.
function mailFacts(payload: Partial<InboundPayload>, from: string, to: string): MailFacts {
  return {
    from,
    to,
    subject: payload.subject ?? "",
    body: payload.body ?? "",
    spf: payload.spf ?? null,
    dkim: payload.dkim ?? null,
    dmarc: payload.dmarc ?? null,
    urls: extractUrls(payload.body ?? ""),
  };
}

/// A message that will never be delivered, whatever anyone does about it. The
/// worker puts `bounce` in the SMTP refusal, so the sender is told why rather
/// than left to retry.
function refuse(reason: string, bounce: string): Response {
  const wire: GatewayVerdict = { action: "reject", reason, bounce };
  return Response.json(wire);
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
export async function POST(request: Request) {
  if (request.headers.get("x-postage-secret") !== required("MAIL_WEBHOOK_SECRET")) {
    return Response.json({ error: "Bad secret" }, { status: 401 });
  }

  const appUrl = required("APP_URL");
  const payload = (await request.json()) as Partial<InboundPayload>;
  const { from, to } = payload;
  if (!from || !to) return Response.json({ error: "from and to are required" }, { status: 400 });

  const handle = handleOf(to);
  const inbox = await inboxByHandle(handle);
  if (!inbox) {
    const wire: GatewayVerdict = { action: "reject", reason: "unknown_inbox" };
    return Response.json(wire, { status: 404 });
  }

  // Both of these are answered 200 with a verdict in the body, like every other
  // outcome. A non-2xx tells the worker it could not reach us, and it then
  // refuses the session with "please retry" — which for a permanent condition
  // means the sender's server retries for days over something retrying cannot
  // fix. Only a gateway fault is a status code.
  if (!inbox.wallet) {
    return refuse("no_wallet", "That address cannot receive mail yet: nobody has claimed it fully.");
  }

  // Forwarding to our own domain sends the message straight back in, and every
  // lap spends a classify call, a chain read and a challenge row. Claiming such
  // a destination is refused, so reaching here needs an inbox that predates that
  // check or a row edited by hand — but nothing else stops it, and a loop nobody
  // notices is a bill nobody agreed to.
  if (isOurs(inbox.destination)) {
    return refuse("loop", "That address forwards back to this domain, so nothing can be delivered.");
  }

  const sender = from.toLowerCase();
  const authenticated = senderIsAuthenticated(payload);

  const facts = mailFacts(payload, sender, to);

  // Dropped on the way past rather than by a job nobody runs, so the table the
  // budget counts over stays the size of one hour.
  await purgeOldClassifications();

  // Read before any pass is honoured. A pass says this sender got through the
  // gate a few minutes ago; it says nothing about what they have written since,
  // and a person who proved they were a person can still be phishing. Skipping
  // the classifier here would have made a single proof a fifteen minute licence
  // to deliver anything at all. Past the hourly budget the model is not asked at
  // all and the message is held as ordinary automated mail — held rather than
  // delivered, because failing open would make a flood the way through the gate
  // rather than merely the way to run up a bill.
  //
  // Only mail whose sender the receiving server could confirm is worth paying a
  // model to read, and only it may spend the budget.
  //
  // Anyone can put any address on an envelope, so counting unauthenticated mail
  // let a stranger drain a recipient's hourly pool with forged senders — and a
  // drained pool is what puts the classifier into its header-only fallback,
  // where a signed domain and a transactional-sounding subject are delivered
  // free. That made the outage something an attacker could manufacture and then
  // walk through. Unauthenticated mail is now judged from its headers and held,
  // which costs nothing and unlocks nothing.
  const budgetRefusal: BudgetState = authenticated
    ? await claimClassification(handle, sender)
    : "spent-by-sender";
  const verdict = budgetRefusal ? classifyFromHeaders(facts) : await classify(facts);

  const forwarded = await forwardWithoutChallenge({
    handle,
    sender,
    verdict,
    authenticated,
    budgetRefusal,
  });
  if (forwarded) {
    const wire: GatewayVerdict = { action: "forward", to: inbox.destination, reason: forwarded.reason };
    // `verdict` rides along for this route's own tests and for logs; the wire
    // contract the worker actually depends on is only the fields on `wire`.
    return Response.json({ ...wire, verdict });
  }

  const challenge = await issueChallenge({
    handle,
    sender,
    subject: facts.subject,
    verdict,
    wallet: inbox.wallet as Hex,
    appUrl,
  });

  // Extra context for this route's own tests and for logs; not part of the
  // wire contract the worker relies on (`GatewayVerdict`, built below).
  const context = {
    verdict,
    price: challenge.price.toString(),
    reasons: challenge.reasons,
    challenge_url: challenge.challengeUrl,
    quote: challenge.quote,
  };

  const { heldUntil } = challenge;
  if (heldUntil === null) {
    const wire: GatewayVerdict = {
      action: "reject",
      reason: verdict.tier,
      bounce: `Not delivered: this looks like an attempt to deceive the recipient, and paying will not change that. If it is a mistake, say so at ${challenge.challengeUrl}`,
    };
    return Response.json({ ...wire, ...context });
  }

  const wire: GatewayVerdict = {
    action: "hold",
    reason: verdict.tier,
    token: challenge.token,
    held_until: heldUntil,
    // Present only when we can write back without mailing a stranger whose name
    // was borrowed. Without it the worker refuses the message instead, and the
    // link travels in the bounce the sender's own server writes them.
    notice: authenticated
      ? challengeMail({
          handle,
          subject: facts.subject,
          amount: challenge.price,
          reasons: challenge.reasons,
          challengeUrl: challenge.challengeUrl,
          appUrl,
          heldUntil,
        })
      : null,
    bounce: `Held, not lost: say whether a person or a machine wrote this and we deliver the message you already sent - ${challenge.challengeUrl}`,
  };
  return Response.json({ ...wire, ...context });
}

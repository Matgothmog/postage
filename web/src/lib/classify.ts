import Anthropic from "@anthropic-ai/sdk";
import { zodOutputFormat } from "@anthropic-ai/sdk/helpers/zod";
import { z } from "zod";

export const TIERS = ["human", "important", "commercial", "dangerous"] as const;
export type Tier = (typeof TIERS)[number];

/// Tier index as the escrow enum orders them. Kept adjacent to TIERS so the two
/// cannot drift apart silently.
export const TIER_INDEX: Record<Tier, number> = {
  human: 0,
  important: 1,
  commercial: 2,
  dangerous: 3,
};

export interface MailFacts {
  from: string;
  to: string;
  subject: string;
  body: string;
  /// Results the receiving MTA already computed. "pass", "fail", "none".
  spf: string | null;
  dkim: string | null;
  dmarc: string | null;
  urls: string[];
}

export interface Verdict {
  tier: Tier;
  confidence: number;
  reasons: string[];
  /// True when the model was unreachable and this came from headers alone. A
  /// degraded verdict is never allowed to charge at the dangerous tier.
  degraded: boolean;
}

const VerdictSchema = z.object({
  tier: z.enum(TIERS),
  confidence: z.number(),
  reasons: z.array(z.string()),
});

const SYSTEM = `You classify inbound email for a gateway that charges senders.

Decide which one of four things a message is:

- human: written by a person to this specific recipient. Personal or
  professional correspondence, a reply, an introduction.
- important: automated but the recipient needs it now. One-time codes, password
  resets, receipts, shipping and booking confirmations, security alerts.
- commercial: automated and legitimate but not urgent. Newsletters, marketing,
  product announcements, digests, notifications the recipient opted into.
- dangerous: trying to deceive. Credential phishing, impersonation of a brand or
  person, payment redirection, malware links, extortion.

Weigh the authentication results heavily. A message claiming to be from a bank
with dmarc=fail is far more suspect than the same text with dmarc=pass. Judge
the links by where they actually point, not by their anchor text.

Err toward "important" over "commercial" when a person would be harmed by a
delay, and toward "commercial" over "dangerous" when a message is merely
unwanted. Blocking real mail costs the user more than letting a newsletter
through.

Give two or three short reasons, each a plain sentence a recipient would
understand.`;

function userContent(mail: MailFacts): string {
  return [
    `From: ${mail.from}`,
    `To: ${mail.to}`,
    `Subject: ${mail.subject}`,
    `SPF: ${mail.spf ?? "unknown"}  DKIM: ${mail.dkim ?? "unknown"}  DMARC: ${mail.dmarc ?? "unknown"}`,
    mail.urls.length ? `Links: ${mail.urls.slice(0, 20).join(", ")}` : "Links: none",
    "",
    "Body:",
    mail.body.slice(0, 12_000),
  ].join("\n");
}

export async function classify(mail: MailFacts): Promise<Verdict> {
  try {
    return await classifyWithModel(mail);
  } catch {
    // An email gateway that stops delivering when its classifier is down is
    // worse than one that falls back to what the headers already told it.
    return classifyFromHeaders(mail);
  }
}

async function classifyWithModel(mail: MailFacts): Promise<Verdict> {
  const client = new Anthropic();

  const response = await client.messages.parse({
    model: "claude-opus-5",
    max_tokens: 2048,
    system: SYSTEM,
    messages: [{ role: "user", content: userContent(mail) }],
    output_config: { format: zodOutputFormat(VerdictSchema), effort: "low" },
  });

  const parsed = response.parsed_output;
  if (!parsed) throw new Error("Classifier returned no parsed output");

  return { ...parsed, degraded: false };
}

const TRANSACTIONAL =
  /\b(verification code|one[- ]time|otp|password reset|reset your|confirm your|receipt|invoice|order (confirmed|shipped)|security alert|sign[- ]in attempt)\b/i;
const MARKETING = /\b(unsubscribe|newsletter|deal|% off|sale|webinar|new features?)\b/i;
const LURE = /\b(verify your account|suspended|unusual activity|click here|confirm your identity|wire transfer|gift card)\b/i;

/// Header-only fallback. Deliberately conservative: it can flag something as
/// suspicious but never as dangerous, because a wrong dangerous verdict blocks
/// mail and charges for it.
export function classifyFromHeaders(mail: MailFacts): Verdict {
  const reasons: string[] = [];
  const authFailed = mail.dmarc === "fail" || (mail.spf === "fail" && mail.dkim !== "pass");

  if (authFailed) reasons.push("Sender authentication failed for this domain");

  if (TRANSACTIONAL.test(mail.subject) && !authFailed) {
    reasons.push("Reads as a transactional message the recipient is waiting for");
    return { tier: "important", confidence: 0.5, reasons, degraded: true };
  }

  if (authFailed && LURE.test(`${mail.subject} ${mail.body}`)) {
    reasons.push("Uses pressure language typical of credential phishing");
    // Still not "dangerous" - a degraded verdict must not charge the top tier.
    return { tier: "commercial", confidence: 0.3, reasons, degraded: true };
  }

  if (MARKETING.test(`${mail.subject} ${mail.body}`)) {
    reasons.push("Looks like bulk or marketing mail");
  } else {
    reasons.push("Classified from headers alone while the model was unavailable");
  }

  return { tier: "commercial", confidence: 0.3, reasons, degraded: true };
}

export function extractUrls(body: string): string[] {
  const found = body.match(/https?:\/\/[^\s<>"')]+/gi) ?? [];
  return [...new Set(found)];
}

import Anthropic from "@anthropic-ai/sdk";
import { zodOutputFormat } from "@anthropic-ai/sdk/helpers/zod";
import { z } from "zod";
import { TIERS, type Tier } from "./tiers";

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

Give two or three reasons. Each is at most eight words, a fragment, no closing
period, concrete about what you actually saw — "sent from a bulk mail
platform", "link text and destination disagree", "DMARC failed for a bank
domain". No hedging, no jargon, and never restate the tier name.`;

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
  } catch (cause) {
    // An email gateway that stops delivering when its classifier is down is
    // worse than one that falls back to what the headers already told it —
    // but that fallback used to be silent, and a missing ANTHROPIC_API_KEY
    // once degraded every classification for hours before anyone noticed,
    // with nothing in the logs to say why. Never the mail itself: `mail`
    // carries a private message's from/subject/body and none of it belongs
    // in a log line.
    console.error("classification degraded to header-only fallback", {
      reason: sanitizedReason(cause),
    });
    return classifyFromHeaders(mail);
  }
}

/// A credential is a long run of key-charset characters with no spaces —
/// `sk-ant-...`, a bearer token, a signed URL's `key=` value. The
/// auth-resolution failure this fallback exists for never carries one (it
/// names which setting is missing, not its value), but nothing guarantees the
/// next thrown error won't quote a request that did, so anything shaped like
/// one is blanked out before the reason reaches the log.
const KEY_SHAPED = /[A-Za-z0-9_-]{20,}/g;

export function sanitizedReason(cause: unknown): string {
  const message = cause instanceof Error ? cause.message : String(cause);
  return message.replace(KEY_SHAPED, "[redacted]");
}

const REASON_WORD_CAP = 8;

/// Shapes the model's own reasons before they leave this module: trim, drop
/// one trailing period, collapse every run of whitespace — newlines included
/// — and cap at REASON_WORD_CAP words.
///
/// Nothing renders these today, whatever the two `reasons` fields being one
/// word apart suggests. The "Why" list in the held-mail notice
/// (`lib/challenge-email.ts:118` text, `:133` html) and on the gate page
/// (`app/c/[token]/page.tsx:92`) is `priced.reasons`, the fixed strings
/// `quote` writes at `lib/pricing.ts:49-92`, carried over by `issueChallenge`
/// (`api/mail/inbound/challenge.ts:104`, stored at `:95`) — which is handed
/// `verdict.tier` and `verdict.degraded` and never a reason (`:71`). A
/// `Verdict`'s own reasons leave this process only inside the inbound route's
/// JSON answer (`api/mail/inbound/route.ts:176`, `:191`), and the worker reads
/// that body as a `GatewayVerdict`, a shape carrying no `reasons` field at all
/// (`worker/src/index.ts:344`, `shared/gateway-verdict.ts:15`).
///
/// Kept anyway, at the source rather than at a renderer that does not exist
/// yet. These are model output shaped by a stranger's email, and the collapse
/// is the part that earns its place: a plain-text body is exactly its own
/// bytes, so a reason ever wired into that half could write its own lines into
/// a message sent under our name — the forgery `oneLine`
/// (`lib/challenge-email.ts:59`) already stops for the subject, which does
/// travel that path. Pure and isolated from the API call, so it unit-tests
/// without a model.
export function tidy(reasons: string[]): string[] {
  return reasons.map(tidyOne);
}

function tidyOne(reason: string): string {
  const words = reason
    .trim()
    .replace(/\.$/, "")
    .split(/\s+/)
    .filter(Boolean);
  return words.slice(0, REASON_WORD_CAP).join(" ");
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

  return { ...parsed, reasons: tidy(parsed.reasons), degraded: false };
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

  if (authFailed) reasons.push("Domain failed its own auth check");

  if (TRANSACTIONAL.test(mail.subject) && !authFailed) {
    reasons.push("Looks like a code or a receipt");
    return { tier: "important", confidence: 0.5, reasons, degraded: true };
  }

  if (authFailed && LURE.test(`${mail.subject} ${mail.body}`)) {
    reasons.push("Pressure language typical of phishing");
    // Still not "dangerous" - a degraded verdict must not charge the top tier.
    return { tier: "commercial", confidence: 0.3, reasons, degraded: true };
  }

  if (MARKETING.test(`${mail.subject} ${mail.body}`)) {
    reasons.push("Reads as bulk marketing");
  } else {
    reasons.push("Judged on headers only");
  }

  return { tier: "commercial", confidence: 0.3, reasons, degraded: true };
}

export function extractUrls(body: string): string[] {
  const found = body.match(/https?:\/\/[^\s<>"')]+/gi) ?? [];
  return [...new Set(found)];
}

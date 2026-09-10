import { accent, bg, card, ink, inkSoft, line, onAccent } from "./brand";
import { formatUsdc } from "./format";
import { postageAddress } from "./handle";
import { now } from "./time";

export interface ChallengeMail {
  subject: string;
  html: string;
  text: string;
}

export interface ChallengeMailFacts {
  handle: string;
  /// What they wrote in the subject line, quoted back so they can tell which
  /// message this is about without opening anything.
  subject: string;
  amount: bigint;
  reasons: string[];
  challengeUrl: string;
  appUrl: string;
  heldUntil: number;
}

/// Everything a body says that is not the sender's own words: derived once,
/// here, so both halves of the mail quote the same subject and name the same
/// price rather than each working it out again.
interface Rendered {
  inbox: string;
  subject: string;
  price: string;
  deadline: string;
}

/// How much of a subject is quoted back. Nothing between the sender and this
/// template bounds it - the worker hands over whatever the header decoded to,
/// and it crosses as JSON, which has no length of its own - and the plain-text
/// half is the whole message for a client that renders no markup, so a subject
/// allowed to run on is a notice whose own words nobody scrolls to. Long
/// enough for any subject written on purpose.
const SUBJECT_LIMIT = 200;

/// Everything that could end the line a subject is quoted on, or reorder what
/// is printed after it: ordinary whitespace, the C0 and C1 controls, Unicode's
/// own line and paragraph separators, and the format characters - the
/// bidirectional overrides among them - that render as nothing at all.
const UNPRINTABLE = /[\s\p{Cc}\p{Cf}\p{Zl}\p{Zp}]+/gu;

/// The sender's subject, reduced to something that fits inside a line of ours.
///
/// The HTML half escapes, so markup is already closed there. The plain-text
/// half has no escaping to fall back on - a plain-text body is exactly its own
/// bytes - so the only defence left is that the value cannot leave the line it
/// was given. That defence is needed rather than theoretical: PostalMime
/// decodes RFC 2047 encoded-words, and an encoded-word decodes to whatever its
/// bytes say, newlines included, where an ordinary folded header would have
/// been unfolded to a space. Without this a sender writes their own lines into
/// a notice sent under our name - a second "A PERSON WROTE IT - free", free
/// and pointing wherever they like, sitting above the real one.
function oneLine(subject: string): string {
  const flattened = subject.replace(UNPRINTABLE, " ").trim();
  if (flattened.length <= SUBJECT_LIMIT) return flattened;
  return `${flattened.slice(0, SUBJECT_LIMIT)}...`;
}

/// The one message a held sender receives. It asks a single question - person or
/// machine - and each answer is a link. Nothing here asks them to write their
/// message again, because we still have it.
export function challengeMail(facts: ChallengeMailFacts): ChallengeMail {
  const rendered: Rendered = {
    inbox: postageAddress(facts.handle),
    subject: oneLine(facts.subject) || "(no subject)",
    price: formatUsdc(facts.amount),
    deadline: holdWindow(facts.heldUntil),
  };

  return {
    subject: `Held: your mail to ${rendered.inbox}`,
    html: html(facts, rendered),
    text: text(facts, rendered),
  };
}

/// `heldUntil` comes off the `held_until` column, which is seconds like every
/// other timestamp column — the same domain `now()` exists to hand back
/// without a units bug, even though this function never itself writes a row.
function holdWindow(heldUntil: number): string {
  const hours = Math.max(1, Math.round((heldUntil - now()) / 3600));
  return hours === 1 ? "an hour" : `${hours} hours`;
}

const escapes: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
};

function escape(value: string): string {
  return value.replace(/[&<>"']/g, (character) => escapes[character]);
}

function text(facts: ChallengeMailFacts, { inbox, subject, price, deadline }: Rendered): string {
  return [
    `Your mail to ${inbox} is held.`,
    "",
    `Subject: ${subject}`,
    "",
    "Answer once and we send it. Nothing to rewrite.",
    "",
    "I'M HUMAN - free",
    `${facts.challengeUrl}?as=human`,
    "",
    `I'M A BOT - ${price}`,
    `${facts.challengeUrl}?as=bot`,
    "",
    "Why this was held:",
    ...facts.reasons.map((reason) => `  - ${reason}`),
    "",
    `Held for ${deadline}. Ignore this and the message is erased unread.`,
    "",
    "---",
    "Tired of paying for other people's attention? Hand out a Postage address",
    "instead of your own and the machines writing to you pay you instead.",
    facts.appUrl,
  ].join("\n");
}

/// Tables and inline styles, because this is read in mail clients rather than
/// browsers. No images: the stamp is drawn with borders, so it survives a client
/// that blocks remote content.
function html(facts: ChallengeMailFacts, { inbox, subject, price, deadline }: Rendered): string {
  const reasons = facts.reasons
    .map(
      (reason) =>
        `<tr><td style="padding:0 0 8px 0;color:${inkSoft};font-size:14px;line-height:21px;">
           <span style="color:${accent};">&#8212;</span>&nbsp;&nbsp;${escape(reason)}</td></tr>`
    )
    .join("");

  return `<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="color-scheme" content="light">
<title>${escape(inbox)}</title>
</head>
<body style="margin:0;padding:0;background:${bg};" bgcolor="${bg}">
<div style="display:none;max-height:0;overflow:hidden;opacity:0;">One click sends it.</div>

<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="background:${bg};" bgcolor="${bg}">
<tr><td align="center" style="padding:32px 16px;">

<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="max-width:560px;">

  <tr><td style="padding:0 0 20px 4px;">
    <span style="font:600 13px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:.22em;text-transform:uppercase;color:${ink};">Postage</span>
    <span style="display:inline-block;margin-left:10px;padding:4px 9px;border:1px solid ${accent};border-radius:3px;font:600 10px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:.16em;text-transform:uppercase;color:${accent};">Held</span>
  </td></tr>

  <tr><td style="background:${card};border:1px solid ${line};border-radius:14px;padding:36px 32px 32px 32px;" bgcolor="${card}">

    <h1 style="margin:0;font:600 26px/1.25 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:-.02em;color:${ink};">
      Held at the door.
    </h1>
    <p style="margin:14px 0 0 0;font:400 16px/1.55 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:${inkSoft};">
      Your mail to <span style="font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:15px;color:${ink};">${escape(inbox)}</span>
      is safe. Answer once and we send it &#8212; you don&#8217;t write it twice.
    </p>

    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:22px 0 26px 0;">
      <tr><td style="border-left:3px solid ${line};padding:2px 0 2px 14px;font:400 14px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:${inkSoft};">
        Subject<br>
        <span style="color:${ink};font-size:15px;">${escape(subject)}</span>
      </td></tr>
    </table>

    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0">
      <tr><td style="padding:0 0 10px 0;">
        <a href="${escape(facts.challengeUrl)}?as=human"
           style="display:block;background:${accent};border-radius:10px;padding:15px 20px;text-decoration:none;" bgcolor="${accent}">
          <span style="display:block;font:600 16px/1.3 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:${onAccent};">I&#8217;m human &#8212; free</span>
          <span style="display:block;margin-top:3px;font:400 13px/1.4 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:${onAccent};">World ID. No account, no wallet.</span>
        </a>
      </td></tr>
      <tr><td>
        <a href="${escape(facts.challengeUrl)}?as=bot"
           style="display:block;background:${card};border:1px solid ${line};border-radius:10px;padding:14px 19px;text-decoration:none;" bgcolor="${card}">
          <span style="display:block;font:600 16px/1.3 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:${ink};">I&#8217;m a bot &#8212; pay ${escape(price)}</span>
          <span style="display:block;margin-top:3px;font:400 13px/1.4 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:${inkSoft};">Goes to them, not us.</span>
        </a>
      </td></tr>
    </table>

    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:28px 0 0 0;border-top:1px solid ${line};">
      <tr><td style="padding:20px 0 10px 0;font:600 11px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:.14em;text-transform:uppercase;color:${inkSoft};">Why</td></tr>
      ${reasons}
      <tr><td style="padding:10px 0 0 0;font:400 13px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:${inkSoft};">
        Held ${escape(deadline)}, then erased. Nobody reads it, nobody is charged.
      </td></tr>
    </table>

  </td></tr>

  <tr><td style="padding:14px 0 0 0;">
    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="background:${card};border:1px solid ${line};border-radius:14px;" bgcolor="${card}">
      <tr><td style="padding:22px 24px;">
        <span style="font:600 11px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:.14em;text-transform:uppercase;color:${accent};">Your inbox could be earning</span>
        <p style="margin:10px 0 0 0;font:600 18px/1.35 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:-.01em;color:${ink};">
          On the other side of this, someone is being paid.
        </p>
        <p style="margin:8px 0 0 0;font:400 14px/1.55 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:${inkSoft};">
          Hand out a Postage address instead of your own. Real people and anything urgent
          reach you free; everything else pays you for the interruption. Keep the inbox you
          already have &#8212; setup is one click.
        </p>
        <a href="${escape(facts.appUrl)}"
           style="display:inline-block;margin-top:14px;background:${accent};border-radius:8px;padding:11px 18px;text-decoration:none;font:600 14px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:${onAccent};" bgcolor="${accent}">
          Create an account and start earning
        </a>
      </td></tr>
    </table>
  </td></tr>

  <tr><td style="padding:20px 4px 0 4px;font:400 12px/1.6 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:${inkSoft};">
    You got this because you wrote to ${escape(inbox)}, filtered by Postage. We keep it until
    it&#8217;s delivered or the hold ends. Never sold, never listed.
  </td></tr>

</table>
</td></tr></table>
</body></html>`;
}

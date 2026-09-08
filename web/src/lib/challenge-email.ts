import { formatUsdc } from "./format";
import { postageAddress } from "./handle";

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
  tier: string;
  amount: bigint;
  reasons: string[];
  challengeUrl: string;
  appUrl: string;
  heldUntil: number;
}

/// The one message a held sender receives. It asks a single question - person or
/// machine - and each answer is a link. Nothing here asks them to write their
/// message again, because we still have it.
export function challengeMail(facts: ChallengeMailFacts): ChallengeMail {
  const inbox = postageAddress(facts.handle);
  const price = formatUsdc(facts.amount);
  const deadline = holdWindow(facts.heldUntil);

  return {
    subject: `Held for ${inbox}: did a person write this?`,
    html: html(facts, inbox, price, deadline),
    text: text(facts, inbox, price, deadline),
  };
}

function holdWindow(heldUntil: number): string {
  const hours = Math.max(1, Math.round((heldUntil - Date.now() / 1000) / 3600));
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

function text(facts: ChallengeMailFacts, inbox: string, price: string, deadline: string): string {
  return [
    `Your message to ${inbox} is being held.`,
    "",
    `Subject: ${facts.subject || "(no subject)"}`,
    "",
    "It has not been delivered and it has not been thrown away. Answer one",
    "question and we deliver the message you already sent, exactly as you wrote",
    "it. There is nothing to send again.",
    "",
    "A PERSON WROTE IT - free",
    `${facts.challengeUrl}?as=human`,
    "",
    `A MACHINE SENT IT - ${price}`,
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
function html(facts: ChallengeMailFacts, inbox: string, price: string, deadline: string): string {
  const reasons = facts.reasons
    .map(
      (reason) =>
        `<tr><td style="padding:0 0 8px 0;color:#6b6761;font-size:14px;line-height:21px;">
           <span style="color:#cf3f27;">&#8212;</span>&nbsp;&nbsp;${escape(reason)}</td></tr>`
    )
    .join("");

  return `<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="color-scheme" content="light">
<title>${escape(inbox)}</title>
</head>
<body style="margin:0;padding:0;background:#faf8f4;">
<div style="display:none;max-height:0;overflow:hidden;opacity:0;">Your message is held, not lost. One click sends it.</div>

<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="background:#faf8f4;">
<tr><td align="center" style="padding:32px 16px;">

<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="max-width:560px;">

  <tr><td style="padding:0 0 20px 4px;">
    <span style="font:600 13px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:.22em;text-transform:uppercase;color:#17161b;">Postage</span>
    <span style="display:inline-block;margin-left:10px;padding:4px 9px;border:1px solid #cf3f27;border-radius:3px;font:600 10px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:.16em;text-transform:uppercase;color:#cf3f27;">Held</span>
  </td></tr>

  <tr><td style="background:#ffffff;border:1px solid #e6e1d7;border-radius:14px;padding:36px 32px 32px 32px;">

    <h1 style="margin:0;font:600 26px/1.25 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:-.02em;color:#17161b;">
      Did a person write this?
    </h1>
    <p style="margin:14px 0 0 0;font:400 16px/1.55 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:#6b6761;">
      Your message to <span style="font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:15px;color:#17161b;">${escape(inbox)}</span>
      is being held. It was not delivered, and it was not thrown away.
      Answer this and we send the one you already wrote &#8212; you do not write it twice.
    </p>

    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:22px 0 26px 0;">
      <tr><td style="border-left:3px solid #e6e1d7;padding:2px 0 2px 14px;font:400 14px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:#9c968d;">
        Subject<br>
        <span style="color:#17161b;font-size:15px;">${escape(facts.subject || "(no subject)")}</span>
      </td></tr>
    </table>

    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0">
      <tr><td style="padding:0 0 10px 0;">
        <a href="${escape(facts.challengeUrl)}?as=human"
           style="display:block;background:#17161b;border-radius:10px;padding:15px 20px;text-decoration:none;">
          <span style="display:block;font:600 16px/1.3 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:#ffffff;">A person wrote it</span>
          <span style="display:block;margin-top:3px;font:400 13px/1.4 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:#b8b3ab;">Prove it once and it goes through free. No account, no wallet.</span>
        </a>
      </td></tr>
      <tr><td>
        <a href="${escape(facts.challengeUrl)}?as=bot"
           style="display:block;background:#ffffff;border:1px solid #d5cec1;border-radius:10px;padding:14px 19px;text-decoration:none;">
          <span style="display:block;font:600 16px/1.3 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:#17161b;">A machine sent it &#8212; pay ${escape(price)}</span>
          <span style="display:block;margin-top:3px;font:400 13px/1.4 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:#6b6761;">The money goes to the person you are writing to, not to us.</span>
        </a>
      </td></tr>
    </table>

    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:28px 0 0 0;border-top:1px solid #e6e1d7;">
      <tr><td style="padding:20px 0 10px 0;font:600 11px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:.14em;text-transform:uppercase;color:#9c968d;">Why it was held</td></tr>
      ${reasons}
      <tr><td style="padding:10px 0 0 0;font:400 13px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:#9c968d;">
        Held for ${escape(deadline)}. Ignore this and it is erased unread &#8212; nobody reads it and nobody is charged.
      </td></tr>
    </table>

  </td></tr>

  <tr><td style="padding:14px 0 0 0;">
    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="background:#fbeeea;border:1px solid #f0d4cc;border-radius:14px;">
      <tr><td style="padding:22px 24px;">
        <span style="font:600 11px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:.14em;text-transform:uppercase;color:#cf3f27;">Your inbox could be earning</span>
        <p style="margin:10px 0 0 0;font:600 18px/1.35 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;letter-spacing:-.01em;color:#17161b;">
          On the other side of this, someone is being paid.
        </p>
        <p style="margin:8px 0 0 0;font:400 14px/1.55 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:#6b6761;">
          Hand out a Postage address instead of your own. Real people and anything urgent
          reach you free; everything else pays you for the interruption. Keep the inbox you
          already have &#8212; setup is one click.
        </p>
        <a href="${escape(facts.appUrl)}"
           style="display:inline-block;margin-top:14px;background:#cf3f27;border-radius:8px;padding:11px 18px;text-decoration:none;font:600 14px/1 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:#ffffff;">
          Create an account and start earning
        </a>
      </td></tr>
    </table>
  </td></tr>

  <tr><td style="padding:20px 4px 0 4px;font:400 12px/1.6 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:#9c968d;">
    You are getting this because you wrote to ${escape(inbox)}, an address filtered by Postage.
    We keep your message only until it is delivered or the hold runs out, and we never sell it,
    read it back to anyone, or add you to a list.
  </td></tr>

</table>
</td></tr></table>
</body></html>`;
}

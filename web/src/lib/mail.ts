import { required } from "./env";

const RESEND_ENDPOINT = "https://api.resend.com/emails";

/// Sends the code that proves whoever is claiming a handle can read the address
/// they are pointing it at. Without this anyone could aim a Postage handle at a
/// stranger's inbox and have us forward to it.
export async function sendVerificationCode(to: string, handle: string, code: string): Promise<void> {
  const response = await fetch(RESEND_ENDPOINT, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${required("RESEND_API_KEY")}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      from: required("MAIL_FROM"),
      to,
      subject: `${code} is your Postage code`,
      text: body(handle, code),
    }),
  });

  if (!response.ok) {
    const detail = await response.text();
    throw new Error(`Could not send the verification code: ${detail.slice(0, 200)}`);
  }
}

function body(handle: string, code: string): string {
  return [
    `Your code is ${code}.`,
    "",
    `Someone asked us to deliver ${handle}@usepostage.com to this address.`,
    "Enter the code to confirm it was you. That is the whole of it.",
    "",
    "If you were not expecting this, ignore it. Nothing reaches you unless you",
    "confirm, and the code expires in 15 minutes.",
  ].join("\n");
}

/// Delivers a message to the inbox owner.
///
/// Every message goes out this way now. Cloudflare receives the mail and hands
/// it to us; it never forwards, which is what lets someone claim a handle
/// without confirming their address to Cloudflare as well as to us.
///
/// The sender's name leads the From line and their address is in Reply-To,
/// rather than either being forged into From. Authentication is then honestly
/// ours: the message really is from us, signed by us, and says whose words it
/// carries. Replying still reaches them.
export async function deliverMessage(message: {
  to: string;
  from: string;
  handle: string;
  subject: string;
  text?: string;
  html?: string;
  note?: string;
}): Promise<void> {
  const footer = message.note ?? `Sent to ${message.handle}@usepostage.com by ${message.from}.`;

  const response = await fetch(RESEND_ENDPOINT, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${required("RESEND_API_KEY")}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      from: asSender(message.from),
      to: message.to,
      reply_to: message.from,
      subject: message.subject,
      ...(message.html
        ? { html: `${message.html}<hr><p style="color:#888;font-size:12px">${footer}</p>` }
        : { text: [message.text ?? "", "", "—", footer].join("\n") }),
    }),
  });

  if (!response.ok) {
    throw new Error(`Could not deliver it: ${(await response.text()).slice(0, 200)}`);
  }
}

/// `Sarah (via Postage) <hello@usepostage.com>`. The recipient sees who wrote
/// to them in the message list rather than a column of identical rows, while
/// every signature on the message stays ours.
function asSender(from: string): string {
  const address = /<([^>]+)>/.exec(required("MAIL_FROM"))?.[1] ?? required("MAIL_FROM");
  const name = from.split("@")[0]?.replace(/[^\w .-]/g, "").slice(0, 40) || "Someone";
  return `${name} (via Postage) <${address}>`;
}

/// A message a cleared sender pasted back in, once their held copy had expired.
export async function relayHeldMessage(message: {
  to: string;
  from: string;
  handle: string;
  subject: string;
  body: string;
}): Promise<void> {
  await deliverMessage({
    to: message.to,
    from: message.from,
    handle: message.handle,
    subject: message.subject,
    text: message.body,
    note: `Sent to ${message.handle}@usepostage.com by ${message.from}, who cleared the gate. Replying goes straight to them.`,
  });
}

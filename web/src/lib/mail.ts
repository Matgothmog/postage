import { required } from "./env";
import { postageAddress } from "./handle";
import { CODE_TTL_SECONDS } from "./verification";

const RESEND_ENDPOINT = "https://api.resend.com/emails";

/// One way out for both of the messages Postage writes itself. Neither is a
/// forward: a released message never passes through here, because it has to
/// leave as the bytes that arrived.
interface Outgoing {
  to: string;
  subject: string;
  text: string;
  /// Where a reply should go, when that is not us.
  replyTo?: string;
}

async function send(message: Outgoing, whatFailed: string): Promise<void> {
  const response = await fetch(RESEND_ENDPOINT, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${required("RESEND_API_KEY")}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      from: required("MAIL_FROM"),
      to: message.to,
      subject: message.subject,
      text: message.text,
      ...(message.replyTo ? { reply_to: message.replyTo } : {}),
    }),
  });

  if (!response.ok) throw new Error(`${whatFailed}: ${(await response.text()).slice(0, 200)}`);
}

/// Sends the code that proves whoever is claiming a handle can read the address
/// they are pointing it at. Without this anyone could aim a Postage handle at a
/// stranger's inbox and have us forward to it.
export async function sendVerificationCode(to: string, handle: string, code: string): Promise<void> {
  await send(
    { to, subject: `${code} is your Postage code`, text: body(handle, code) },
    "Could not send the verification code"
  );
}

function body(handle: string, code: string): string {
  return [
    code,
    "",
    `Someone pointed ${postageAddress(handle)} at this address. Enter the code`,
    "above to confirm it was you.",
    "",
    "Cloudflare, who carries the mail, is sending a separate confirmation too.",
    "Both are needed before anything forwards here.",
    "",
    "Not you? Ignore both. Nothing forwards without confirming, and this code",
    `expires in ${CODE_TTL_SECONDS / 60} minutes.`,
  ].join("\n");
}

/// Sends a message a cleared sender pasted back in.
///
/// It goes out under our own name with theirs in Reply-To, rather than forged
/// into the From line. A message that claimed to be from them would be
/// unsigned mail wearing their domain, which is the thing this whole gateway
/// exists to catch.
export async function relayHeldMessage(message: {
  to: string;
  from: string;
  handle: string;
  subject: string;
  body: string;
}): Promise<void> {
  await send(
    {
      to: message.to,
      replyTo: message.from,
      subject: message.subject,
      text: [
        message.body,
        "",
        "—",
        `${message.from} cleared the gate. Sent to ${postageAddress(message.handle)}.`,
        "Reply goes straight to them.",
      ].join("\n"),
    },
    "Could not deliver it"
  );
}

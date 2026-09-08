import { required } from "./env";
import { postageAddress } from "./handle";
import { CODE_TTL_SECONDS } from "./verification";

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
    `Someone asked us to forward ${postageAddress(handle)} to this address.`,
    "Enter the code to confirm it was you.",
    "",
    "Cloudflare, who carries the mail, has sent a separate email asking you to",
    "confirm the same thing. Both are needed before anything is forwarded here.",
    "",
    "If you were not expecting this, ignore both. Nothing reaches you unless you",
    `confirm, and the code expires in ${CODE_TTL_SECONDS / 60} minutes.`,
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
  const response = await fetch(RESEND_ENDPOINT, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${required("RESEND_API_KEY")}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      from: required("MAIL_FROM"),
      to: message.to,
      reply_to: message.from,
      subject: message.subject,
      text: [
        message.body,
        "",
        "—",
        `Sent to ${postageAddress(message.handle)} by ${message.from}, who cleared the gate.`,
        "Replying goes straight to them.",
      ].join("\n"),
    }),
  });

  if (!response.ok) {
    throw new Error(`Could not deliver it: ${(await response.text()).slice(0, 200)}`);
  }
}

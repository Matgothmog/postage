import { claimHold, inboxByHandle, markDelivered } from "./db";
import { required } from "./env";

export type Release =
  | { delivered: true }
  | { delivered: false; reason_undelivered: "expired" | "no_inbox" | "send_failed" };

/// Sends the message that was held, so answering the question is the whole of
/// what a sender does.
///
/// The message never passes through here. The worker holds the bytes and hands
/// them straight to the relay, so this says "send it" and learns whether it
/// went - which is what keeps a released message identical to the one sent.
///
/// The hold is claimed first, and claiming is one conditional update. Two clicks
/// a second apart cannot both come away believing they may send it, so a message
/// cannot be delivered twice; a failure afterwards leaves the sender with the
/// paste-it-back route rather than a duplicate in someone's inbox.
export async function releaseHeldMessage(token: string, handle: string): Promise<Release> {
  const inbox = await inboxByHandle(handle);
  if (!inbox) return { delivered: false, reason_undelivered: "no_inbox" };
  if (!(await claimHold(token))) return { delivered: false, reason_undelivered: "expired" };

  try {
    const response = await fetch(`${required("MAIL_WORKER_URL")}/release`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "x-postage-secret": required("MAIL_WEBHOOK_SECRET"),
      },
      body: JSON.stringify({ token, to: inbox.destination }),
    });
    if (response.ok) {
      await markDelivered(token);
      return { delivered: true };
    }
  } catch {
    // The gate is open either way; the sender can paste it back in.
  }
  return { delivered: false, reason_undelivered: "send_failed" };
}

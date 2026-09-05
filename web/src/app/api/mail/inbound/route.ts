import { randomUUID } from "node:crypto";
import { keccak256, stringToBytes } from "viem";
import {
  deliver,
  insertMessage,
  isKnownSender,
  walletForInbox,
} from "@/lib/db";
import { required } from "@/lib/env";

interface InboundPayload {
  from: string;
  to: string;
  subject: string;
  body: string;
}

/// Called by the Cloudflare Email Worker for every message arriving at the
/// domain. Decides whether the message goes straight through or gets held
/// until the sender proves they are human or attaches postage.
export async function POST(request: Request) {
  if (request.headers.get("x-postage-secret") !== required("MAIL_WEBHOOK_SECRET")) {
    return Response.json({ error: "Bad secret" }, { status: 401 });
  }

  // Read the config before touching the database. Discovering a missing
  // variable after the insert leaves a message stored that nobody can ever be
  // told how to unlock.
  const appUrl = required("APP_URL");

  const payload = (await request.json()) as Partial<InboundPayload>;
  const { from, to, subject, body } = payload;
  if (!from || !to) {
    return Response.json({ error: "from and to are required" }, { status: 400 });
  }

  const localPart = to.split("@")[0]?.toLowerCase() ?? "";
  if (!(await walletForInbox(localPart))) {
    return Response.json({ status: "unknown_inbox" }, { status: 404 });
  }

  const token = randomUUID().replaceAll("-", "");
  const receivedAt = Math.floor(Date.now() / 1000);
  const sender = from.toLowerCase();

  await insertMessage({
    id: randomUUID(),
    // The id the sender will pay against onchain. Derived from the message so
    // one stamp can only ever unlock the message it was bought for.
    message_hash: keccak256(stringToBytes(`${sender}|${localPart}|${subject ?? ""}|${receivedAt}`)),
    sender,
    recipient_local: localPart,
    subject: subject ?? "(no subject)",
    body: body ?? "",
    status: "held",
    unlock_token: token,
    received_at: receivedAt,
  });

  // Someone the recipient has already corresponded with never pays again.
  // Without this every first reply from a friend would be held too.
  if (await isKnownSender(localPart, sender)) {
    await deliver(token, "known");
    return Response.json({ status: "delivered", reason: "known_sender" });
  }

  return Response.json({
    status: "held",
    unlock_url: `${appUrl}/u/${token}`,
  });
}

import { challengeByToken, inboxByHandle, spendPass } from "@/lib/db";
import { relayHeldMessage } from "@/lib/mail";

const MAX_SUBJECT = 200;
const MAX_BODY = 20_000;

/// Delivers a message the sender pastes back in, once they have cleared the
/// gate. Postage never kept the one that was refused — it was turned away at
/// the door and only ever existed in their outbox — so getting it through
/// otherwise means going back to their mail client and sending it again. This
/// saves that trip without anyone having to store mail.
///
/// It is not a relay anyone can use: it spends the same pass an inbound message
/// would have spent, so it can only send what the sender had already earned the
/// right to send.
export async function POST(request: Request) {
  const { token, subject, body } = (await request.json()) as {
    token?: string;
    subject?: string;
    body?: string;
  };

  if (!token || !body?.trim()) {
    return Response.json({ error: "token and a message are required" }, { status: 400 });
  }
  if (body.length > MAX_BODY || (subject?.length ?? 0) > MAX_SUBJECT) {
    return Response.json({ error: "That message is too long to send this way" }, { status: 413 });
  }

  const challenge = await challengeByToken(token);
  if (!challenge) return Response.json({ error: "Unknown challenge" }, { status: 404 });
  if (!challenge.resolved_at) {
    return Response.json({ error: "Clear the gate first" }, { status: 403 });
  }

  const inbox = await inboxByHandle(challenge.handle);
  if (!inbox) return Response.json({ error: "That inbox no longer exists" }, { status: 404 });

  const pass = await spendPass(challenge.handle, challenge.sender);
  if (!pass) {
    return Response.json(
      { error: "That pass has run out. Prove you are a person again, or pay" },
      { status: 403 }
    );
  }

  try {
    await relayHeldMessage({
      to: inbox.destination,
      from: challenge.sender,
      handle: challenge.handle,
      subject: subject?.trim() || "(no subject)",
      body: body.trim(),
    });
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : "Could not deliver it";
    return Response.json({ error: detail }, { status: 502 });
  }

  return Response.json({ status: "delivered", to: challenge.handle });
}

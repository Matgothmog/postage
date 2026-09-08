import { classify, extractUrls } from "@/lib/classify";
import { challengeByToken, inboxByHandle, refundPass, spendPass } from "@/lib/db";
import { relayHeldMessage } from "@/lib/mail";

const MAX_SUBJECT = 200;
const MAX_BODY = 20_000;

/// Delivers a message the sender pastes back in. The way through is normally
/// the one we are holding, released as the bytes that arrived — this is what is
/// left when there is nothing to release: a hold that ran out, a sender who was
/// refused inside the session rather than held, or a relay that would not take
/// it. What goes out here is written in our form rather than theirs, so it goes
/// under our name; it is the fallback, not the path.
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

  // What was pasted is not what was judged. The verdict on this token describes
  // the message that was held; the box below it accepts anything, and the pass
  // that authorises the send belongs to the sender rather than to this token —
  // so a benign message cleared earlier would otherwise carry a phishing one
  // through here. Both are checked: the tier this token was given, and the
  // words actually about to be delivered.
  if (challenge.tier === "dangerous") {
    return Response.json(
      { error: "This will not be delivered whoever sends it, and paying did not buy that" },
      { status: 403 }
    );
  }

  const inbox = await inboxByHandle(challenge.handle);
  if (!inbox) return Response.json({ error: "That inbox no longer exists" }, { status: 404 });

  // Taken before the classifier runs. Knowing a token is the only thing this
  // route asks for, and a token stays settled for good, so classifying first
  // would let anyone holding a spent one run up model calls in a loop.
  const pass = await spendPass(challenge.handle, challenge.sender);
  if (!pass) {
    return Response.json(
      { error: "That pass has run out. Prove you are a person again, or pay" },
      { status: 403 }
    );
  }

  try {
    const pasted = await classify({
      from: challenge.sender,
      to: `${challenge.handle}@usepostage.com`,
      subject: subject?.trim() ?? "",
      body: body.trim(),
      spf: null,
      dkim: null,
      dmarc: null,
      urls: extractUrls(body),
    });
    if (pasted.tier === "dangerous") {
      return Response.json(
        { error: "That reads as an attempt to deceive the recipient, so it will not be sent" },
        { status: 403 }
      );
    }

    await relayHeldMessage({
      to: inbox.destination,
      from: challenge.sender,
      handle: challenge.handle,
      subject: subject?.trim() || "(no subject)",
      body: body.trim(),
    });
  } catch (cause) {
    // Nothing was delivered, so the use goes back. A sender who paid and then
    // met an outage would otherwise be left with a settled challenge, a spent
    // pass and no way through that they could buy again.
    await refundPass(challenge.handle, challenge.sender);
    const detail = cause instanceof Error ? cause.message : "Could not deliver it";
    return Response.json({ error: detail }, { status: 502 });
  }

  return Response.json({ status: "delivered", to: challenge.handle });
}

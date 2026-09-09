import { classify, extractUrls } from "@/lib/classify";
import { challengeByToken } from "@/lib/db/challenges";
import { claimClassification, releaseClassificationSlot } from "@/lib/db/classifications";
import { inboxByHandle } from "@/lib/db/inboxes";
import { extendPassIfExpiring, hasLivePass, refundPass, spendPass } from "@/lib/db/passes";
import { postageAddress } from "@/lib/handle";
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

  // Checked without being spent, so the classifier below cannot be run up by
  // anyone holding a token whose pass is long gone — knowing a token is all
  // this route asks for, and a token stays settled for good.
  if (!(await hasLivePass(challenge.handle, challenge.sender))) {
    return Response.json(
      { error: "That pass has run out. Prove you are a person again, or pay" },
      { status: 403 }
    );
  }

  // The same budget the inbound path answers to. A human pass is unlimited for
  // fifteen minutes, so without this anyone holding one could post bodies in a
  // loop and every one would be a model call nothing counted.
  // Counted against its own pool, not the inbox's. Sharing it let a handful of
  // senders with live passes spend a recipient's whole hourly allowance on
  // refused pastes, after which that inbox stopped being read at all.
  const budgetRefusal = await claimClassification(`paste:${challenge.handle}`, challenge.sender);
  if (budgetRefusal) {
    return Response.json(
      { error: "Too much has been sent this hour. Try again later" },
      { status: 429 }
    );
  }

  const pasted = await classify({
    from: challenge.sender,
    to: postageAddress(challenge.handle),
    subject: subject?.trim() ?? "",
    body: body.trim(),
    spf: null,
    dkim: null,
    dmarc: null,
    urls: extractUrls(body),
  });

  // Judged before anything is spent, so a refusal costs the sender nothing and
  // a false positive does not burn a delivery they paid for.
  if (pasted.tier === "dangerous") {
    return Response.json(
      { error: "That reads as an attempt to deceive the recipient, so it will not be sent" },
      { status: 403 }
    );
  }

  // A verdict from the headers alone cannot say "dangerous" at all, and this
  // text has no headers to read. Relaying it under our own name while unable to
  // judge it is how a gateway lends its reputation to whatever it is handed.
  if (pasted.degraded) {
    // The window is pushed back out, because the reason we cannot judge this is
    // ours. Without it a long outage silently spends a paid sender's fifteen
    // minutes and leaves them with nothing. Only when it is nearly gone, so
    // repeated polling cannot hold a pass open indefinitely.
    // The slot goes back too: nothing reached the model, and burning twenty of
    // them on retries during an outage would lock the sender out for the hour
    // having sent nothing.
    await releaseClassificationSlot(`paste:${challenge.handle}`, challenge.sender);
    await extendPassIfExpiring(challenge.handle, challenge.sender);
    return Response.json(
      { error: "Cannot check that right now. Try again in a few minutes" },
      { status: 503 }
    );
  }

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
    // Only a counted pass had anything taken from it. An unlimited window is
    // returned untouched by spendPass, so refunding here would find whatever
    // row exists by then and add a use nobody spent.
    if (pass.uses_left !== null) {
      // Best effort: whatever broke the relay may break this too, and the
      // sender should still be told what actually went wrong.
      await refundPass(challenge.handle, challenge.sender).catch(() => {});
    }
    const detail = cause instanceof Error ? cause.message : "Could not deliver it";
    return Response.json({ error: detail }, { status: 502 });
  }

  return Response.json({ status: "delivered", to: challenge.handle });
}

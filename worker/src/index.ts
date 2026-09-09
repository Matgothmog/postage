import PostalMime from "postal-mime";
import type { GatewayNotice, GatewayVerdict } from "../../shared/gateway-verdict.ts";

interface Env {
  POSTAGE_API_URL: string;
  POSTAGE_SECRET: string;
  /// Held mail, as the exact bytes that arrived. Nothing else has a copy.
  ///
  /// Each value carries its own expiry, so a hold nobody answers is dropped by
  /// Cloudflare at its deadline rather than by us noticing later. Reads happen
  /// however long a person takes to open their mail, so KV's consistency window
  /// does not come into it.
  HELD: KVNamespace;
  /// Carries a released message out again. Cloudflare cannot: `send_email`
  /// refuses raw MIME whose `From:` is not a domain on this account, and
  /// rewriting `From:` is the one thing a forward must never do.
  MAILGUN_API_BASE: string;
  MAILGUN_DOMAIN: string;
  MAILGUN_API_KEY: string;
}

/// Only reached if the gateway sends a hold with no deadline on it. Not the
/// source of truth for how long a message is kept - that is the gateway's - just
/// a floor under it, so nothing can be stored indefinitely by omission.
const FALLBACK_HOLD_SECONDS = 24 * 60 * 60;

/// Said whenever the fault is ours rather than the sender's, and deliberately
/// said the same way every time: it is a temporary refusal, and the sending
/// MTA's own retry is what recovers the message afterwards.
const RETRY_LATER = "Postage is temporarily unavailable, please retry";

/// How many times a released hold is asked to go away before we stop asking.
/// The message is already sent by then, so the extra attempt costs nothing but
/// a little latency on a path that is failing anyway, and buys back the common
/// case: a KV write that stumbles once and lands on the next try.
const RETIRE_ATTEMPTS = 2;

/// What the receiving MTA concluded about the sender, read out of
/// `Authentication-Results`.
///
/// A sender may write this header themselves, and `Headers.get()` joins every
/// copy into one string with ", ", so a naive scan of that string can return
/// whichever copy happens to mention the method first. Cloudflare's own is the
/// one that means anything; the rest are the sender's claims about the sender.
///
/// We cannot tell them apart from here, so a method is only believed when every
/// copy agrees on it. A sender who adds `dmarc=pass` to a message Cloudflare
/// marked `dmarc=fail` now contradicts it and comes away with nothing, where
/// before they could hand us the answer.
///
/// What is left is a message for which Cloudflare recorded no result at all:
/// there is then nothing to disagree with, and a forged claim stands. That is
/// narrow, because a forged claim only buys anything if Cloudflare stated
/// neither dmarc nor spf — state either and the two either agree, in which case
/// the forgery changed nothing, or they do not, in which case both are dropped.
/// Closing it needs Cloudflare's authserv-id, which is not established.
///
/// Honest mail is unaffected. A relay that adds its own results either agrees
/// or is describing a different hop; where it disagrees about DKIM — a message
/// with two signatures, one of which does not verify — the method reads unknown
/// rather than pass, which no path treats as a failure.
///
/// A method name has to start a token, which is more than a word boundary asks
/// for. `policy.dmarc=none` states the domain's published policy and
/// `x-dkim=fail` is a vendor's own method: both are ordinary RFC 8601 syntax,
/// and both put a word boundary immediately in front of a method name. Read
/// that way they are second copies that disagree, so a DMARC-passing message
/// that also carries its policy comes away unknown — honest mail downgraded to
/// unauthenticated by its own authentication header. Only a word character, a
/// dot or a hyphen can put a name inside a larger token, so those are what the
/// lookbehind rules out.
///
/// This is still a scan rather than a parse — text inside a quoted `reason=`
/// is read like anything else. Reading too much can only add a value to the
/// set, which either agrees or loses the method, so it errs toward unknown.
export function authResults(header: string | null): { spf: string | null; dkim: string | null; dmarc: string | null } {
  const read = (method: string) => {
    const found = header?.matchAll(new RegExp(`(?<![\\w.-])${method}=(\\w+)`, "gi")) ?? [];
    const claimed = new Set([...found].map((match) => match[1].toLowerCase()));
    return claimed.size === 1 ? [...claimed][0] : null;
  };
  return { spf: read("spf"), dkim: read("dkim"), dmarc: read("dmarc") };
}

export default {
  async email(message: ForwardableEmailMessage, env: Env): Promise<void> {
    // Buffered before parsing, because the raw stream reads once and holding a
    // message means keeping exactly these bytes rather than a rendering of them.
    const raw = await new Response(message.raw).arrayBuffer();
    const parsed = await PostalMime.parse(raw);
    const auth = authResults(message.headers.get("authentication-results"));

    let verdict: GatewayVerdict;
    try {
      verdict = await ask(env, {
        from: message.from,
        to: message.to,
        subject: parsed.subject ?? "",
        body: parsed.text ?? parsed.html ?? "",
        ...auth,
      });
    } catch (cause) {
      // Without this the sender retries into a wall nobody can explain.
      console.error("classify failed", {
        gateway: safeHost(env.POSTAGE_API_URL),
        cause: causeMessage(cause),
      });
      // Refused rather than forwarded unfiltered, so the sending MTA holds the
      // message and retries rather than the recipient losing the gate.
      message.setReject(RETRY_LATER);
      return;
    }

    // Untouched, so the sender's DKIM signature still covers what arrives and
    // their address still displays as the one that wrote it.
    if (verdict.action === "forward") {
      if (!verdict.to) {
        rejectIncoherentVerdict(message, "forward without a destination");
        return;
      }

      try {
        await message.forward(verdict.to);
      } catch {
        // A destination Cloudflare will not accept must refuse the session, so
        // the sending MTA retries. Throwing here loses the message instead.
        message.setReject("Postage could not deliver to that inbox, please retry");
      }
      return;
    }

    if (verdict.action === "hold") {
      if (!verdict.token) {
        rejectIncoherentVerdict(message, "hold without a token");
        return;
      }

      const heldUntil = verdict.held_until ?? Math.floor(Date.now() / 1000) + FALLBACK_HOLD_SECONDS;
      try {
        await env.HELD.put(verdict.token, raw, { expiration: heldUntil });
      } catch (cause) {
        // KV holds the only copy of the message. A put that did not land leaves
        // the release link pointing at nothing, so telling the sender it is
        // being kept would be a lie about mail we no longer have. Refusing
        // instead leaves the bytes at the sending MTA, which is then the only
        // place they still exist. Not logged: the token is the capability that
        // releases the message.
        console.error("hold failed", { cause: causeMessage(cause) });
        message.setReject(RETRY_LATER);
        return;
      }

      if (verdict.notice && (await replied(message, verdict.notice))) return;

      message.setReject(verdict.bounce ?? "Held. See the link in this message to release it");
      return;
    }

    if (verdict.reason === "unknown_inbox") {
      message.setReject("No such address at this domain");
      return;
    }

    message.setReject(verdict.bounce ?? "Not delivered.");
  },

  /// The one route. Everything else here answers 404.
  async fetch(request: Request, env: Env): Promise<Response> {
    const { pathname } = new URL(request.url);
    if (request.method !== "POST" || pathname !== "/release") {
      return new Response("Not found", { status: 404 });
    }
    if (request.headers.get("x-postage-secret") !== env.POSTAGE_SECRET) {
      return new Response("Bad secret", { status: 401 });
    }

    let body: { token?: string; to?: string };
    try {
      body = await request.json();
    } catch {
      return new Response("Body must be JSON", { status: 400 });
    }

    const { token, to } = body;
    if (!token || !to) return new Response("token and to are required", { status: 400 });

    let held: ArrayBuffer | null;
    try {
      held = await env.HELD.get(token, "arrayBuffer");
    } catch (cause) {
      // Nothing has been sent and the hold is untouched, so asking again costs
      // the caller nothing - which makes this the same temporary fault of ours
      // that the inbound side refuses a session over, said the same way.
      // Answered rather than thrown: an escaping rejection is an unhandled
      // worker exception, where every other failure here is a response. Not
      // logged: the token is the capability that releases the message.
      console.error("release lookup failed", { cause: causeMessage(cause) });
      return new Response(RETRY_LATER, { status: 503 });
    }
    if (!held) return new Response("Nothing is held under that token", { status: 404 });

    try {
      await deliverUntouched(env, held, to);
    } catch (cause) {
      // Kept, so the sender can be told it did not go and try again rather than
      // losing a message they were promised was safe. Logged, not returned:
      // Mailgun's own wording, or a raw network error, is not something the
      // caller needs or should read about our infrastructure - every other
      // failure in this handler answers with fixed wording, and this one
      // should too.
      console.error("release delivery failed", { cause: causeMessage(cause) });
      return new Response("Could not send it", { status: 502 });
    }

    await retireHold(env, token);
    return Response.json({ sent: true });
  },
};

/// Puts the message back on the wire as the bytes that arrived.
///
/// Every option here turns something off. Mailgun would otherwise sign the
/// message with our key and rewrite every link in the body for click tracking,
/// and rewriting the body changes what the sender's own signature covers - the
/// message would arrive looking forged by exactly the measure this gateway
/// exists to apply. The envelope sender is Mailgun's, as it is in any forward;
/// the `From:` header, which is what the recipient sees and what DMARC aligns
/// against, is untouched.
async function deliverUntouched(env: Env, raw: ArrayBuffer, to: string): Promise<void> {
  const form = new FormData();
  form.append("to", to);
  form.append("message", new Blob([raw], { type: "message/rfc822" }), "held.eml");
  form.append("o:dkim", "no");
  form.append("o:tracking", "no");
  form.append("o:tracking-clicks", "no");
  form.append("o:tracking-opens", "no");

  const response = await fetch(`${env.MAILGUN_API_BASE}/v3/${env.MAILGUN_DOMAIN}/messages.mime`, {
    method: "POST",
    headers: { Authorization: `Basic ${btoa(`api:${env.MAILGUN_API_KEY}`)}` },
    body: form,
  });

  if (!response.ok) {
    throw new Error(`Mailgun returned ${response.status}: ${(await response.text()).slice(0, 200)}`);
  }
}

/// Spends the token, once the message it releases has already gone out.
///
/// The order is forced, and it is the whole design. KV holds the only copy, so
/// the bytes cannot be dropped before Mailgun has taken them - which makes a
/// release at-least-once, because the send is irreversible by the time the key
/// is removed. The alternative, retiring the token first, is at-most-once: a
/// Mailgun outage, much the commoner failure, would then destroy a message the
/// recipient explicitly asked for, with no copy left anywhere to retry from. A
/// message arriving twice is a nuisance; a message that no longer exists cannot
/// be recovered by anyone. It is also the call the delivery-failure path above
/// already makes, deliberately, in leaving the token spendable.
///
/// So a delete that fails must not become an error. The mail is out, an error
/// is an invitation to retry a request that worked, and the retry would send it
/// a second time. Answering `sent: true` is both the truth and the guard: the
/// caller records the delivery and never presents the token again, which is
/// exactly what the old unguarded reject took away by leaving it with no answer
/// at all - the caller then reported a failed send for mail that had arrived,
/// and the sender was invited to send it once more by hand.
///
/// Retried because what is left behind is a live capability and a KV write that
/// fails once usually does not fail twice. If every attempt fails the key
/// survives its own release, and nothing in a stateless handler can prevent
/// that: the only durable record of "spent" is the write that will not land.
/// It expires on the deadline set when the message was held, so it does not
/// accumulate.
async function retireHold(env: Env, token: string): Promise<void> {
  let lastCause: unknown;
  for (let attempt = 0; attempt < RETIRE_ATTEMPTS; attempt += 1) {
    try {
      await env.HELD.delete(token);
      return;
    } catch (cause) {
      lastCause = cause;
    }
  }
  // Not logged: the token is the capability that releases the message.
  console.error("hold not retired after release", {
    attempts: RETIRE_ATTEMPTS,
    cause: causeMessage(lastCause),
  });
}

/// A verdict that asks for something and omits the one field that carries it
/// out: a forward naming nowhere, a hold with nothing to key it under. Nothing
/// here can complete it, and the generic refusal at the end of the handler
/// would file our own bug as an ordinary "not delivered" and lose a message the
/// gateway meant to keep. Refused as temporary instead, so the message waits at
/// the sending MTA while the log names what was wrong with the answer.
function rejectIncoherentVerdict(message: ForwardableEmailMessage, verdict: string): void {
  console.error("incoherent verdict", { verdict });
  message.setReject(RETRY_LATER);
}

/// True if the sender was told. A refusal here is not a failure worth losing the
/// message over - Cloudflare will not let us reply to an unauthenticated sender,
/// which is exactly the case where replying would mail the wrong person - so the
/// caller falls back to refusing inside the session.
async function replied(message: ForwardableEmailMessage, notice: GatewayNotice): Promise<boolean> {
  try {
    await message.reply({
      from: { name: "Postage", email: message.to },
      subject: notice.subject,
      text: notice.text,
      html: notice.html,
    });
    return true;
  } catch {
    return false;
  }
}

async function ask(
  env: Env,
  payload: {
    from: string;
    to: string;
    subject: string;
    body: string;
    spf: string | null;
    dkim: string | null;
    dmarc: string | null;
  }
): Promise<GatewayVerdict> {
  const response = await fetch(`${env.POSTAGE_API_URL}/api/mail/inbound`, {
    method: "POST",
    headers: { "Content-Type": "application/json", "x-postage-secret": env.POSTAGE_SECRET },
    body: JSON.stringify(payload),
  });

  // 404 is a real answer (no such inbox), not a failure to reach the gateway.
  if (response.status === 404) return { action: "reject", reason: "unknown_inbox" };
  if (!response.ok) {
    throw new Error(`Gateway returned ${response.status}: ${(await response.text()).slice(0, 300)}`);
  }
  return (await response.json()) as GatewayVerdict;
}

/// What to write down about a thrown value. A worker can be handed anything - a
/// fetch rejection, a KV error, a string some library threw - so this is stated
/// once and every failure in this file is logged the same shape.
function causeMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/// The host on its own. Enough to tell a misconfigured gateway from an
/// unreachable one, without writing a configured URL into the logs whole.
function safeHost(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return "unparseable POSTAGE_API_URL";
  }
}

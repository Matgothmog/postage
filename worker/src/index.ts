import PostalMime from "postal-mime";

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

interface Notice {
  subject: string;
  html: string;
  text: string;
}

interface Verdict {
  action: "forward" | "hold" | "reject";
  /// Verified destination, on `forward` only.
  to?: string;
  reason?: string;
  /// Key the message is held under, on `hold` only.
  token?: string;
  held_until?: number;
  /// Absent when answering the sender would mean mailing someone whose name was
  /// forged, in which case the SMTP refusal carries the link instead.
  notice?: Notice | null;
  /// What to say inside the SMTP session if we do not write back.
  bounce?: string;
}

/// Only reached if the gateway sends a hold with no deadline on it. Not the
/// source of truth for how long a message is kept - that is the gateway's - just
/// a floor under it, so nothing can be stored indefinitely by omission.
const FALLBACK_HOLD_SECONDS = 24 * 60 * 60;

function authResults(header: string | null): { spf: string | null; dkim: string | null; dmarc: string | null } {
  const read = (method: string) => {
    const found = header?.match(new RegExp(`\\b${method}=(\\w+)`, "i"));
    return found ? found[1].toLowerCase() : null;
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

    let verdict: Verdict;
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
        cause: cause instanceof Error ? cause.message : String(cause),
      });
      // Refused rather than forwarded unfiltered, so the sending MTA holds the
      // message and retries rather than the recipient losing the gate.
      message.setReject("Postage is temporarily unavailable, please retry");
      return;
    }

    // Untouched, so the sender's DKIM signature still covers what arrives and
    // their address still displays as the one that wrote it.
    if (verdict.action === "forward" && verdict.to) {
      try {
        await message.forward(verdict.to);
      } catch {
        // A destination Cloudflare will not accept must refuse the session, so
        // the sending MTA retries. Throwing here loses the message instead.
        message.setReject("Postage could not deliver to that inbox, please retry");
      }
      return;
    }

    if (verdict.action === "hold" && verdict.token) {
      const heldUntil = verdict.held_until ?? Math.floor(Date.now() / 1000) + FALLBACK_HOLD_SECONDS;
      await env.HELD.put(verdict.token, raw, { expiration: heldUntil });

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

    const held = await env.HELD.get(token, "arrayBuffer");
    if (!held) return new Response("Nothing is held under that token", { status: 404 });

    try {
      await deliverUntouched(env, held, to);
    } catch (cause) {
      // Kept, so the sender can be told it did not go and try again rather than
      // losing a message they were promised was safe.
      return new Response(cause instanceof Error ? cause.message : "Could not send it", { status: 502 });
    }

    await env.HELD.delete(token);
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

/// True if the sender was told. A refusal here is not a failure worth losing the
/// message over - Cloudflare will not let us reply to an unauthenticated sender,
/// which is exactly the case where replying would mail the wrong person - so the
/// caller falls back to refusing inside the session.
async function replied(message: ForwardableEmailMessage, notice: Notice): Promise<boolean> {
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
): Promise<Verdict> {
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
  return (await response.json()) as Verdict;
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

import PostalMime from "postal-mime";

interface Env {
  POSTAGE_API_URL: string;
  POSTAGE_SECRET: string;
}

type Verdict = {
  /// `delivered` means the gateway has already sent it on. Cloudflare never
  /// forwards, so no destination address has to be verified with it, and
  /// claiming a handle needs nothing but the code we email.
  action: "delivered" | "reject";
  reason?: string;
  challenge_url?: string;
  /// True when the gateway is holding the message, so clearing the gate
  /// delivers it and the sender never sends it twice.
  held?: boolean;
};

/// Reads the Authentication-Results the receiving MTA already wrote, so the
/// classifier is told whether the sender is who they claim rather than having
/// to guess from the prose.
function authResults(header: string | null): { spf: string | null; dkim: string | null; dmarc: string | null } {
  const read = (method: string) => {
    const found = header?.match(new RegExp(`\\b${method}=(\\w+)`, "i"));
    return found ? found[1].toLowerCase() : null;
  };
  return { spf: read("spf"), dkim: read("dkim"), dmarc: read("dmarc") };
}

export default {
  async email(message: ForwardableEmailMessage, env: Env): Promise<void> {
    const parsed = await PostalMime.parse(message.raw);
    const auth = authResults(message.headers.get("authentication-results"));

    let verdict: Verdict;
    try {
      verdict = await ask(env, {
        from: message.from,
        to: message.to,
        subject: parsed.subject ?? "",
        body: parsed.text ?? "",
        // Most commercial mail is HTML only, and it is the tier that pays, so
        // it should not arrive as a wall of markup.
        html: parsed.html ?? undefined,
        ...auth,
      });
    } catch {
      // Refused rather than forwarded unfiltered, so the sending MTA holds the
      // message and retries rather than the recipient losing the gate.
      message.setReject("Postage is temporarily unavailable, please retry");
      return;
    }

    // Accepted with no further action: the gateway has already delivered it.
    if (verdict.action === "delivered") return;

    if (verdict.reason === "unknown_inbox") {
      message.setReject("No such address at this domain");
      return;
    }

    // This line is the only thing the sender ever sees, so it has to say what
    // happened and what to do about it in one breath. Their mail is still in
    // their outbox; the link is how it gets through.
    message.setReject(rejection(verdict));
  },
};

function rejection(verdict: Verdict): string {
  if (!verdict.challenge_url) return "Not delivered.";
  if (verdict.reason === "delivery_failed") {
    return "Postage could not deliver this right now, please retry";
  }
  if (verdict.reason === "dangerous") {
    return `Not delivered: this looks like an attempt to deceive the recipient, and paying will not change that. If it is a mistake, say so at ${verdict.challenge_url}`;
  }
  if (verdict.held) {
    return `Held for 15 minutes, not lost: prove you are a person for free, or pay, and it is delivered for you - no need to send it again - ${verdict.challenge_url}`;
  }
  return `Not delivered: prove you are a person for free, or pay, then send again - ${verdict.challenge_url}`;
}

async function ask(
  env: Env,
  payload: {
    from: string;
    to: string;
    subject: string;
    body: string;
    html?: string;
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
  if (!response.ok) throw new Error(`Gateway returned ${response.status}`);
  return (await response.json()) as Verdict;
}

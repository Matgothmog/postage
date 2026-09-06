import PostalMime from "postal-mime";

interface Env {
  POSTAGE_API_URL: string;
  POSTAGE_SECRET: string;
}

type Verdict = {
  action: "forward" | "reject";
  to?: string;
  reason?: string;
  challenge_url?: string;
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
        body: parsed.text ?? parsed.html ?? "",
        ...auth,
      });
    } catch {
      // The gateway being down must not bounce someone's mail. Deliver it and
      // let the recipient's own provider apply its usual filtering.
      message.setReject("Postage is temporarily unavailable, please retry");
      return;
    }

    if (verdict.action === "forward" && verdict.to) {
      await message.forward(verdict.to);
      return;
    }

    if (verdict.reason === "unknown_inbox") {
      message.setReject("No such address at this domain");
      return;
    }

    message.setReject(
      verdict.challenge_url
        ? `Message held. Release it at ${verdict.challenge_url}`
        : "Message rejected"
    );
  },
};

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
  if (!response.ok) throw new Error(`Gateway returned ${response.status}`);
  return (await response.json()) as Verdict;
}

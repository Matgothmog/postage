import PostalMime from "postal-mime";

interface Env {
  POSTAGE_API_URL: string;
  POSTAGE_SECRET: string;
}

interface GatewayVerdict {
  status: "held" | "delivered" | "unknown_inbox";
  unlock_url?: string;
}

/// Cloudflare hands us every message arriving at the domain. We hand it to the
/// gateway, which decides whether it goes through or waits for postage.
export default {
  async email(message: ForwardableEmailMessage, env: Env): Promise<void> {
    const parsed = await PostalMime.parse(message.raw);

    const verdict = await callGateway(env, {
      from: message.from,
      to: message.to,
      subject: parsed.subject ?? "",
      body: parsed.text ?? parsed.html ?? "",
    });

    if (verdict.status === "unknown_inbox") {
      message.setReject("No such inbox at this domain");
      return;
    }

    if (verdict.status === "delivered") return;

    // Refusing with the unlock link keeps the whole flow inside SMTP, so no
    // outbound mail service is needed. The sender's provider surfaces this
    // reason back to them, which is where they pick the message up again.
    message.setReject(`Postage required. Release this message at ${verdict.unlock_url}`);
  },
};

async function callGateway(
  env: Env,
  payload: { from: string; to: string; subject: string; body: string }
): Promise<GatewayVerdict> {
  const response = await fetch(`${env.POSTAGE_API_URL}/api/mail/inbound`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "x-postage-secret": env.POSTAGE_SECRET,
    },
    body: JSON.stringify(payload),
  });

  if (response.status === 404) return { status: "unknown_inbox" };
  if (!response.ok) throw new Error(`Gateway returned ${response.status}`);
  return (await response.json()) as GatewayVerdict;
}

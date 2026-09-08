import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";

const workspace = mkdtempSync(join(tmpdir(), "postage-inbound-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.MAIL_WEBHOOK_SECRET = "secret";
process.env.APP_URL = "http://localhost";
process.env.MESSAGE_ID_SECRET = "x".repeat(32);
process.env.CLASSIFIER_PRIVATE_KEY = `0x${"11".repeat(32)}`;

const { CLASSIFY_PER_HANDLE_HOURLY, createInbox, reset } = await import("@/lib/db");
const { POST } = await import("./route");

/// No ANTHROPIC_API_KEY here, so every verdict comes from the header fallback —
/// which is exactly the state an attacker was able to manufacture by draining
/// the pool, and the state in which free delivery must not be available.
async function deliverTo(from: string, subject: string, authenticated: boolean) {
  const auth = authenticated
    ? { spf: "pass", dkim: "pass", dmarc: "pass" }
    : { spf: "none", dkim: "none", dmarc: "none" };
  const response = await POST(
    new Request("http://localhost/api/mail/inbound", {
      method: "POST",
      headers: { "Content-Type": "application/json", "x-postage-secret": "secret" },
      body: JSON.stringify({ from, to: "demo@usepostage.com", subject, body: "x", ...auth }),
    })
  );
  return (await response.json()) as { action?: string; reason?: string };
}

beforeEach(async () => {
  await reset();
  await createInbox("demo", "demo@example.com", `0x${"11".repeat(20)}`);
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

test("unauthenticated mail cannot drain the inbox's reading budget", async () => {
  for (let n = 0; n < CLASSIFY_PER_HANDLE_HOURLY + 10; n += 1) {
    await deliverTo(`forged${n}@x.com`, "hello", false);
  }

  const result = await deliverTo("noreply@stripe.com", "Your verification code is 4821", true);
  assert.equal(
    result.action,
    "forward",
    "a stranger's flood must not stop a real login code arriving"
  );
});

test("an authenticated stranger cannot buy free delivery with a transactional subject", async () => {
  const result = await deliverTo("spam@nowhere.example", "your one-time code", false);
  assert.equal(result.action, "hold");
});

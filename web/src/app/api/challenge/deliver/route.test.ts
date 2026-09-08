import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";

const workspace = mkdtempSync(join(tmpdir(), "postage-deliver-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.RESEND_API_KEY = "test";
process.env.MAIL_FROM = "Postage <hello@usepostage.com>";

const { claimChallenge, createChallenge, createInbox, grantPass, reset } = await import("@/lib/db");
const { POST } = await import("./route");

const now = () => Math.floor(Date.now() / 1000);

async function paste(token: string, body: string) {
  const response = await POST(
    new Request("http://localhost/api/challenge/deliver", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ token, subject: "hello", body }),
    })
  );
  return { status: response.status, body: (await response.json()) as { error?: string } };
}

beforeEach(async () => {
  await reset();
  await createInbox("demo", "demo@example.com", `0x${"11".repeat(20)}`);
  await createChallenge({
    token: "tok",
    handle: "demo",
    sender: "sender@x.com",
    message_id: `0x${"ab".repeat(32)}`,
    tier: "commercial",
    amount: "1",
    quote_json: "{}",
    held_until: now() + 900,
    created_at: now(),
  });
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

/// The budget is reported as a state, where "no state" means there was room.
/// Read as a boolean it inverts: the first paste is refused for being over
/// budget, and the route only starts working once the budget is actually gone.
test("a first paste is not refused for being over budget", async () => {
  await claimChallenge("tok", "paid");
  await grantPass("demo", "sender@x.com", "paid", 1);

  const { status, body } = await paste("tok", "here is what I wrote");
  assert.notEqual(
    status,
    429,
    `a sender with room left must not be told they have used their hour: ${body.error}`
  );
});

test("an unsettled challenge cannot be pasted through", async () => {
  const { status } = await paste("tok", "let me in");
  assert.equal(status, 403);
});

import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";
import { now } from "@/lib/time";
import { startStubModel } from "../../../../../test/model";

const workspace = mkdtempSync(join(tmpdir(), "postage-deliver-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.RESEND_API_KEY = "test";
process.env.MAIL_FROM = "Postage <hello@usepostage.com>";

/// Answers, rather than refusing. Everything this route does after the
/// classifier — spending the pass, relaying, giving the delivery back when the
/// relay will not take it — is behind a verdict the model actually returned.
/// Left to whatever the machine has, the route degrades and answers 503, and
/// every one of those steps sits untouched behind an error path.
const model = await startStubModel({
  tier: "commercial",
  confidence: 0.9,
  reasons: ["a stub answers for the model here"],
});

/// Resend's address is a constant inside the mailer, so answering for it here is
/// the only thing standing between a delivered test message and real mail.
/// Loopback is let through because the stub model is on it; anything else is a
/// test about to reach the network and is stopped rather than allowed.
const RESEND_ENDPOINT = "https://api.resend.com/emails";
const realFetch = globalThis.fetch;
const relayed: Array<{ to: string; subject: string; text: string }> = [];
let relayRefuses = false;

globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
  const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
  if (url.startsWith("http://127.0.0.1:")) return realFetch(input, init);
  if (url !== RESEND_ENDPOINT) throw new Error(`a test tried to reach ${url}`);
  if (relayRefuses) return new Response("mailbox unavailable", { status: 502 });
  relayed.push(JSON.parse(String(init?.body)) as (typeof relayed)[number]);
  return Response.json({ id: "stub" });
};

const { claimChallenge, createChallenge } = await import("@/lib/db/challenges");
const { reset } = await import("@/lib/db/client");
const { createInbox } = await import("@/lib/db/inboxes");
const { grantPass, hasLivePass } = await import("@/lib/db/passes");
const { POST } = await import("./route");

const HANDLE = "demo";
const SENDER = "sender@x.com";

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

/// A challenge already settled by paying, and the delivery that payment bought.
/// Everything this route does is downstream of a sender in exactly that state.
async function cleared() {
  await claimChallenge("tok", "paid");
  await grantPass(HANDLE, SENDER, "paid", 1);
}

beforeEach(async () => {
  relayed.length = 0;
  relayRefuses = false;
  await reset();
  await createInbox(HANDLE, "demo@example.com", `0x${"11".repeat(20)}`);
  await createChallenge({
    token: "tok",
    handle: HANDLE,
    sender: SENDER,
    message_id: `0x${"ab".repeat(32)}`,
    tier: "commercial",
    amount: "1",
    quote_json: "{}",
    held_until: now() + 900,
    created_at: now(),
  });
});

after(async () => {
  globalThis.fetch = realFetch;
  await model.close();
  rmSync(workspace, { recursive: true, force: true });
});

/// The budget is reported as a state, where "no state" means there was room.
/// Read as a boolean it inverts: the first paste is refused for being over
/// budget, and the route only starts working once the budget is actually gone.
test("a first paste is not refused for being over budget", async () => {
  await cleared();

  const { status, body } = await paste("tok", "here is what I wrote");
  assert.equal(
    status,
    200,
    `a sender with room left must not be told they have used their hour: ${body.error}`
  );
});

/// What the whole route is for, and the first thing here that puts a message on
/// the wire. Addressed to where the inbox forwards, not to the handle nobody
/// outside this system can deliver to.
test("a cleared sender's paste reaches the address the inbox forwards to", async () => {
  await cleared();

  await paste("tok", "here is what I wrote");
  assert.deepEqual(
    relayed.map((message) => message.to),
    ["demo@example.com"]
  );
});

/// One payment buys one delivery, so the pass has to be gone afterwards.
/// Without this the box under the challenge page is a relay anyone holding a
/// spent token may keep using.
test("a delivered paste spends the pass that authorised it", async () => {
  await cleared();

  await paste("tok", "here is what I wrote");
  assert.equal(await hasLivePass(HANDLE, SENDER), false);
});

/// The other half of that. A use taken for a delivery that never happened has to
/// come back, or an outage at the relay costs the sender the message they paid
/// for and there is nothing they can do about it.
test("a paste the relay will not take gives the delivery back", async () => {
  await cleared();
  relayRefuses = true;

  const { status } = await paste("tok", "here is what I wrote");
  assert.equal(status, 502);
  assert.equal(
    await hasLivePass(HANDLE, SENDER),
    true,
    "nothing was delivered, so the sender must still be holding what they paid for"
  );
});

test("an unsettled challenge cannot be pasted through", async () => {
  const { status } = await paste("tok", "let me in");
  assert.equal(status, 403);
});

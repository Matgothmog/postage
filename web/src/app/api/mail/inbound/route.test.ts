import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";
import { startStubChain } from "../../../../../test/chain";
import { startStubModel } from "../../../../../test/model";

const chain = await startStubChain();

const workspace = mkdtempSync(join(tmpdir(), "postage-inbound-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.MAIL_WEBHOOK_SECRET = "secret";
process.env.APP_URL = "http://localhost";
process.env.MESSAGE_ID_SECRET = "x".repeat(32);
process.env.CLASSIFIER_PRIVATE_KEY = `0x${"11".repeat(32)}`;

/// The model refuses every request, so every verdict here comes from the header
/// fallback — which is exactly the state an attacker was able to manufacture by
/// draining the pool, and the state in which free delivery must not be available.
///
/// Refused by this process rather than by a variable nobody set: the SDK reads
/// credentials off the machine as well as the environment, so on a developer's
/// laptop these tests would classify against the live model, prove none of what
/// their names claim, and still pass.
const model = await startStubModel();

const {
  CLASSIFY_PER_DOMAIN_HOURLY,
  CLASSIFY_PER_HANDLE_HOURLY,
  CLASSIFY_PER_SENDER_HOURLY,
  claimClassification,
} = await import("@/lib/db/classifications");
const { reset } = await import("@/lib/db/client");
const { createInbox } = await import("@/lib/db/inboxes");
const { POST } = await import("./route");

async function deliverTo(from: string, subject: string, authenticated: boolean) {
  const auth = authenticated
    ? { spf: "pass", dkim: "pass", dmarc: "pass" }
    : { spf: "none", dkim: "none", dmarc: "none" };
  return deliverWithAuth(from, subject, auth);
}

async function deliverWithAuth(
  from: string,
  subject: string,
  auth: { spf?: string; dkim?: string; dmarc?: string }
) {
  const response = await POST(
    new Request("http://localhost/api/mail/inbound", {
      method: "POST",
      headers: { "Content-Type": "application/json", "x-postage-secret": "secret" },
      body: JSON.stringify({ from, to: "demo@usepostage.com", subject, body: "x", ...auth }),
    })
  );
  return (await response.json()) as {
    action?: string;
    reason?: string;
    notice?: string | null;
    verdict?: { degraded: boolean };
  };
}

beforeEach(async () => {
  await reset();
  await createInbox("demo", "demo@example.com", `0x${"11".repeat(20)}`);
});

after(async () => {
  await chain.close();
  await model.close();
  rmSync(workspace, { recursive: true, force: true });
});

/// The premise the three tests below are about, asserted rather than assumed. A
/// stubbed model and a live one are indistinguishable in their results, so
/// nothing else in this file would notice if the classifier started answering
/// for real and the degraded state they exist to describe stopped happening.
test("the model cannot be reached from here, so a verdict is the header fallback", async () => {
  const asked = model.calls;

  const result = await deliverTo("someone@nowhere.example", "hello", true);

  assert.ok(model.calls > asked, "an authenticated first message must reach the classifier");
  assert.equal(result.verdict?.degraded, true);
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

/// A domain's allowance is only spendable by mail from that domain that the
/// receiving server could actually confirm - otherwise anyone could shut a real
/// sender out of an inbox by writing their name on enough envelopes.
test("an unauthenticated flood cannot spend the allowance of the domain it names", async () => {
  for (let n = 0; n < CLASSIFY_PER_DOMAIN_HOURLY + 5; n += 1) {
    await deliverTo(`forged${n}@stripe.com`, "hello", false);
  }

  const result = await deliverTo("noreply@stripe.com", "Your verification code is 4821", true);
  assert.equal(
    result.action,
    "forward",
    "forged mail claiming a domain must not consume what that domain is allowed"
  );
});

/// The bypass the three ceilings and the rule below them exist to close. Empty
/// the inbox's pool - which takes nothing but authenticated mail, spread over
/// enough domains - and every message after it was read from headers alone,
/// where a transactional-sounding subject is called important and important is
/// delivered free. One sender could buy that for everybody, for an hour.
test("a drained inbox pool does not buy the next sender free delivery", async () => {
  for (let taken = 0; taken < CLASSIFY_PER_HANDLE_HOURLY; taken += 1) {
    const domain = Math.floor(taken / CLASSIFY_PER_DOMAIN_HOURLY);
    await claimClassification("demo", `writer${taken}@drain${domain}.example`);
  }

  const result = await deliverTo("stranger@nowhere.example", "your one-time code", true);
  assert.equal(
    result.action,
    "hold",
    "emptying the pool must not be a way through the gate for whoever comes next"
  );
});

test("unauthenticated mail cannot buy free delivery with a transactional subject", async () => {
  const result = await deliverTo("spam@nowhere.example", "your one-time code", false);
  assert.equal(result.action, "hold");
});

/// The other half of the same rule, and the one that has teeth. A sender's own
/// hourly slice is theirs to spend, so spending it must not unlock the free
/// tier — otherwise emptying it deliberately is the way through the gate.
test("a sender who spends their own slice cannot then be delivered free", async () => {
  for (let n = 0; n < CLASSIFY_PER_SENDER_HOURLY; n += 1) {
    await deliverTo("greedy@nowhere.example", "hello", true);
  }

  const result = await deliverTo("greedy@nowhere.example", "your one-time code", true);
  assert.equal(
    result.action,
    "hold",
    "draining your own budget must not be a way to reach the tier nobody pays for"
  );
});

/// the worker collapses a disagreeing Authentication-Results
/// header to null, so a sender able to make copies of the header disagree
/// could turn Cloudflare's real `dkim=fail` into `dkim=null` and read as
/// authenticated anyway - the same value dkim already carries whenever a
/// sender never signs at all. If spf pass plus a merely-not-failing dkim were
/// still enough here, that hole would still be open, and so would the plainer
/// version of it: a sender can always just skip signing, no forgery required.
test("spf pass alone, without a verified dkim signature, does not authenticate the sender", async () => {
  const asked = model.calls;

  const result = await deliverWithAuth("someone@nowhere.example", "hello", {
    spf: "pass",
    dkim: "none",
    dmarc: "none",
  });

  assert.equal(
    model.calls,
    asked,
    "an unverified dkim must not unlock the classifier budget an authenticated sender gets"
  );
  assert.equal(result.action, "hold");
  assert.equal(
    result.notice,
    null,
    "a sender we can't confirm must not be mailed back, to avoid backscatter to whoever's name they wrote down"
  );
});

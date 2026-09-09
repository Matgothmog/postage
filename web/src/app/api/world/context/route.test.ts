import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";
import { now } from "@/lib/time";

// Signing itself is pure local cryptography and touches no network, but the
// route reads the challenge the context is being signed for, so the database
// needs a home of its own before the module under test is imported — the same
// pattern every db-touching test file in this tree follows. The three World
// Developer Portal values `required()` demands are set per test rather than
// once here, since the misconfiguration test below depends on unsetting one.
const workspace = mkdtempSync(join(tmpdir(), "postage-world-context-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.WORLD_ACTION = "send-free";
process.env.WORLD_RP_ID = "app_test_rp_id";

const SIGNING_KEY = `0x${"11".repeat(32)}`;

const { claimChallenge, createChallenge } = await import("@/lib/db/challenges");
const { reset } = await import("@/lib/db/client");
const { MAX_LIVE_CONTEXTS_PER_TOKEN, consumeIssuedContext } = await import(
  "@/lib/db/issued-contexts"
);
const { RP_CONTEXT_TTL_SECONDS } = await import("@/lib/rp-context");
const { POST } = await import("./route");

const HANDLE = "demo";
const SENDER = "sender@x.com";
const TOKEN = "tok";

interface ContextResponse {
  rp_id?: string;
  nonce?: string;
  created_at?: number;
  expires_at?: number;
  signature?: string;
  action?: string;
  error?: string;
}

async function context(body: unknown, raw?: string) {
  const response = await POST(
    new Request("http://localhost/api/world/context", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: raw ?? JSON.stringify(body),
    })
  );
  return { status: response.status, body: (await response.json()) as ContextResponse };
}

async function seed(token: string, tier: string): Promise<void> {
  await createChallenge({
    token,
    handle: HANDLE,
    sender: SENDER,
    message_id: `0x${"ab".repeat(32)}`,
    tier,
    amount: "1",
    quote_json: "{}",
    held_until: now() + 900,
    created_at: now(),
  });
}

beforeEach(async () => {
  process.env.WORLD_RP_SIGNING_KEY = SIGNING_KEY;
  await reset();
  await seed(TOKEN, "commercial");
});

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

test("signs an rp_context for a sender holding an open challenge", async () => {
  const { status, body } = await context({ token: TOKEN });

  assert.equal(status, 200, `expected success: ${body.error}`);
  assert.equal(body.rp_id, "app_test_rp_id");
  // 32-byte nonce and 65-byte recoverable signature, both hex-encoded — the
  // exact bytes are random per call, so only their shape is checked here.
  assert.match(body.nonce ?? "", /^0x[0-9a-f]{64}$/);
  assert.match(body.signature ?? "", /^0x[0-9a-f]{130}$/);
});

test("returns the action it signed into the context, so the browser reads it from here rather than its own env var", async () => {
  const { body } = await context({ token: TOKEN });

  assert.equal(body.action, process.env.WORLD_ACTION);
});

test("signs a window the client's own polling window can fit inside", async () => {
  const { body } = await context({ token: TOKEN });

  assert.equal((body.expires_at ?? 0) - (body.created_at ?? 0), RP_CONTEXT_TTL_SECONDS);
});

test("binds the nonce it signed to the challenge it signed it for", async () => {
  const { body } = await context({ token: TOKEN });

  assert.equal(await consumeIssuedContext("another-token", body.nonce ?? ""), false);
  assert.equal(await consumeIssuedContext(TOKEN, body.nonce ?? ""), true);
});

test("refuses a caller presenting no challenge token at all", async () => {
  const { status, body } = await context({});

  assert.equal(status, 400);
  assert.match(body.error ?? "", /token is required/i);
  assert.equal(body.signature, undefined);
});

test("refuses a body that is not JSON", async () => {
  const { status, body } = await context(undefined, "not json");

  assert.equal(status, 400);
  assert.equal(body.signature, undefined);
});

test("refuses a challenge token nobody was ever issued", async () => {
  const { status, body } = await context({ token: "bogus" });

  assert.equal(status, 404);
  assert.equal(body.signature, undefined);
});

test("refuses a challenge that has already been settled", async () => {
  await claimChallenge(TOKEN, "human");

  const { status, body } = await context({ token: TOKEN });

  assert.equal(status, 409);
  assert.equal(body.signature, undefined);
});

test("refuses a challenge no proof of personhood could ever release", async () => {
  await seed("danger", "dangerous");

  const { status, body } = await context({ token: "danger" });

  assert.equal(status, 403);
  assert.equal(body.signature, undefined);
});

test("bounds how many contexts one challenge token can mint", async () => {
  for (let issued = 0; issued < MAX_LIVE_CONTEXTS_PER_TOKEN; issued += 1) {
    const { status } = await context({ token: TOKEN });
    assert.equal(status, 200, `context ${issued} should have been signed`);
  }

  const { status, body } = await context({ token: TOKEN });

  assert.equal(status, 429);
  assert.equal(body.signature, undefined);
});

/// Thrown, not returned as a body: Next turns an unhandled route error into a
/// bare 500, which is what a misconfigured deployment should tell the world.
/// Naming the missing variable in a response would hand an anonymous caller a
/// map of the server's secrets.
test("fails loudly rather than signing without a configured key", async () => {
  delete process.env.WORLD_RP_SIGNING_KEY;

  await assert.rejects(
    () =>
      POST(
        new Request("http://localhost/api/world/context", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ token: TOKEN }),
        })
      ),
    /WORLD_RP_SIGNING_KEY is not set/
  );
});

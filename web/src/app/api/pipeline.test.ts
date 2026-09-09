import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { HUMAN_REGISTRY } from "@/lib/contracts";
import { startStubModel } from "../../../test/model";

/// Walks the whole product loop through its own public entry points, the way
/// the worker and a sender's browser actually drive it — not a mailbox.
///
/// `POST /api/mail/inbound` is exactly what the Cloudflare Worker's SMTP
/// session calls for a real inbound message (`worker/src/index.ts`), and the
/// challenge URL it hands back is minted synchronously by `issueChallenge`
/// (`mail/inbound/challenge.ts`) — nothing about reaching `/c/<token>` depends
/// on a mail transport existing. A real message from
/// matgothmog.angelux@gmail.com cannot be reproduced here: the worker's SMTP
/// session and Cloudflare Email Routing on the deployed usepostage.com are the
/// only things that have ever put a message in front of this route from a real
/// mailbox, and neither is reachable from this workspace, so this test never
/// claims to exercise them.
///
/// What is real: the gateway route, the challenge/verify/gate wiring, and the
/// database. What is stubbed, each a loopback server rather than a monkey-
/// patched `fetch`: the chain (a custom RPC stub set up below, not
/// `test/chain.ts`'s — see the note above it), the classifier model
/// (`startStubModel`), and the mail worker's own `/release` endpoint set up
/// below — the same shape `hold.test.ts` and
/// `challenge/deliver/route.test.ts` use for their own outbound legs. Running
/// under `IDENTITY_MODE=mock` (the unset default, set explicitly here so this
/// file's mode never depends on a value nobody set) means the World ID leg
/// itself is stubbed too: `verifyWithWorld` is never called, so no proof, no
/// IDKit and no World endpoint are exercised by this test at all.
const workspace = mkdtempSync(join(tmpdir(), "postage-pipeline-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.MAIL_WEBHOOK_SECRET = "secret";
process.env.APP_URL = "http://localhost";
process.env.MESSAGE_ID_SECRET = "x".repeat(32);
process.env.CLASSIFIER_PRIVATE_KEY = `0x${"11".repeat(32)}`;
process.env.IDENTITY_MODE = "mock";

/// A stand-in for the chain, distinguishing calls by which contract they
/// target rather than answering every `eth_call` alike — this pipeline reads
/// two: `effectiveFloor` on the escrow (`issueChallenge`, any value works,
/// it is a `uint256`) and `humanUntil` on the human registry
/// (`recordPersonhood`, a `uint40` — decoding a value wider than 40 bits
/// throws, which `test/chain.ts`'s single-answer stub cannot avoid). The
/// registry is answered with the largest value a `uint40` holds so every run
/// here takes `recordPersonhood`'s "already fresh" early return and never
/// reaches the attest-and-wait flow behind it, matching the fixture
/// `world/verify/route.test.ts` uses for the same reason.
const HUMAN_UNTIL_MAX = `0x${"00".repeat(27)}${"ff".repeat(5)}`;
const FLOOR = `0x${(10n ** 16n).toString(16).padStart(64, "0")}`;
const rpc = createServer((request, response) => {
  let body = "";
  request.on("data", (chunk: Buffer) => (body += chunk));
  request.on("end", () => {
    const { id, method, params } = JSON.parse(body) as {
      id: number;
      method: string;
      params?: [{ to?: string }];
    };
    const to = params?.[0]?.to?.toLowerCase();
    const result = method === "eth_call" && to === HUMAN_REGISTRY ? HUMAN_UNTIL_MAX : FLOOR;
    response.writeHead(200, { "Content-Type": "application/json" });
    response.end(JSON.stringify({ jsonrpc: "2.0", id, result }));
  });
});
await new Promise<void>((listening) => rpc.listen(0, "127.0.0.1", listening));
process.env.ARC_RPC_URL = `http://127.0.0.1:${(rpc.address() as AddressInfo).port}`;

/// Held mail must reach a human tier for anyone to have something to clear —
/// "important" is delivered free and "dangerous" is never held (see the doc
/// comment above `POST` in `mail/inbound/route.ts`) — so the tier is pinned
/// here rather than left to whatever the header fallback would have picked
/// for this particular subject line.
const model = await startStubModel({
  tier: "commercial",
  confidence: 0.9,
  reasons: ["a stub answers for the model here"],
});

/// Stands in for the mail worker's own `/release` endpoint — the one leg of
/// this pipeline nothing in this workspace can drive for real, since the
/// worker holds the message bytes and only a live Cloudflare deployment can
/// answer this call. Records exactly what `releaseHeldMessage`
/// (`@/lib/hold.ts`) sent it, so the test can assert the payload rather than
/// merely that some request arrived.
const releases: Array<{ token?: string; to?: string }> = [];
const worker = createServer((request, response) => {
  let body = "";
  request.on("data", (chunk: Buffer) => (body += chunk));
  request.on("end", () => {
    releases.push(JSON.parse(body || "{}") as { token?: string; to?: string });
    response.writeHead(200, { "Content-Type": "application/json" }).end("{}");
  });
});
// Listened for at the top level rather than in a `before()` hook: an async
// hook that resolves through an I/O callback races `beforeEach` on this
// runtime — top-level `await` is what `hold.test.ts` uses for the same
// reason.
await new Promise<void>((listening) => worker.listen(0, "127.0.0.1", listening));
process.env.MAIL_WORKER_URL = `http://127.0.0.1:${(worker.address() as AddressInfo).port}`;

const { challengeByToken } = await import("@/lib/db/challenges");
const { reset } = await import("@/lib/db/client");
const { createInbox } = await import("@/lib/db/inboxes");
const { hasLivePass } = await import("@/lib/db/passes");
const { POST: inbound } = await import("./mail/inbound/route");
const { POST: verify } = await import("./world/verify/route");

const HANDLE = "demo";
const SENDER = "matgothmog.angelux@gmail.com";
const DESTINATION = "demo-owner@example.com";

await reset();
await createInbox(HANDLE, DESTINATION, `0x${"11".repeat(20)}`);

after(async () => {
  worker.close();
  // The RPC client keeps its sockets alive, so a plain close would wait for a
  // pool that is not going to disconnect itself and hang the suite — the same
  // reason `test/chain.ts`'s `close()` calls this first.
  rpc.closeAllConnections();
  rpc.close();
  await model.close();
  rmSync(workspace, { recursive: true, force: true });
});

test("a held mail's challenge clears on the mock path and releases the message it was holding", async () => {
  // Stage 1 — a mail addressed to a claimed handle, from an authenticated
  // sender the gateway has never seen before, must be held rather than
  // forwarded or rejected. Asserted on the decision itself, not just a 200:
  // any of "forward", "hold" or "reject" answers with a 200 (`route.ts`
  // never uses a status code for a verdict), so only `action` says which one
  // this was.
  const inboundResponse = await inbound(
    new Request("http://localhost/api/mail/inbound", {
      method: "POST",
      headers: { "Content-Type": "application/json", "x-postage-secret": "secret" },
      body: JSON.stringify({
        from: SENDER,
        to: `${HANDLE}@usepostage.com`,
        subject: "Quick question about the project",
        body: "Hey, do you have a minute to talk this week?",
        spf: "pass",
        dkim: "pass",
        dmarc: "pass",
      }),
    })
  );
  const inboundBody = (await inboundResponse.json()) as {
    action?: string;
    token?: string;
    challenge_url?: string;
  };
  assert.equal(
    inboundBody.action,
    "hold",
    `the message must be held, not forwarded or rejected: ${JSON.stringify(inboundBody)}`
  );

  // Stage 2 — the response's `challenge_url` is usable with no mailbox read:
  // it is the same URL the held-mail notice would have mailed the sender
  // (`route.ts`'s `context.challenge_url`, echoed straight from
  // `issueChallenge`'s synchronous return value), and the token inside it
  // resolves to a real row rather than merely looking like one.
  assert.ok(inboundBody.challenge_url, "a held message must carry a challenge URL");
  const token = new URL(inboundBody.challenge_url as string).pathname.replace(/^\/c\//, "");
  assert.equal(token, inboundBody.token, "the challenge URL and the wire token must name the same challenge");

  const held = await challengeByToken(token);
  assert.ok(held, "the token inside the challenge URL must resolve to a real challenge row");
  assert.equal(held?.handle, HANDLE);
  assert.equal(held?.sender, SENDER.toLowerCase());
  assert.ok((held?.held_until ?? 0) > 0, "the message this challenge names must actually be on hold");

  // Stage 3 — clearing the human check on the mock path takes no World ID
  // proof at all: `identityMode() !== "live"` derives the nullifier from the
  // sender address instead of forwarding anything to World
  // (`world/verify/route.ts`'s `mockNullifier`).
  const verifyResponse = await verify(
    new Request("http://localhost/api/world/verify", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ token }),
    })
  );
  const verifyBody = (await verifyResponse.json()) as { error?: string; identity?: string };
  assert.equal(verifyResponse.status, 200, `expected the mock check to clear: ${verifyBody.error}`);
  assert.match(verifyBody.identity ?? "", /^0x[0-9a-fA-F]{40}$/);
  assert.equal(
    await hasLivePass(HANDLE, SENDER),
    true,
    "clearing the check on the human lane must grant a live pass"
  );

  // Stage 4 — the gate opened, and that must have actually released the
  // message this challenge was holding: exactly one call reached the mail
  // worker's `/release` endpoint, naming this exact token — the only way the
  // worker knows which held bytes to send — and this inbox's real
  // destination, not merely that some call happened.
  assert.equal(releases.length, 1, "exactly one release must have reached the mail worker");
  assert.equal(releases[0]?.token, token, "the release must name the message that was actually held");
  assert.equal(
    releases[0]?.to,
    DESTINATION,
    "the release must be addressed to the claimed inbox's real destination"
  );

  // Stage 5 — the challenge is recorded as delivered, so the hold cannot be
  // spent a second time.
  const settled = await challengeByToken(token);
  assert.ok(settled?.delivered_at, "the challenge must be recorded as delivered");
  assert.equal(settled?.held_until, null, "the hold must be spent, not merely answered");
});

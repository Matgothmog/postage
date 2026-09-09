import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";
import { hashSignal } from "@worldcoin/idkit/hashing";
import { keccak256, stringToBytes } from "viem";
import { now } from "@/lib/time";

// This route reads a challenge from the database and, on the happy path,
// reads the chain before ever reaching World — both need a controlled home
// before the module under test is imported, the same pattern every
// db-touching test file in this tree follows.
const workspace = mkdtempSync(join(tmpdir(), "postage-world-verify-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.WORLD_RP_ID = "app_test_rp_id";
process.env.ARC_RPC_URL = "http://127.0.0.1:9/rpc";

const WORLD_VERIFY_URL = "https://developer.world.org/api/v4/verify/app_test_rp_id";
const RPC_URL = process.env.ARC_RPC_URL;

/// What `recordPersonhood` reads before writing anything. Answered with the
/// largest value a `uint40` can hold, so every test that reaches this point
/// takes the "already fresh" early return and never touches the attest-and-
/// wait flow behind it — the World call under test stays the only thing on
/// the wire that this file's tests are actually about.
const HUMAN_UNTIL_MAX = `0x${"00".repeat(27)}${"ff".repeat(5)}`;

const realFetch = globalThis.fetch;
let worldHandler: () => Response | Promise<Response> = () => defaultWorldResponse();
let worldCalls = 0;
// Toggled on for the one test that needs the RPC leg to fail the way a real
// transport does: `viem`'s `HttpRequestError` builds its `message` out of the
// full request URL, which is exactly what must never reach a response body.
let rpcShouldFail = false;

function defaultWorldResponse(): Response {
  return Response.json({
    success: true,
    results: [{ identifier: "selfie", success: true, nullifier: "world-nullifier" }],
  });
}

globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
  const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
  if (url === WORLD_VERIFY_URL) {
    worldCalls += 1;
    return worldHandler();
  }
  if (url === RPC_URL) {
    if (rpcShouldFail) throw new TypeError("fetch failed");
    const { id, method } = JSON.parse(String(init?.body)) as { id: number; method: string };
    if (method === "eth_call") return Response.json({ jsonrpc: "2.0", id, result: HUMAN_UNTIL_MAX });
  }
  throw new Error(`a test tried to reach ${url}`);
};

const { createChallenge } = await import("@/lib/db/challenges");
const { db, reset } = await import("@/lib/db/client");
const { recordIssuedContext } = await import("@/lib/db/issued-contexts");
const { rebindsOf, senderHoldingNullifier } = await import("@/lib/db/nullifiers");
const { addPaidUse, hasLivePass } = await import("@/lib/db/passes");
const { SCHEMA } = await import("@/lib/db/schema");
const { POST } = await import("./route");

/// What the ledger is keyed on once the route has been through the stubbed
/// World response above: `world-nullifier` is not already 32 bytes of hex, so
/// the route hashes it the same way this does.
const NULLIFIER_HASH = keccak256(stringToBytes("world-nullifier"));

const HANDLE = "demo";
const SENDER = "sender@x.com";
const OTHER_SENDER = "attacker@x.com";
const TOKEN = "tok";
const OTHER_TOKEN = "tok-two";

/// The exact shape `ChallengeActions.verifyHuman` will send once it is wired
/// up to IDKit: a `responses` array naming the Selfie Check credential by
/// `identifier`, per the v3.0 legacy format described in
/// docs/world-feedback.md. The route inspects only `identifier` on each
/// entry; the rest is forwarded to World untouched.
/// `signal_hash` is the binding: World App hashes the signal the client asked
/// for into the proof, and the route recomputes it from the token in the same
/// request. A proof built for one token therefore does not fit another.
function proofFor(token: string) {
  return {
    protocol_version: "3.0",
    // Derived from the token, like `signal_hash` below, so two proofs made
    // for two different challenges in the same test never collide on the
    // ledger's nonce primary key.
    nonce: `nonce-${token}`,
    responses: [
      {
        identifier: "selfie",
        signal_hash: hashSignal(token),
        proof: "0xproof",
        merkle_root: "0xroot",
        nullifier: "world-nullifier",
      },
    ],
  };
}

const WELL_FORMED_PROOF = proofFor(TOKEN);

interface VerifyResponse {
  error?: string;
  identity?: string;
}

async function verify(body: unknown) {
  const response = await POST(
    new Request("http://localhost/api/world/verify", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    })
  );
  return { status: response.status, body: (await response.json()) as VerifyResponse };
}

/// Simulates the `/api/world/context` call every live-mode proof is
/// preceded by: a signed context this route can later spend exactly once.
async function issueContext(token: string, nonce: string): Promise<void> {
  await recordIssuedContext(token, nonce, now(), now() + 300);
}

/// Puts the nullifier in the first sender's hands the honest way, so a test
/// about what must *not* move it starts from a binding worth defending.
async function bindToFirstSender(): Promise<void> {
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });
  assert.equal(status, 200, `the honest verification this test builds on must succeed: ${body.error}`);
}

/// A second sender with a challenge and a signing context of their own — the
/// attacker's half of every takeover test below, set up legitimately so that
/// only the proof is ever in question.
async function challengeForOtherSender(): Promise<ReturnType<typeof proofFor>> {
  await createTestChallenge(OTHER_TOKEN, OTHER_SENDER);
  const proof = proofFor(OTHER_TOKEN);
  await issueContext(OTHER_TOKEN, proof.nonce);
  return proof;
}

async function createTestChallenge(token: string, sender: string): Promise<void> {
  await createChallenge({
    token,
    handle: HANDLE,
    sender,
    message_id: `0x${"ab".repeat(32)}`,
    tier: "commercial",
    amount: "1",
    quote_json: "{}",
    held_until: now() + 900,
    created_at: now(),
  });
}

beforeEach(async () => {
  worldHandler = () => defaultWorldResponse();
  worldCalls = 0;
  rpcShouldFail = false;
  await reset();
  await createChallenge({
    token: TOKEN,
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

after(() => {
  globalThis.fetch = realFetch;
  rmSync(workspace, { recursive: true, force: true });
});

test("live mode accepts a well-formed proof and forwards it to World", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 200, `expected success: ${body.error}`);
  assert.match(body.identity ?? "", /^0x[0-9a-fA-F]{40}$/);
});

test("live mode rejects a request with no proof, before ever calling World", async () => {
  process.env.IDENTITY_MODE = "live";

  const { status, body } = await verify({ token: TOKEN });

  assert.equal(status, 400);
  assert.match(body.error ?? "", /proof is required/i);
});

test("live mode rejects a malformed proof, before ever calling World", async () => {
  process.env.IDENTITY_MODE = "live";

  const { status, body } = await verify({ token: TOKEN, proof: { not: "a proof" } });

  assert.equal(status, 400);
  assert.match(body.error ?? "", /malformed/i);
});

test("mock mode still verifies with no proof at all", async () => {
  process.env.IDENTITY_MODE = "mock";

  const { status, body } = await verify({ token: TOKEN });

  assert.equal(status, 200, `expected success: ${body.error}`);
  assert.match(body.identity ?? "", /^0x[0-9a-fA-F]{40}$/);
});

test("a network failure reaching World is a gateway error, not a rejected proof", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  worldHandler = () => {
    throw new Error("simulated network failure");
  };

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 502);
  assert.match(body.error ?? "", /could not reach world id/i);
});

test("World's own server error is a gateway error, not a rejected proof", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  worldHandler = () => Response.json({ detail: "internal error, try again later" }, { status: 503 });

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 502);
  // A 5xx here is World's own failure, not a verdict on the proof, so the
  // taxonomy in the comment above `verifyWithWorld` demands 502 rather than
  // the 400 the old `!response.ok` check alone would have produced — and
  // World's own internal detail is never World's to hand back either.
  assert.equal(body.error?.includes("internal error"), false);
});

test("a proof World rejects outright is a bad request, not a gateway error", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  worldHandler = () => Response.json({ success: false, detail: "invalid rp signature" }, { status: 400 });

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 400);
  assert.match(body.error ?? "", /invalid rp signature/);
});

test("a 2xx response with no Selfie Check credential is a bad request", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  worldHandler = () =>
    Response.json({ success: true, results: [{ identifier: "orb", success: true, nullifier: "x" }] });

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 400);
  assert.match(body.error ?? "", /selfie check/i);
});

test("a proof made for another challenge is refused, before ever calling World", async () => {
  process.env.IDENTITY_MODE = "live";

  const { status, body } = await verify({ token: TOKEN, proof: proofFor(OTHER_TOKEN) });

  assert.equal(status, 400);
  assert.match(body.error ?? "", /not made for this challenge/i);
  assert.equal(worldCalls, 0);
});

test("a proof bound to nothing at all is refused, before ever calling World", async () => {
  process.env.IDENTITY_MODE = "live";
  const unbound = {
    protocol_version: "3.0",
    nonce: "test-nonce",
    responses: [
      { identifier: "selfie", proof: "0xproof", merkle_root: "0xroot", nullifier: "world-nullifier" },
    ],
  };

  const { status, body } = await verify({ token: TOKEN, proof: unbound });

  assert.equal(status, 400);
  assert.match(body.error ?? "", /not made for this challenge/i);
  assert.equal(worldCalls, 0);
});

test("a proof with no Selfie Check credential at all is refused, before ever calling World", async () => {
  process.env.IDENTITY_MODE = "live";
  const noSelfie = {
    protocol_version: "3.0",
    nonce: "nonce-no-selfie",
    responses: [{ identifier: "orb", proof: "0xproof", merkle_root: "0xroot", nullifier: "world-nullifier" }],
  };

  const { status, body } = await verify({ token: TOKEN, proof: noSelfie });

  assert.equal(status, 400);
  assert.match(body.error ?? "", /exactly one selfie check credential/i);
  assert.equal(worldCalls, 0);
});

/// The shape a stub-plus-harvested-proof attack would take: one entry
/// carrying this challenge's own `signal_hash` so the binding check finds
/// something to approve of, riding alongside a second "selfie" entry that
/// could just as well be an unrelated, already-verified proof. Without
/// requiring exactly one, only the first entry gets checked here while
/// `verifyWithWorld` reads World's response independently and could come back
/// naming either entry's nullifier — the checked credential and the consumed
/// one would no longer be provably the same. Refusing at two closes that off
/// before World is ever asked, rather than trusting the two arrays to agree.
test("a proof carrying two Selfie Check credentials is refused, before ever calling World", async () => {
  process.env.IDENTITY_MODE = "live";
  const twoSelfies = {
    protocol_version: "3.0",
    nonce: "nonce-two-selfies",
    responses: [
      {
        identifier: "selfie",
        signal_hash: hashSignal(TOKEN),
        proof: "0xproof1",
        merkle_root: "0xroot1",
        nullifier: "n1",
      },
      {
        identifier: "selfie",
        signal_hash: hashSignal(TOKEN),
        proof: "0xproof2",
        merkle_root: "0xroot2",
        nullifier: "n2",
      },
    ],
  };

  const { status, body } = await verify({ token: TOKEN, proof: twoSelfies });

  assert.equal(status, 400);
  assert.match(body.error ?? "", /exactly one selfie check credential/i);
  assert.equal(worldCalls, 0);
});

test("a second sender presenting one person's nullifier takes the binding over", async () => {
  process.env.IDENTITY_MODE = "live";
  await bindToFirstSender();
  const otherProof = await challengeForOtherSender();

  // Properly signed for the second sender's own challenge, so the signal
  // binding has nothing to say about it. Under the old rule this was refused
  // outright, which made a nullifier spent on somebody else's challenge token
  // lost for good; the person it names moves it back by doing exactly this.
  const moved = await verify({ token: OTHER_TOKEN, proof: otherProof });

  assert.equal(moved.status, 200, `expected the binding to move rather than be refused: ${moved.body.error}`);
  assert.equal(await senderHoldingNullifier(NULLIFIER_HASH), OTHER_SENDER);
});

test("a takeover releases the sender who held the nullifier before it", async () => {
  process.env.IDENTITY_MODE = "live";
  await bindToFirstSender();
  const otherProof = await challengeForOtherSender();

  await verify({ token: OTHER_TOKEN, proof: otherProof });

  // The whole of what keeps farming shut: the ledger names one sender, so
  // taking the lane onto a second address is the same write that takes it off
  // the first, and one person cannot stand on two at once.
  assert.notEqual(await senderHoldingNullifier(NULLIFIER_HASH), SENDER);
});

test("a takeover is recorded, so the move is visible after the fact", async () => {
  process.env.IDENTITY_MODE = "live";
  await bindToFirstSender();
  const otherProof = await challengeForOtherSender();

  await verify({ token: OTHER_TOKEN, proof: otherProof });

  assert.deepEqual(
    (await rebindsOf(NULLIFIER_HASH)).map(({ from_sender, to_sender }) => [from_sender, to_sender]),
    [[SENDER, OTHER_SENDER]]
  );
});

test("a takeover does not tell the new sender who held the nullifier before them", async () => {
  process.env.IDENTITY_MODE = "live";
  await bindToFirstSender();
  const otherProof = await challengeForOtherSender();

  const moved = await verify({ token: OTHER_TOKEN, proof: otherProof });

  // Whoever the lane came off is somebody else's business, and on the path
  // worth worrying about the one asking is the attacker.
  assert.equal(JSON.stringify(moved.body).includes(SENDER), false);
});

test("a takeover revokes the earned pass the released sender was standing on", async () => {
  process.env.IDENTITY_MODE = "live";
  await bindToFirstSender();
  assert.equal(await hasLivePass(HANDLE, SENDER), true, "test setup: clearing the challenge must have earned a pass");
  const otherProof = await challengeForOtherSender();

  await verify({ token: OTHER_TOKEN, proof: otherProof });

  // This is the whole point of the charge: releasing the nullifier binding
  // must also free the lane it was standing on, or a farmer can keep
  // standing in it after losing the nullifier that was supposed to account
  // for it.
  assert.equal(await hasLivePass(HANDLE, SENDER), false);
});

test("a takeover never revokes a paid pass on the released sender", async () => {
  process.env.IDENTITY_MODE = "live";
  // Paid first, so the balance is sitting on the row before the human proof
  // lands on top of it — the same order `grantPass`'s own `ON CONFLICT`
  // guards against losing (see `EARNED_PASS_CONDITION` in `db/passes.ts`):
  // clearing this challenge afterwards folds a "human" grant onto a row that
  // is still carrying an unspent, paid `uses_left`, and that count must
  // survive both the merge and the takeover below.
  await addPaidUse(HANDLE, SENDER);
  await bindToFirstSender();
  const otherProof = await challengeForOtherSender();

  await verify({ token: OTHER_TOKEN, proof: otherProof });

  // A delivery bought with money is never in reach of a nullifier moving to
  // someone else, however that money and that proof happened to land on the
  // same (handle, sender) row.
  assert.equal(await hasLivePass(HANDLE, SENDER), true);
});

test("a proof World rejects cannot move a binding", async () => {
  process.env.IDENTITY_MODE = "live";
  await bindToFirstSender();
  const otherProof = await challengeForOtherSender();
  worldHandler = () => Response.json({ success: false, detail: "invalid rp signature" }, { status: 400 });

  const { status } = await verify({ token: OTHER_TOKEN, proof: otherProof });

  assert.equal(status, 400);
  assert.equal(await senderHoldingNullifier(NULLIFIER_HASH), SENDER);
  assert.deepEqual(await rebindsOf(NULLIFIER_HASH), []);
});

test("a proof made for another challenge cannot move a binding", async () => {
  process.env.IDENTITY_MODE = "live";
  await bindToFirstSender();
  await challengeForOtherSender();

  // The signal binding, presented with somebody else's own valid proof bytes:
  // refused before World is asked, and the ledger must not have moved either.
  const { status } = await verify({ token: OTHER_TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 400);
  assert.equal(await senderHoldingNullifier(NULLIFIER_HASH), SENDER);
  assert.deepEqual(await rebindsOf(NULLIFIER_HASH), []);
});

test("a proof carrying a context this server never issued cannot move a binding", async () => {
  process.env.IDENTITY_MODE = "live";
  await bindToFirstSender();
  await createTestChallenge(OTHER_TOKEN, OTHER_SENDER);
  // No issueContext for the second sender: the single-use nonce gate is the
  // control under test here, and it has to hold the ledger still on its own.
  const { status } = await verify({ token: OTHER_TOKEN, proof: proofFor(OTHER_TOKEN) });

  assert.equal(status, 400);
  assert.equal(await senderHoldingNullifier(NULLIFIER_HASH), SENDER);
  assert.deepEqual(await rebindsOf(NULLIFIER_HASH), []);
});

test("two senders verifying at the same moment leave one holder and one recorded move", async () => {
  process.env.IDENTITY_MODE = "live";
  await createTestChallenge(OTHER_TOKEN, OTHER_SENDER);
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  const otherProof = proofFor(OTHER_TOKEN);
  await issueContext(OTHER_TOKEN, otherProof.nonce);

  const [first, second] = await Promise.all([
    verify({ token: TOKEN, proof: WELL_FORMED_PROOF }),
    verify({ token: OTHER_TOKEN, proof: otherProof }),
  ]);

  assert.equal(first.status, 200, `expected success: ${first.body.error}`);
  assert.equal(second.status, 200, `expected success: ${second.body.error}`);
  // Whichever landed second holds it, and the trail says so: one hop, ending
  // where the ledger now points. Two claims cannot both write themselves in.
  const hops = await rebindsOf(NULLIFIER_HASH);
  assert.equal(hops.length, 1);
  assert.equal(hops[0].to_sender, await senderHoldingNullifier(NULLIFIER_HASH));
});

test("the same person clearing a second challenge of their own moves nothing", async () => {
  process.env.IDENTITY_MODE = "live";
  await createTestChallenge(OTHER_TOKEN, SENDER);
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  const otherProof = proofFor(OTHER_TOKEN);
  await issueContext(OTHER_TOKEN, otherProof.nonce);

  const first = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });
  const second = await verify({ token: OTHER_TOKEN, proof: otherProof });

  assert.equal(first.status, 200, `expected success: ${first.body.error}`);
  assert.equal(second.status, 200, `a pass lasts 15 minutes, so re-proving is the design: ${second.body.error}`);
  assert.deepEqual(await rebindsOf(NULLIFIER_HASH), []);
});

test("a proof carrying a context this server never issued is refused, before ever calling World", async () => {
  process.env.IDENTITY_MODE = "live";
  // No issueContext call: WELL_FORMED_PROOF.nonce was never recorded, which
  // is what a proof harvested from someone else's flow looks like here.

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 400);
  assert.match(body.error ?? "", /never issued for this challenge/i);
  assert.equal(worldCalls, 0);
});

test("a proof whose signed context was already spent is refused on the second presentation", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);

  const first = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });
  assert.equal(first.status, 200, `expected the first verification to succeed: ${first.body.error}`);

  // Same token, same proof, same nonce: nothing left in the ledger to spend a
  // second time, so this must be refused without ever reaching World again.
  worldCalls = 0;
  const replayed = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(replayed.status, 400);
  assert.match(replayed.body.error ?? "", /already used/i);
  assert.equal(worldCalls, 0);
});

test("an RPC failure recording personhood does not leak the RPC endpoint", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  rpcShouldFail = true;

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 502);
  // A real transport failure here is a viem `HttpRequestError`, whose own
  // `message` embeds the full request URL — which is `ARC_RPC_URL`, an
  // operator-configured endpoint that may carry a key in its path or query.
  assert.equal(body.error?.includes(RPC_URL), false);
  assert.equal(body.error?.includes("127.0.0.1"), false);
  assert.match(body.error ?? "", /could not record the attestation/i);
});

test("a missing rp_id is our misconfiguration, not the sender's, and exposes nothing", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  const originalRpId = process.env.WORLD_RP_ID;
  delete process.env.WORLD_RP_ID;

  try {
    // Thrown, not returned as a body: Next turns an unhandled route error
    // into a bare 500, which is what a misconfigured deployment should tell
    // the world — naming the missing variable in a response would hand an
    // anonymous caller a map of the server's secrets. Same contract as
    // `api/world/context/route.test.ts`'s equivalent test for the sibling
    // route.
    await assert.rejects(
      () => verify({ token: TOKEN, proof: WELL_FORMED_PROOF }),
      /WORLD_RP_ID is not set/
    );
  } finally {
    process.env.WORLD_RP_ID = originalRpId;
  }
});

/// The token is the bearer capability that clears this exact challenge, so an
/// operator with log access must never be able to read it back out of a log
/// line and use it themselves. Reusing the same missing-`WORLD_RP_ID` failure
/// as the test above, but watching what actually reaches `console.error`
/// rather than the response, the same way `gate.test.ts` pins a log's fields.
test("an unexpected failure logs a fingerprint of the challenge token, never the token itself", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  const originalRpId = process.env.WORLD_RP_ID;
  delete process.env.WORLD_RP_ID;

  const logged: unknown[][] = [];
  const originalError = console.error;
  console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  try {
    await assert.rejects(() => verify({ token: TOKEN, proof: WELL_FORMED_PROOF }));
  } finally {
    console.error = originalError;
    process.env.WORLD_RP_ID = originalRpId;
  }

  // Checked against the logged *values*, not the whole serialized call: a
  // field is literally named `tokenRef`, so stringifying the field names
  // along with their contents would flag its own key as if it were the token.
  const loggedValues = logged.flatMap(([, context]) => Object.values((context ?? {}) as Record<string, unknown>));
  assert.equal(
    loggedValues.some((value) => typeof value === "string" && value.includes(TOKEN)),
    false
  );
  assert.ok(
    logged.some(([, context]) => typeof (context as { tokenRef?: unknown })?.tokenRef === "string"),
    "a correlator should still reach the log so an operator can tie failures about the same request together"
  );
});

test("mock mode still clears a second sender's own challenge, unchanged by the ledger", async () => {
  process.env.IDENTITY_MODE = "mock";
  await createTestChallenge(OTHER_TOKEN, OTHER_SENDER);

  const first = await verify({ token: TOKEN });
  const second = await verify({ token: OTHER_TOKEN });

  assert.equal(first.status, 200, `expected success: ${first.body.error}`);
  assert.equal(second.status, 200, `mock derives a nullifier per sender, so this must still pass: ${second.body.error}`);
});

/// `schema.ts`'s note on the `nullifiers` table is explicit that nothing in
/// it identifies anyone on its own — a sender address next to the nullifier
/// hash it is bound to is the one combination that would. Dropping the table
/// out from under `claimNullifier` is the simplest way to force its failure
/// path without touching a file outside this route's scope, and the table is
/// rebuilt from `SCHEMA` in `finally` so later tests see the same fixture
/// they always have. Placed last so a mistake here cannot leave any other
/// test running against a half-restored database.
test("a ledger failure while binding a nullifier never logs the sender and the nullifier hash together", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  const client = await db();
  await client.execute("DROP TABLE nullifiers");

  const logged: unknown[][] = [];
  const originalError = console.error;
  console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  try {
    const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });
    assert.equal(status, 502);
    assert.match(body.error ?? "", /could not check this world id/i);
  } finally {
    console.error = originalError;
    for (const statement of SCHEMA) await client.execute(statement);
  }

  const text = JSON.stringify(logged);
  assert.ok(text.includes(NULLIFIER_HASH), "the failure should still be diagnosable by nullifier hash");
  assert.equal(
    logged.some(([, context]) => {
      const line = JSON.stringify(context ?? {});
      return line.includes(SENDER) && line.includes(NULLIFIER_HASH);
    }),
    false,
    "sender and nullifier hash must never be readable off the same log line"
  );
});

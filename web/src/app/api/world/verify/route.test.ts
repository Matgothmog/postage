import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, afterEach, beforeEach, test } from "node:test";
import { hashSignal } from "@worldcoin/idkit/hashing";
import { keccak256, numberToHex, stringToBytes } from "viem";
import { chain } from "@/lib/contracts";
import { now } from "@/lib/time";

// This route reads a challenge from the database and, on the happy path,
// reads the chain before ever reaching World — both need a controlled home
// before the module under test is imported, the same pattern every
// db-touching test file in this tree follows.
const workspace = mkdtempSync(join(tmpdir(), "postage-world-verify-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.WORLD_RP_ID = "app_test_rp_id";
// Keyed like a real load-balanced RPC endpoint would be — a fake key in the
// path, not a bare host:port — so a test that provokes a real transport
// failure against this exercises the exact shape `redact()`'s `RPC_URL`
// pattern exists for: a value a dependency's own error message quotes in
// full, key included, rather than a URL with nothing in it worth hiding.
process.env.ARC_RPC_URL = "http://127.0.0.1:9/rpc/fake-key-9f3a7c21b5e8";
// Throwaway keys for the attest-and-wait leg, which `required()` refuses to
// run without. They only ever sign against the stubbed RPC below, and the
// addresses they derive are never asserted on.
process.env.ATTESTER_PRIVATE_KEY = `0x${"11".repeat(32)}`;
process.env.RELAYER_PRIVATE_KEY = `0x${"22".repeat(32)}`;

const WORLD_VERIFY_URL = "https://developer.world.org/api/v4/verify/app_test_rp_id";
const RPC_URL = process.env.ARC_RPC_URL;

/// The two JSON-RPC "endpoints" a test can target a failure at — see
/// `failRpc` below. `"arc"` is `ARC_RPC_URL`, the one every read, the
/// attestation send, and its receipt wait are all meant to share
/// (`rpcTransport`, `@/lib/client`). `"relayerDefault"` is the chain
/// definition's own public endpoint; nothing should ever reach it now that
/// the write leg and the read leg were unified onto one transport, so it is
/// kept distinguishable here purely as a regression probe for the split that
/// used to paper over the two — where the write leg's *send* landed on this
/// endpoint while reads and the receipt wait went to `ARC_RPC_URL`. Signing
/// itself was never on either: `recordPersonhood`'s attester and relayer are
/// both local accounts (`privateKeyToAccount(...).signTypedData(...)`), so
/// producing a signature is local ECDSA and touches no endpoint at all — only
/// broadcasting the already-signed transaction, and the reads around it, ever
/// go over the wire.
type RpcEndpoint = "arc" | "relayerDefault";
const RELAYER_RPC_URL = chain.rpcUrls.default.http[0];

/// What `recordPersonhood` reads before writing anything, and the
/// `beforeEach` default: the largest value a `uint40` can hold, so a test
/// that never touches `humanUntilAnswer` takes the "already fresh" early
/// return and never reaches the sign-send-wait leg at all — the World call
/// (in live mode) or nothing (in mock mode) stays the only thing such a test
/// puts on the wire. Several tests below deliberately override this to drive
/// that leg instead; this is only what a test gets by not asking for anything
/// different.
const HUMAN_UNTIL_MAX = `0x${"00".repeat(27)}${"ff".repeat(5)}`;
/// The same read answered as zero — nobody has ever been attested — so the
/// early return above does not fire and the whole sign-send-wait leg runs.
const HUMAN_UNTIL_NEVER = `0x${"00".repeat(32)}`;

const ATTESTATION_TX_HASH = `0x${"cd".repeat(32)}`;

const realFetch = globalThis.fetch;
let worldHandler: () => Response | Promise<Response> = () => defaultWorldResponse();
let worldCalls = 0;

/// One RPC failure a test has registered via `failRpc`. `endpoint` narrows it
/// to one leg — omit it to fail the method on either — and `method` is the
/// exact JSON-RPC method name, so a test can fail `eth_getTransactionReceipt`
/// alone and leave `eth_call` and `eth_sendRawTransaction` to succeed, rather
/// than the whole chain going dark at once.
///
/// `mode` picks how it fails. `"network"` (the default) throws the same bare
/// exception a dropped connection would: `viem` wraps it as `HttpRequestError`
/// and retries it three times with backoff, which is the realism the
/// URL-redaction test below depends on — its whole point is that a *real*
/// transport failure's message carries the request URL. A `{ code, message }`
/// mode instead answers with a genuine JSON-RPC error response, which `viem`'s
/// retry policy does not retry for an ordinary error code — reach for this
/// where a test needs the failure to land fast, or to recur many times
/// without paying for backoff on each one.
interface RpcFailure {
  endpoint?: RpcEndpoint;
  method: string;
  mode?: "network" | { code: number; message?: string };
}
let rpcFailures: RpcFailure[] = [];
function failRpc(failure: RpcFailure): void {
  rpcFailures.push(failure);
}
function matchingRpcFailure(endpoint: RpcEndpoint, method: string): RpcFailure | undefined {
  return rpcFailures.find(
    (failure) => (failure.endpoint === undefined || failure.endpoint === endpoint) && failure.method === method
  );
}

/// Every JSON-RPC call this run's stub has actually seen, endpoint and method
/// together — what a test reaches for when it has to prove *where* the route
/// got to before it failed (a send that really went out before its receipt
/// wait failed, say) rather than only which status code came back.
let rpcCallLog: { endpoint: RpcEndpoint; method: string }[] = [];
function rpcCallCount(method: string, endpoint?: RpcEndpoint): number {
  return rpcCallLog.filter((call) => call.method === method && (endpoint === undefined || call.endpoint === endpoint))
    .length;
}
/// Calls this run made to `RELAYER_RPC_URL` specifically, whatever the
/// method — see that constant's comment. Should stay zero on every path
/// through `recordPersonhood`; two tests assert exactly that.
function relayerDefaultCallCount(): number {
  return rpcCallLog.filter((call) => call.endpoint === "relayerDefault").length;
}

/// Methods this stub was asked for but was never taught how to answer,
/// checked in `afterEach` below against every test, not only the ones that
/// expect an RPC call. An unlisted method used to throw a bare `Error` from
/// inside `fetch`, which `viem` cannot tell apart from a dropped connection
/// and so retries three times with backoff — a route that started calling a
/// method this file had never accounted for surfaced as a *slow* test that
/// still happened to land on the same 502 a real fail-closed test asserts on,
/// not as a failure at all. Answering with a genuine "method not found"
/// JSON-RPC error instead (see the stub below) fixes the speed; recording it
/// here and asserting on it after every test is what makes it fail loud
/// rather than quietly agree with whatever status code the test already
/// expected.
let unexpectedRpcMethods: string[] = [];

/// What the `humanUntil` read (`eth_call`) answers. A single value answers
/// every call the same way; an array is consumed one read at a time (the
/// last element repeats once exhausted) — for the one test that needs two
/// different answers from the *same* method depending on when it is asked,
/// without reaching for a second stub layered over this one.
let humanUntilAnswer: string | string[] = HUMAN_UNTIL_MAX;
let humanUntilReads = 0;
function nextHumanUntilAnswer(): string {
  const answer = Array.isArray(humanUntilAnswer)
    ? humanUntilAnswer[Math.min(humanUntilReads, humanUntilAnswer.length - 1)]
    : humanUntilAnswer;
  humanUntilReads += 1;
  return answer;
}

/// What the receipt for the attestation transaction reports. Defaults to the
/// shape every other test in this file wants — already attested, so nothing
/// is sent — and is moved only by the tests about the write leg itself.
let attestationReceiptStatus = "0x1";

function defaultWorldResponse(): Response {
  return Response.json({
    success: true,
    results: [{ identifier: "selfie", success: true, nullifier: "world-nullifier" }],
  });
}

/// Every JSON-RPC method `recordPersonhood` can reach on a working chain,
/// answered from the module state above rather than from a per-test handler,
/// so a test that cares about one of them does not have to restate the rest.
/// Anything not listed here is unexpected — see `unexpectedRpcMethods` —
/// which is how a test finds out the route (or `viem`, on a version bump)
/// started making a call this stub was never taught to expect.
function rpcResult(method: string): unknown {
  switch (method) {
    case "eth_call":
      return nextHumanUntilAnswer();
    case "eth_chainId":
      return numberToHex(chain.id);
    case "eth_getTransactionCount":
      return "0x0";
    case "eth_gasPrice":
    case "eth_maxPriorityFeePerGas":
      return "0x3b9aca00";
    case "eth_estimateGas":
      return "0x186a0";
    case "eth_getBlockByNumber":
      return { number: "0x1", baseFeePerGas: "0x3b9aca00", transactions: [] };
    case "eth_blockNumber":
      return "0x1";
    // No `eth_getTransactionByHash` case, deliberately. It existed for
    // `waitForTransactionReceipt`'s replacement probe, which the route now
    // turns off (`checkReplacement: false`) — so the route asking for it again
    // would mean that decision had been reversed, and `unexpectedRpcMethods`
    // is what says so out loud rather than this file quietly answering.
    case "eth_sendRawTransaction":
      return ATTESTATION_TX_HASH;
    case "eth_getTransactionReceipt":
      return {
        transactionHash: ATTESTATION_TX_HASH,
        transactionIndex: "0x0",
        blockHash: `0x${"ef".repeat(32)}`,
        blockNumber: "0x1",
        cumulativeGasUsed: "0x186a0",
        gasUsed: "0x186a0",
        effectiveGasPrice: "0x3b9aca00",
        contractAddress: null,
        logs: [],
        logsBloom: `0x${"00".repeat(256)}`,
        type: "0x2",
        status: attestationReceiptStatus,
      };
    default:
      return undefined;
  }
}

function withoutTrailingSlash(url: string): string {
  return url.endsWith("/") ? url.slice(0, -1) : url;
}

/// Trailing slash trimmed on both sides: `viem`'s HTTP transport normalises a
/// bare origin into one, and the chain definition's own URL carries none.
function resolveRpcEndpoint(url: string): RpcEndpoint | undefined {
  if (withoutTrailingSlash(url) === RPC_URL) return "arc";
  if (withoutTrailingSlash(url) === withoutTrailingSlash(RELAYER_RPC_URL)) return "relayerDefault";
  return undefined;
}

globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
  const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
  if (url === WORLD_VERIFY_URL) {
    worldCalls += 1;
    return worldHandler();
  }

  const endpoint = resolveRpcEndpoint(url);
  if (!endpoint) throw new Error(`a test tried to reach ${url}`);

  const { id, method } = JSON.parse(String(init?.body)) as { id: number; method: string };
  rpcCallLog.push({ endpoint, method });

  if (method === "eth_fillTransaction") {
    // `viem`'s own fill-in-one-round-trip optimisation, attempted before the
    // individual nonce/gas/fee reads below on every write this route makes.
    // Answering "not found" is what a real node without this extension sends
    // too, and is what makes `viem` fall back to the calls this stub already
    // knows (see `prepareTransactionRequest`'s own fallback, keyed on this
    // exact error name). A permanent, expected part of every write here, not
    // a gap in this stub's own knowledge — so it is never counted as
    // unexpected.
    return Response.json({
      jsonrpc: "2.0",
      id,
      error: { code: -32601, message: "eth_fillTransaction is not available" },
    });
  }

  const failure = matchingRpcFailure(endpoint, method);
  if (failure) {
    const mode = failure.mode ?? "network";
    if (mode === "network") throw new TypeError("fetch failed");
    return Response.json({ jsonrpc: "2.0", id, error: { code: mode.code, message: mode.message ?? "simulated RPC failure" } });
  }

  const result = rpcResult(method);
  if (result !== undefined) return Response.json({ jsonrpc: "2.0", id, result });

  // Unlisted — never something a test means to exercise. A JSON-RPC "method
  // not found" error, rather than a thrown network exception, is what makes
  // this land fast: `viem`'s retry policy does not retry a numbered RPC error
  // outside a small allow-list, so this surfaces on the first attempt rather
  // than after three rounds of backoff. `unexpectedRpcMethods` is what makes
  // it land loud — see its own comment.
  unexpectedRpcMethods.push(method);
  return Response.json({
    jsonrpc: "2.0",
    id,
    error: { code: -32601, message: `test stub: unlisted RPC method "${method}"` },
  });
};

const { claimChallenge, createChallenge } = await import("@/lib/db/challenges");
const { db, reset } = await import("@/lib/db/client");
const { MAX_LIVE_CONTEXTS_PER_TOKEN, recordIssuedContext } = await import("@/lib/db/issued-contexts");
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
  rpcFailures = [];
  rpcCallLog = [];
  unexpectedRpcMethods = [];
  humanUntilAnswer = HUMAN_UNTIL_MAX;
  humanUntilReads = 0;
  attestationReceiptStatus = "0x1";
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

afterEach(() => {
  // The regression guard for every test at once, not just the ones that
  // expect an RPC call — see `unexpectedRpcMethods`'s own comment for why
  // this has to be a hard failure rather than a log line.
  assert.deepEqual(
    unexpectedRpcMethods,
    [],
    `test stub saw unlisted RPC method(s): ${unexpectedRpcMethods.join(", ")} — teach rpcResult about them, or the route changed in a way this file has not caught up with`
  );
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

test("a network failure reaching World is a gateway error, not a rejected proof, and logs a correlator", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  worldHandler = () => {
    throw new Error("simulated network failure");
  };

  const logged: unknown[][] = [];
  const originalError = console.error;
  console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  let status: number;
  let body: VerifyResponse;
  try {
    ({ status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF }));
  } finally {
    console.error = originalError;
  }

  assert.equal(status, 502);
  assert.match(body.error ?? "", /could not reach world id/i);
  // An outage here is exactly what a rejected proof already gets a `tokenRef`
  // for — this sibling failure must be correlatable the same way.
  assert.ok(
    logged.some(([, context]) => typeof (context as { tokenRef?: unknown })?.tokenRef === "string"),
    "a network failure reaching World should still log a tokenRef"
  );
});

test("World's own server error is a gateway error, not a rejected proof, and logs a correlator", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  worldHandler = () => Response.json({ detail: "internal error, try again later" }, { status: 503 });

  const logged: unknown[][] = [];
  const originalError = console.error;
  console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  let status: number;
  let body: VerifyResponse;
  try {
    ({ status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF }));
  } finally {
    console.error = originalError;
  }

  assert.equal(status, 502);
  // A 5xx here is World's own failure, not a verdict on the proof, so the
  // taxonomy in the comment above `verifyWithWorld` demands 502 rather than
  // the 400 the old `!response.ok` check alone would have produced — and
  // World's own internal detail is never World's to hand back either.
  assert.equal(body.error?.includes("internal error"), false);
  assert.ok(
    logged.some(([, context]) => typeof (context as { tokenRef?: unknown })?.tokenRef === "string"),
    "World's own server error should still log a tokenRef"
  );
});

test("a proof World rejects outright is a bad request, not a gateway error", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  worldHandler = () => Response.json({ success: false, detail: "invalid rp signature" }, { status: 400 });

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 400);
  // World's own `detail` is its prose about its own API, not copy written
  // for a sender — an unrecognised code (none is sent here) falls back to
  // curated copy rather than repeating World's words verbatim.
  assert.match(body.error ?? "", /verification failed/i);
  assert.equal(body.error?.includes("invalid rp signature"), false);
});

/// The defect this file exists to close: World's own `detail` is its prose
/// about its own API, aimed at a developer, and reaches this route unfiltered
/// — see docs/world-feedback.md:267 for World emitting exactly this register
/// of sentence-form text. It must never reach a sender reading their mail
/// client, whatever World happens to say.
test("World's own detail text is never handed back to the sender", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  worldHandler = () => Response.json({ success: false, detail: "No action found for this app." }, { status: 400 });

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 400);
  assert.equal(body.error?.includes("No action found for this app"), false);
  assert.match(body.error ?? "", /verification failed/i);
});

test("a signature-expired rejection gets curated copy, not World's own wording", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  worldHandler = () =>
    Response.json(
      { success: false, code: "rp_signature_expired", detail: "signature outside its validity window" },
      { status: 400 }
    );

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 400);
  assert.match(body.error ?? "", /expired/i);
  assert.equal(body.error?.includes("validity window"), false);
});

/// All three spellings `route.ts` maps to the same copy, because nobody
/// knows which one World actually sends for this endpoint: `max_verifications_reached`
/// is the one compiled into IDKit's shipped binary today, `exceeded_max_verifications`
/// and `already_verified` are older names still seen in World's own v2-era
/// docs and search results. The two untested spellings are exactly the ones a
/// later edit would drop first, on the mistaken assumption that only the
/// current binary's name matters.
for (const verificationLimitCode of ["max_verifications_reached", "exceeded_max_verifications", "already_verified"]) {
  test(`a verification-limit rejection ("${verificationLimitCode}") gets curated copy, not World's own wording`, async () => {
    process.env.IDENTITY_MODE = "live";
    await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
    worldHandler = () =>
      Response.json(
        { success: false, code: verificationLimitCode, detail: "this account has no verifications left" },
        { status: 400 }
      );

    const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

    assert.equal(status, 400);
    assert.match(body.error ?? "", /already been used/i);
    assert.equal(body.error?.includes("no verifications left"), false);
    // This is the failure most likely to be permanent — World's own limit,
    // not ours — so unlike a transient rejection it must not sit there
    // offering the sender nothing to do next.
    assert.match(body.error ?? "", /pay instead/i);
  });
}

test("a rejection with neither code nor detail still gets safe generic copy", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  worldHandler = () => Response.json({ success: false }, { status: 400 });

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 400);
  assert.match(body.error ?? "", /verification failed/i);
});

/// The unrecognised-code case is the hole that would silently reopen this
/// defect the moment World adds a new code: this pins that it still cannot
/// leak `detail`, and that the unmapped code reaches the server log loudly
/// enough that an operator can notice and add curated copy for it.
test("an unrecognised code falls back to generic copy and is logged for follow-up", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  worldHandler = () =>
    Response.json(
      { success: false, code: "some_new_code_world_added", detail: "World's own new explanation" },
      { status: 400 }
    );

  const logged: unknown[][] = [];
  const originalError = console.error;
  console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  let status: number;
  let body: VerifyResponse;
  try {
    ({ status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF }));
  } finally {
    console.error = originalError;
  }

  assert.equal(status, 400);
  assert.match(body.error ?? "", /verification failed/i);
  assert.equal(body.error?.includes("World's own new explanation"), false);
  assert.equal(body.error?.includes("some_new_code_world_added"), false);

  const contexts = logged.map(([, context]) => context as Record<string, unknown> | undefined);
  assert.ok(
    contexts.some((context) => context?.code === "some_new_code_world_added"),
    "the unrecognised code should still reach the server log so it can be added"
  );
  assert.ok(
    contexts.some((context) => context?.detail === "World's own new explanation"),
    "World's own detail should still reach the server log for diagnosis"
  );
  // Same token-secrecy contract the rest of this file already enforces: a
  // correlator reaches the log, never the bearer token itself.
  const loggedValues = logged.flatMap(([, context]) => Object.values((context ?? {}) as Record<string, unknown>));
  assert.equal(
    loggedValues.some((value) => typeof value === "string" && value.includes(TOKEN)),
    false
  );
});

/// The copy table used to be an object literal indexed straight with
/// `payload.code`, which is whatever World put in a response body — so a code
/// naming something every object inherits found it. `"toString"` answered with
/// `Object.prototype.toString`, which is not `undefined`, so the generic
/// fallback never fired and the sender was handed
/// "function toString() { [native code] }" at 400. The lookup goes through a
/// `Map` now (`@/lib/world-id-messages.ts`), which has no inherited keys to
/// find; this is what would notice if it ever went back to an object.
test("a code naming an inherited property gets generic copy, not a stringified function", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  worldHandler = () => Response.json({ success: false, code: "toString" }, { status: 400 });

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 400);
  assert.match(body.error ?? "", /verification failed/i);
  assert.equal(body.error?.includes("native code"), false);
  assert.equal(body.error?.includes("function"), false);
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
  // The pre-send `humanUntil` read, targeted explicitly rather than the whole
  // chain going dark — `mode: "network"` (the default) is what makes this a
  // real viem `HttpRequestError`, the realism this test's own assertion needs.
  failRpc({ endpoint: "arc", method: "eth_call" });

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 502);
  // A real transport failure here is a viem `HttpRequestError`, whose own
  // `message` embeds the full request URL — which is `ARC_RPC_URL`, an
  // operator-configured endpoint that may carry a key in its path or query.
  assert.equal(body.error?.includes(RPC_URL), false);
  assert.equal(body.error?.includes("127.0.0.1"), false);
  assert.match(body.error ?? "", /could not record the attestation/i);
});

/// The regression this charge exists to close: `describeFailure` used to walk
/// `.cause` and log the raw text with no redaction at all — worse than the
/// bare `cause.message` it replaced, since a transport failure's message
/// carries the endpoint several `.cause` links down, and the walk surfaced
/// every one of them. The test above already pins that nothing reaches the
/// sender; this pins the log line an operator actually reads.
test("a keyed RPC endpoint does not survive into the log line, even nested under a real cause chain", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  failRpc({ endpoint: "arc", method: "eth_call" });

  const logged: unknown[][] = [];
  const originalError = console.error;
  console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  let status: number;
  try {
    ({ status } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF }));
  } finally {
    console.error = originalError;
  }
  assert.equal(status, 502);

  const entry = logged.find(([message]) => message === "world id attestation failed");
  assert.ok(entry, "expected an attestation-failure log line");
  const reason = (entry[1] as { reason?: unknown })?.reason;
  assert.equal(typeof reason, "string");
  const text = reason as string;

  // Proves the walk actually reached the link that names the endpoint, not
  // just the outer wrapper: a real `ContractFunctionExecutionError` nests
  // several `.cause` links above the `HttpRequestError` that carries the URL
  // (see the comment above `describeFailure`), so a `reason` with only one
  // segment would mean this assertion never exercised the nested case at all.
  assert.ok(text.includes(": "), `expected a joined multi-link chain, got: ${text}`);
  assert.equal(text.includes(RPC_URL), false, `raw RPC URL leaked into the log: ${text}`);
  assert.equal(text.includes("fake-key-9f3a7c21b5e8"), false, `raw key leaked into the log: ${text}`);
  assert.match(text, /\[redacted ARC_RPC_URL\]/, `expected a redaction marker in place of the URL, got: ${text}`);
});

/// The defect this charge exists to close. `bindNullifierToSender` used to run
/// ahead of the chain write, so a failed attestation answered "could not
/// record the attestation" over a ledger that had already been written to.
/// Fail-closed means the sentence is true: the write did not land, so nothing
/// bound.
test("a failed attestation leaves no nullifier bound to anyone", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  failRpc({ endpoint: "arc", method: "eth_call" });

  const { status } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 502);
  assert.equal(await senderHoldingNullifier(NULLIFIER_HASH), null);
  assert.deepEqual(await rebindsOf(NULLIFIER_HASH), []);
});

test("a failed attestation grants no pass and leaves the challenge unsettled", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  failRpc({ endpoint: "arc", method: "eth_call" });

  const { status } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 502);
  // Personhood is the pass, and it is not granted unless the record landed.
  assert.equal(await hasLivePass(HANDLE, SENDER), false);
});

/// The expensive half of the old ordering, and the reason this is worth more
/// than a tidier error path: `claimNullifier` takes the nullifier off whoever
/// held it *and* deletes the earned passes they were standing on, in one
/// transaction. Run before the chain write, a failed attestation stripped a
/// third party's free lane and handed the sender nothing in exchange — a
/// state change nobody asked for, behind a 502 saying nothing had happened.
///
/// No concurrent ("two failures racing") variant of this exists, and none is
/// missing: the whole point of the ordering is that a failed attestation
/// never reaches `claimNullifier` at all, so two of them in flight together
/// touch no shared, mutable state and cannot interact — running two at once
/// with `Promise.all` would just be this same assertion twice, over two
/// requests that never see each other.
test("a failed attestation cannot take a binding off the sender who holds it", async () => {
  process.env.IDENTITY_MODE = "live";
  await bindToFirstSender();
  assert.equal(await hasLivePass(HANDLE, SENDER), true, "test setup: the first sender must be standing on a pass");
  const otherProof = await challengeForOtherSender();
  failRpc({ endpoint: "arc", method: "eth_call" });

  const { status } = await verify({ token: OTHER_TOKEN, proof: otherProof });

  assert.equal(status, 502);
  assert.equal(await senderHoldingNullifier(NULLIFIER_HASH), SENDER);
  assert.deepEqual(await rebindsOf(NULLIFIER_HASH), []);
  assert.equal(await hasLivePass(HANDLE, SENDER), true);
});

test("a failed attestation tells the sender nothing was saved", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  failRpc({ endpoint: "arc", method: "eth_call" });

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 502);
  // The sender is about to decide whether to spend another Selfie Check on
  // this, so the one thing the message has to carry is that there is nothing
  // behind them to undo. This fails before any hash exists, so unlike the
  // receipt-wait-failed case, "nothing was saved" is not a guess here.
  assert.match(body.error ?? "", /nothing was saved/i);
  // World's own one-time verification for this World ID is already spent —
  // `verifyWithWorld` ran before `recordPersonhood` ever did — so the message
  // must not promise a retry will work; paying is the door that still does.
  assert.doesNotMatch(body.error ?? "", /try again/i);
  assert.match(body.error ?? "", /pay instead/i);
});

/// Mock mode reaches the chain write with no proof and no signing context at
/// all, which is the configuration the failure was actually found under on a
/// deployed preview. Nothing about fail-closed may depend on the live branch
/// having run.
test("a failed attestation under mock mode leaves no binding either", async () => {
  process.env.IDENTITY_MODE = "mock";
  failRpc({ endpoint: "arc", method: "eth_call" });
  const mockHash = keccak256(stringToBytes(`mock-selfie:${SENDER.toLowerCase()}`));

  const { status } = await verify({ token: TOKEN });

  assert.equal(status, 502);
  assert.equal(await senderHoldingNullifier(mockHash), null);
  assert.equal(await hasLivePass(HANDLE, SENDER), false);
});

/// Retrying has to end where a first attempt that never failed would have:
/// one holder, no recorded hop, a pass in hand. A hop here would mean the
/// failed attempt had left a binding for the retry to move, which is exactly
/// what must not have happened.
test("a retry after a failed attestation clears the challenge and records one binding", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  failRpc({ endpoint: "arc", method: "eth_call" });
  const failed = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });
  assert.equal(failed.status, 502);

  // A fresh Selfie Check is a fresh signing context, so the retry carries a
  // nonce of its own — the spent one is not presentable again, by design.
  rpcFailures = [];
  const retryProof = { ...WELL_FORMED_PROOF, nonce: "nonce-retry" };
  await issueContext(TOKEN, retryProof.nonce);

  const retried = await verify({ token: TOKEN, proof: retryProof });

  assert.equal(retried.status, 200, `the retry must succeed on its own: ${retried.body.error}`);
  assert.equal(await senderHoldingNullifier(NULLIFIER_HASH), SENDER);
  assert.deepEqual(await rebindsOf(NULLIFIER_HASH), []);
  assert.equal(await hasLivePass(HANDLE, SENDER), true);
});

/// The riskiest gap this branch left uncovered: `eth_sendRawTransaction`
/// succeeding and only the receipt wait afterwards failing. Every fail-closed
/// test above fails at the pre-send `humanUntil` read, where "nothing was
/// saved" is trivially true — nothing was ever sent. Here a transaction
/// really was broadcast, so that sentence would be a guess, not a fact: the
/// attestation may still mine after this response goes out. The route says so
/// rather than asserting the stronger, false claim — this is the one branch
/// `AttestationOutcomeUnknown` exists for. `rpcCallCount("eth_sendRawTransaction")`
/// is what makes "a transaction really was sent" real rather than assumed.
///
/// Reaching this through `waitForTransactionReceipt`'s real polling path
/// (rather than fabricating a rejection some other way) is why `rpcResult`
/// above answers `eth_blockNumber`: the first, swallowed receipt read always
/// falls through to viem's block watcher regardless of how it fails, and that
/// answer is what lets the watcher's very first tick — `emitOnBegin` fires it
/// immediately, no polling-interval wait — reach a second, unswallowed receipt
/// read. That second read is where this failure is made to land, and it
/// rejects immediately rather than waiting out the 20s bound
/// `recordPersonhood` puts on the wait, because the JSON-RPC error code here
/// is one `viem`'s retry policy does not retry.
test("send succeeded but the receipt wait failed: the sender is told the outcome is unknown, not that nothing was saved", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  humanUntilAnswer = HUMAN_UNTIL_NEVER;
  failRpc({
    endpoint: "arc",
    method: "eth_getTransactionReceipt",
    mode: { code: -32000, message: "simulated: receipt temporarily unavailable" },
  });

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 502);
  assert.equal(
    rpcCallCount("eth_sendRawTransaction"),
    1,
    "test setup: the send has to have gone out for this case to mean anything"
  );
  // A hash exists here, so unlike every pre-send or confirmed-revert failure
  // above, this response must not claim "nothing was saved" — it does not
  // know that. It also must not tell the sender to try again: World's own
  // one-time verification for this World ID is already spent by this point
  // regardless of how the chain write resolves.
  assert.doesNotMatch(body.error ?? "", /nothing was saved/i);
  assert.doesNotMatch(body.error ?? "", /try again/i);
  assert.match(body.error ?? "", /may still complete/i);
  assert.match(body.error ?? "", /pay instead/i);
  assert.equal(await senderHoldingNullifier(NULLIFIER_HASH), null);
  assert.equal(await hasLivePass(HANDLE, SENDER), false);
});

/// `AttestationOutcomeUnknown`'s own message names the transaction hash and
/// nothing else, so logging it alone handed an operator a hash to reconcile by
/// hand with no account of why it needed reconciling — a wait that timed out,
/// an RPC that went away and a receipt read that errored all read identically.
/// The wrapped cause is what tells them apart, and it only reaches the log
/// because the log follows `.cause`.
test("an unknown attestation outcome logs what actually failed, not just the hash", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  humanUntilAnswer = HUMAN_UNTIL_NEVER;
  failRpc({
    endpoint: "arc",
    method: "eth_getTransactionReceipt",
    mode: { code: -32000, message: "simulated: receipt temporarily unavailable" },
  });

  const logged: unknown[][] = [];
  const originalError = console.error;
  console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  let status: number;
  try {
    ({ status } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF }));
  } finally {
    console.error = originalError;
  }

  assert.equal(status, 502);
  const failure = logged.find(([line]) => line === "world id attestation failed");
  assert.ok(failure, "the failure has to reach the log at all");
  const context = JSON.stringify(failure[1] ?? {});
  assert.ok(context.includes(ATTESTATION_TX_HASH), "the hash is what an operator reconciles by");
  assert.ok(
    context.includes("receipt temporarily unavailable"),
    "and the wrapped cause is what says which failure this was"
  );
});

/// A receipt is not a success. `waitForTransactionReceipt` resolves a reverted
/// transaction exactly as happily as a mined one, so every revert `attest` can
/// throw — `NotAnExtension`, `NullifierAlreadyBound`, `InvalidSignature`, all
/// in `contracts/src/HumanRegistry.sol` — used to return from
/// `recordPersonhood` as though the record had been written. That is the same
/// defect as the one above wearing a different coat: a lane granted with
/// nothing onchain to account for it.
test("an attestation that reverts onchain is not personhood", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  // Low enough that the route signs and sends rather than taking the "already
  // fresh" early return, and a receipt that reports the revert.
  humanUntilAnswer = HUMAN_UNTIL_NEVER;
  attestationReceiptStatus = "0x0";

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 502);
  // A receipt confirmed the revert, so this is known, not guessed — the
  // message may still say "nothing was saved" even though a hash exists.
  assert.match(body.error ?? "", /nothing was saved/i);
  assert.doesNotMatch(body.error ?? "", /try again/i);
  assert.match(body.error ?? "", /pay instead/i);
  assert.equal(await senderHoldingNullifier(NULLIFIER_HASH), null);
  assert.equal(await hasLivePass(HANDLE, SENDER), false);
});

/// The race the fix above closes: two verifications of the same nullifier
/// both pass `recordPersonhood`'s "already fresh" pre-send read before
/// either has written anything, both sign and send, and whichever lands
/// second reverts `NotAnExtension` against the `humanUntil` the first one
/// just set — a revert that means "already attested", not "nothing was
/// saved". Proving the fix needs the *first* `humanUntil` read (before send)
/// to say "not fresh" — so the route actually signs and sends instead of
/// taking the early return — while the *second* read (this fix's re-check,
/// after the revert) says "fresh", standing in for the concurrent winner's
/// write having landed in between. `humanUntilAnswer` takes a sequence for
/// exactly this: the two reads answered in order, declared here rather than
/// through a second stub layered over the shared one.
test("a revert against an identity already fresh on chain is granted the lane anyway", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  // Not fresh yet on the pre-send read, same setup as "an attestation that
  // reverts onchain is not personhood" — but here the post-revert re-check
  // answers as though a concurrent winner had landed in between.
  humanUntilAnswer = [HUMAN_UNTIL_NEVER, HUMAN_UNTIL_MAX];
  attestationReceiptStatus = "0x0";

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 200, `expected the race's loser to still be granted the lane: ${body.error}`);
  assert.equal(await senderHoldingNullifier(NULLIFIER_HASH), SENDER);
  assert.equal(await hasLivePass(HANDLE, SENDER), true);
  assert.equal(humanUntilReads, 2, "expected exactly one pre-send read and one post-revert re-check");
  // Both reads went through `publicClient` on `ARC_RPC_URL`, never the
  // chain's own public default — the same regression this file already
  // guards for the write leg.
  assert.equal(relayerDefaultCallCount(), 0);
});

/// The other side of that same re-check, and the fail-open it shipped with:
/// asking `>= now()` asked only "does this identity hold any live record at
/// all", which every returning sender inside the 90-day credential lifetime
/// answers yes to. A revert with nothing to do with the race — an
/// `InvalidSignature` after an attester-key rotation, a redeployed registry —
/// therefore returned from `recordPersonhood` as success, and the route bound
/// the nullifier, opened the gate and answered 200 with no `console.error`
/// anywhere. `>= expiresAt` is the comparison `NotAnExtension` is itself
/// written in, so a record that is live but *less* fresh than this attempt
/// cannot be what caused the revert and is not a reason to grant anything.
test("a revert against an identity whose record is live but older than this attempt is not personhood", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  // Not fresh at all on the pre-send read, so the route signs and sends; live
  // for another day on the post-revert re-check, where this attempt was
  // writing ninety. Under the old `>= now()` threshold this was a 200.
  humanUntilAnswer = [HUMAN_UNTIL_NEVER, numberToHex(BigInt(now() + 86_400), { size: 32 })];
  attestationReceiptStatus = "0x0";

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 502);
  assert.match(body.error ?? "", /nothing was saved/i);
  assert.equal(await senderHoldingNullifier(NULLIFIER_HASH), null);
  assert.equal(await hasLivePass(HANDLE, SENDER), false);
});

/// A rescued revert is still a revert, and the outage hiding behind one is the
/// hardest kind to see: an attester key that has stopped producing valid
/// signatures fails only senders with no record yet and succeeds silently for
/// everyone who already has one. Nothing but this log line stands between that
/// and nobody noticing, so what it carries is pinned here rather than left to
/// habit.
test("a revert granted the lane anyway is still logged, with the reading that justified it", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  humanUntilAnswer = [HUMAN_UNTIL_NEVER, HUMAN_UNTIL_MAX];
  attestationReceiptStatus = "0x0";

  const logged: unknown[][] = [];
  const originalError = console.error;
  console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  let status: number;
  try {
    ({ status } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF }));
  } finally {
    console.error = originalError;
  }

  assert.equal(status, 200);
  const reverted = logged.find(([line]) => typeof line === "string" && line.includes("reverted"));
  assert.ok(reverted, "a revert has to reach the log even when the lane is granted anyway");
  const context = JSON.stringify(reverted[1] ?? {});
  assert.ok(context.includes(ATTESTATION_TX_HASH), "the hash is what makes a revert diagnosable after the fact");
  assert.match(context, /"grantedAnyway":true/);
});

test("an attestation that mines successfully clears the challenge", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  // The same write leg as the test above, differing only in what the receipt
  // reports — otherwise a route that refused every receipt would pass it.
  humanUntilAnswer = HUMAN_UNTIL_NEVER;

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 200, `expected the attestation to be accepted: ${body.error}`);
  assert.equal(await senderHoldingNullifier(NULLIFIER_HASH), SENDER);
  assert.equal(await hasLivePass(HANDLE, SENDER), true);
});

/// The regression guard for the two-provider split `RELAYER_RPC_URL`'s own
/// comment describes: `recordPersonhood` used to read `humanUntil` and wait
/// for its receipt through `ARC_RPC_URL` while its wallet client — built on a
/// second, unconfigured provider (a bare `http()`, defaulting to whatever
/// public RPC `arcTestnet` ships with) — broadcast the attestation there
/// instead. Signing was never part of the split: it is local ECDSA
/// (`privateKeyToAccount(...).signTypedData(...)`) and touches no endpoint at
/// all, on either side of this fix. This test drives the same full
/// sign-send-wait leg as "an attestation that mines successfully clears the
/// challenge" and asserts nothing on that path ever reached the second
/// endpoint — every JSON-RPC call this route makes, including the ones inside
/// `waitForTransactionReceipt`, went to `ARC_RPC_URL` alone.
///
/// This does not exercise cross-provider propagation lag itself — this file's
/// stub answers `eth_getTransactionReceipt` immediately regardless of what
/// broadcast it. The failure mode it therefore cannot reproduce is the one the
/// split actually produced in the field: a receipt wait polling a provider
/// that never saw the broadcast, timing out at the bound as though nothing had
/// been sent while the attestation mined perfectly well somewhere else. What
/// this does confirm is that the two legs are no longer configured to diverge
/// in the first place.
test("the attestation write and its receipt wait share one RPC endpoint, never the chain's public default", async () => {
  process.env.IDENTITY_MODE = "live";
  await issueContext(TOKEN, WELL_FORMED_PROOF.nonce);
  humanUntilAnswer = HUMAN_UNTIL_NEVER;

  const { status, body } = await verify({ token: TOKEN, proof: WELL_FORMED_PROOF });

  assert.equal(status, 200, `expected the attestation to be accepted: ${body.error}`);
  assert.equal(relayerDefaultCallCount(), 0);
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

/// Mock mode presents no proof, so it reaches none of the three refusals the
/// live branch is bounded by — and every request past them signs and sends a
/// relayer-funded `attest`, since `recordPersonhood`'s "already fresh" early
/// return stops firing the moment `expiresAt` moves on. One ordinary
/// challenge token was therefore an unbounded draw on the relayer's vault, in
/// the one configuration the deployed app actually runs. The four tests below
/// pin what now stops it and what must still work.
///
/// A JSON-RPC error targeted at `eth_call`, rather than a `mode: "network"`
/// failure: both fail `recordPersonhood` on its first read and leave the
/// challenge unsettled, which is what keeps the ceiling — rather than the
/// settled check below — the thing under test, but this one is not retried
/// by the transport, so the several attempts this test has to make do not
/// each pay for viem's backoff.
test("mock mode stops reaching the chain once one token has spent its verification attempts", async () => {
  process.env.IDENTITY_MODE = "mock";
  failRpc({ endpoint: "arc", method: "eth_call", mode: { code: -32000, message: "simulated node failure" } });

  for (let attempt = 1; attempt <= MAX_LIVE_CONTEXTS_PER_TOKEN; attempt += 1) {
    const { status } = await verify({ token: TOKEN });
    assert.equal(status, 502, `attempt ${attempt} should still be spending an allowance, not refused by it`);
  }
  const spent = rpcCallLog.length;
  assert.ok(spent > 0, "the attempts above have to have reached the chain for bounding them to mean anything");

  const refused = await verify({ token: TOKEN });

  assert.equal(refused.status, 429);
  assert.match(refused.body.error ?? "", /too many verification attempts/i);
  assert.equal(rpcCallLog.length, spent, "a refused attempt must not reach the chain at all");
});

/// The settled-token half. Nothing above bounds it: `claimChallenge` — the
/// settle-once guard — sits at the very end of this route, inside `openGate`,
/// so a token that had already been answered still bought a fresh attestation
/// before anything noticed, and the ceiling above cannot stand in for that
/// because it is a rate limit rather than a lifetime cap — its slots expire on
/// the 300s rp_context TTL, and a post-send failure leaves `resolved_at` NULL
/// for the sender to retry against, which is exactly what the retry test below
/// depends on. Live mode refuses a settled token a step earlier, at
/// `/api/world/context`, which will not sign for a challenge whose
/// `resolved_at` is set.
///
/// What mock mode does here instead of refusing: skip the chain write and go
/// on to `openGate` anyway. The relayer pays nothing, which is all the refusal
/// was ever for, and `gate.ts:41-42`'s fifth invariant — answering a settled
/// challenge reports what actually happened, and repairs a sender who holds
/// nothing — stays true in the one mode production runs. This asserted 409
/// before, which was the settled check refusing outright, and which
/// `postWorldVerify` (`world-id.ts`) turns into a thrown hard failure; the
/// expectation moved to 200 because the behaviour deliberately did. The
/// `eth_sendRawTransaction` count is unchanged, and is what still holds the
/// gas drain closed.
test("a mock verification of an already-answered challenge buys no second attestation", async () => {
  process.env.IDENTITY_MODE = "mock";
  // Nobody is attested yet, so the first attempt runs the whole sign-send-wait
  // leg and the relayer really does pay for one.
  humanUntilAnswer = HUMAN_UNTIL_NEVER;

  const first = await verify({ token: TOKEN });
  assert.equal(first.status, 200, `expected the first verification to succeed: ${first.body.error}`);
  assert.equal(
    rpcCallCount("eth_sendRawTransaction"),
    1,
    "test setup: the first attempt has to have paid for an attestation"
  );
  const reachedChain = rpcCallLog.length;

  const second = await verify({ token: TOKEN });

  assert.equal(second.status, 200, `a settled challenge reports what happened rather than refusing: ${second.body.error}`);
  assert.equal(rpcCallCount("eth_sendRawTransaction"), 1, "a settled challenge must not drive another relayer-funded write");
  assert.equal(
    rpcCallLog.length,
    reachedChain,
    "a settled challenge must not reach the chain at all — not even for the pre-send read"
  );
});

/// The other half of skipping the write rather than refusing the request:
/// `openGate` still runs, so its recover path can give back what the lane
/// earned to a sender holding nothing. A bare `claimChallenge` is exactly that
/// state — settled by us, with the answer lost before anything was granted —
/// which is the case `gate.ts:41-42` is written for and which a 409 here
/// turned into a hard failure at `postWorldVerify` instead of a repair.
test("a mock verification repairs a settled sender who holds nothing, and still spends no gas", async () => {
  process.env.IDENTITY_MODE = "mock";
  assert.equal(await claimChallenge(TOKEN, "human"), true, "test setup: the claim has to settle the challenge");
  assert.equal(await hasLivePass(HANDLE, SENDER), false, "test setup: the sender has to start out holding nothing");
  // Nobody is attested, so a route that had reached `recordPersonhood` at all
  // would have run the whole sign-send-wait leg rather than an early return.
  humanUntilAnswer = HUMAN_UNTIL_NEVER;

  const { status, body } = await verify({ token: TOKEN });

  assert.equal(status, 200, `expected the repair, not a refusal: ${body.error}`);
  assert.equal(await hasLivePass(HANDLE, SENDER), true);
  assert.equal(rpcCallLog.length, 0, "repairing a settled sender must cost the relayer nothing, not even a read");
});

/// The ceiling is keyed on the challenge token, which is the bearer capability
/// this whole route is already judged against — so spending it is something
/// only its holder can do, and one exhausted token cannot be used to shut
/// anybody else out.
test("one token's spent attempts never refuse another sender's challenge", async () => {
  process.env.IDENTITY_MODE = "mock";
  await createTestChallenge(OTHER_TOKEN, OTHER_SENDER);
  for (let taken = 0; taken < MAX_LIVE_CONTEXTS_PER_TOKEN; taken += 1) {
    await issueContext(TOKEN, `spent-${taken}`);
  }

  const refused = await verify({ token: TOKEN });
  const other = await verify({ token: OTHER_TOKEN });

  assert.equal(refused.status, 429);
  assert.equal(other.status, 200, `a second token's allowance is its own: ${other.body.error}`);
});

/// What the ceiling must not cost: mock mode exists so the app is usable with
/// no World credentials at all, and a sender whose attempt failed on our side
/// has to be able to try again.
test("mock mode still lets a sender retry after a failed attestation", async () => {
  process.env.IDENTITY_MODE = "mock";
  failRpc({ endpoint: "arc", method: "eth_call" });
  const failed = await verify({ token: TOKEN });
  assert.equal(failed.status, 502);

  rpcFailures = [];
  const retried = await verify({ token: TOKEN });

  assert.equal(retried.status, 200, `a retry inside the allowance must still clear: ${retried.body.error}`);
  assert.equal(await hasLivePass(HANDLE, SENDER), true);
});

/// Live mode is bounded by `spendIssuedContext` and the signing route's own
/// refusals, and none of the above is allowed to reach it. The case that would
/// show it had: a context minted before the challenge was settled, presented
/// after — `openGate`'s recover path, which repairs a sender whose answer was
/// lost, and which a settled-check on the shared path would have turned into a
/// refusal.
test("live mode still clears a challenge that was settled while its context was outstanding", async () => {
  process.env.IDENTITY_MODE = "live";
  await bindToFirstSender();
  const outstanding = { ...WELL_FORMED_PROOF, nonce: "nonce-outstanding" };
  await issueContext(TOKEN, outstanding.nonce);

  const { status, body } = await verify({ token: TOKEN, proof: outstanding });

  assert.equal(status, 200, `live mode must be exactly as it was: ${body.error}`);
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

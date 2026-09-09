import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, test } from "node:test";
import { startStubChain } from "../../../../../test/chain";
import { startStubModel } from "../../../../../test/model";

const chain = await startStubChain();

const workspace = mkdtempSync(join(tmpdir(), "postage-faults-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";
process.env.MAIL_WEBHOOK_SECRET = "secret";
process.env.APP_URL = "http://localhost";
process.env.MESSAGE_ID_SECRET = "x".repeat(32);
process.env.CLASSIFIER_PRIVATE_KEY = `0x${"11".repeat(32)}`;

const model = await startStubModel();

const { reset } = await import("@/lib/db/client");
const { createInbox } = await import("@/lib/db/inboxes");
const { GatewayFault, during, faultResponse, redact } = await import("./faults");
const { POST } = await import("./route");

/// Every fault in this file writes a line to `console.error` on purpose, and
/// two of the tests are about what that line says. Collected rather than
/// printed, so the suite's own output stays readable and the line can be read
/// back rather than assumed.
const REAL_CONSOLE_ERROR = console.error;
const logged: unknown[][] = [];
console.error = (...line: unknown[]) => {
  logged.push(line);
};

const MESSAGE = {
  from: "someone@nowhere.example",
  to: "demo@usepostage.com",
  subject: "hello",
  body: "x",
  spf: "pass",
  dkim: "pass",
  dmarc: "pass",
};

function post(message: object, secret: string | null = "secret"): Promise<Response> {
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (secret !== null) headers["x-postage-secret"] = secret;
  return POST(
    new Request("http://localhost/api/mail/inbound", {
      method: "POST",
      headers,
      body: JSON.stringify(message),
    })
  );
}

/// Runs one case with a variable taken away and put back, whatever the case
/// does. A test that leaves `MAIL_WEBHOOK_SECRET` unset takes every test after
/// it down with it, and the failure would point at the wrong one.
async function without(name: string, check: () => Promise<void>): Promise<void> {
  const original = process.env[name];
  delete process.env[name];
  try {
    await check();
  } finally {
    if (original !== undefined) process.env[name] = original;
  }
}

beforeEach(async () => {
  logged.length = 0;
  await reset();
  await createInbox("demo", "demo@example.com", `0x${"11".repeat(20)}`);
});

after(async () => {
  console.error = REAL_CONSOLE_ERROR;
  await chain.close();
  await model.close();
  rmSync(workspace, { recursive: true, force: true });
});

/// The defect the live outage exposed, stated as the thing that must not
/// happen again: the worker read the status, found nothing after it, and logged
/// `Gateway returned 500: ` with the reason missing.
test("a chain that will not answer says so in the body rather than sending nothing", async () => {
  chain.failing = true;
  try {
    const response = await post(MESSAGE);
    const body = await response.text();

    assert.equal(response.status, 500);
    assert.notEqual(body.length, 0, "an empty body is what left the last outage undiagnosable");
    assert.deepEqual(JSON.parse(body), {
      error: "The gateway could not read the price floor from the chain",
      fault: "chain",
    });
  } finally {
    chain.failing = false;
  }
});

test("a chain fault is logged under the stage that failed, with what the chain said", async () => {
  chain.failing = true;
  try {
    await post(MESSAGE);
  } finally {
    chain.failing = false;
  }

  const line = logged.find(([message]) => message === "inbound mail gateway fault");
  assert.ok(line, "a chain fault must still reach the log, wherever it lands among this request's log lines");
  const context = line[1] as { stage: string; reason: string };
  assert.equal(context.stage, "chain");
  assert.match(context.reason, /not accepting calls/);
});

/// The one variable read before the caller has proved anything, and so the one
/// that may not be named in the answer. `/api/world/context` states the rule:
/// the credential is checked before any other setting is read, so an anonymous
/// caller learns nothing about how this deployment is configured. This route
/// cannot check the credential first — the credential *is* the setting — so it
/// answers about the class of failure instead of the variable.
test("a webhook secret nobody set does not name itself to an anonymous caller", async () => {
  await without("MAIL_WEBHOOK_SECRET", async () => {
    const response = await post(MESSAGE);
    const body = await response.text();

    assert.equal(response.status, 500);
    assert.ok(
      !body.includes("MAIL_WEBHOOK_SECRET"),
      "an unauthenticated caller must not be told which variable is missing"
    );
    assert.deepEqual(JSON.parse(body), {
      error: "The gateway is missing a required setting",
      fault: "config",
    });
  });
});

/// The other half: withholding it from the caller cannot mean losing it. Which
/// variable is unset is the whole diagnosis and the whole fix, and the log is
/// where an operator reads it.
test("the webhook secret's name still reaches the log, where only an operator sees it", async () => {
  await without("MAIL_WEBHOOK_SECRET", async () => {
    await post(MESSAGE);

    const context = logged[0]?.[1] as { stage: string; reason: string };
    assert.equal(context.stage, "config");
    assert.match(context.reason, /MAIL_WEBHOOK_SECRET/);
  });
});

test("an app url nobody set names the variable too", async () => {
  await without("APP_URL", async () => {
    const response = await post(MESSAGE);

    assert.equal(response.status, 500);
    assert.deepEqual(await response.json(), {
      error: "APP_URL is not set on the gateway",
      fault: "config",
    });
  });
});

/// The first of the two refusals this route already answered properly. The
/// worker keys on the status, so both are pinned byte for byte rather than
/// merely "still a 4xx".
test("a wrong secret is refused exactly as it always was", async () => {
  const response = await post(MESSAGE, "wrong");

  assert.equal(response.status, 401);
  assert.deepEqual(await response.json(), { error: "Bad secret" });
});

/// The second, and the one with teeth: 404 is the only status the worker reads
/// as a real answer rather than as the gateway having fallen over, so a fault
/// must never be able to borrow it and a real unknown inbox must never lose it.
test("an unknown handle is refused exactly as it always was", async () => {
  const response = await post({ ...MESSAGE, to: "nobody@usepostage.com" });

  assert.equal(response.status, 404);
  assert.deepEqual(await response.json(), { action: "reject", reason: "unknown_inbox" });
});

test("a message the gateway can answer is untouched by any of this", async () => {
  const response = await post(MESSAGE);

  assert.equal(response.status, 200);
  assert.equal(((await response.json()) as { action: string }).action, "hold");
});

/// Anything thrown outside a stage this route knows about still has to arrive
/// as a body. A `catch` clause is handed `unknown`, so the fault that carries
/// the least information is the one most worth pinning.
test("something nobody anticipated still answers with a body rather than nothing", async () => {
  const response = faultResponse("a library threw a string");

  assert.equal(response.status, 500);
  assert.deepEqual(await response.json(), {
    error: "The gateway failed for an unexpected reason",
    fault: "unexpected",
  });
});

/// The message on the outermost error is often the least useful thing about
/// it: a database nobody can reach arrives as `fetch failed` and stops there.
test("a fault is logged with the reason under the reason, not only the top of it", () => {
  logged.length = 0;

  faultResponse(
    new Error("fetch failed", { cause: new Error("connect ECONNREFUSED 10.0.0.1:443") })
  );

  const context = logged[0]?.[1] as { reason: string };
  assert.equal(context.reason, "fetch failed: connect ECONNREFUSED 10.0.0.1:443");
});

test("a stage does not relabel a fault that already named its own", async () => {
  const inner = new GatewayFault("chain", "the chain said no");

  await assert.rejects(
    during("database", () => Promise.reject(inner)),
    (thrown: unknown) => thrown === inner
  );
});

test("a stage passes a value straight back when nothing goes wrong", async () => {
  assert.equal(await during("database", () => Promise.resolve(7)), 7);
});

/// `redact` exists because the message on a thrown value is written by whoever
/// threw it: libsql quotes the database path it could not open, viem quotes the
/// whole RPC URL it called, and a refusing upstream can echo any of it back.
/// All of that belongs in a log and none of it may carry a credential there.
test("a secret's value is taken out of a message that quoted it", () => {
  process.env.TEST_REDACT_API_KEY = "sk-live-0123456789abcdef";
  try {
    assert.equal(
      redact("call to https://rpc.example/sk-live-0123456789abcdef failed"),
      "call to https://rpc.example/[redacted TEST_REDACT_API_KEY] failed"
    );
  } finally {
    delete process.env.TEST_REDACT_API_KEY;
  }
});

test("every occurrence goes, not just the first", () => {
  process.env.TEST_REDACT_SECRET = "hunter2-hunter2";
  try {
    assert.equal(
      redact("hunter2-hunter2 then hunter2-hunter2"),
      "[redacted TEST_REDACT_SECRET] then [redacted TEST_REDACT_SECRET]"
    );
  } finally {
    delete process.env.TEST_REDACT_SECRET;
  }
});

/// A connection string is not named like a secret and routinely carries one.
test("a database url is treated as a secret even though it is not named like one", () => {
  const original = process.env.DATABASE_URL;
  process.env.DATABASE_URL = "libsql://postage-demo.turso.io";
  try {
    assert.equal(
      redact("could not open libsql://postage-demo.turso.io"),
      "could not open [redacted DATABASE_URL]"
    );
  } finally {
    process.env.DATABASE_URL = original as string;
  }
});

/// The variables saying what this deployment is, rather than what it knows,
/// stay readable: losing them would cost a diagnosis and protect nothing.
test("a public url is left alone", () => {
  const original = process.env.APP_URL;
  process.env.APP_URL = "https://postage-seven.vercel.app";
  try {
    assert.equal(
      redact("could not reach https://postage-seven.vercel.app"),
      "could not reach https://postage-seven.vercel.app"
    );
  } finally {
    process.env.APP_URL = original as string;
  }
});

test("a value too short to be a secret does not shred the message it appears in", () => {
  process.env.TEST_REDACT_TOKEN = "ab";
  try {
    assert.equal(redact("a fabulous database"), "a fabulous database");
  } finally {
    delete process.env.TEST_REDACT_TOKEN;
  }
});

test("a message with nothing to hide comes back unchanged", () => {
  assert.equal(redact("the server is not accepting queries"), "the server is not accepting queries");
});

/// The one variable in `.env.local.example` that carries a credential and was
/// not covered: a Graph Studio query URL holds its API key in the path, the
/// same shape `graph.ts` builds for the network gateway, and `GRAPH_QUERY_URL`
/// is no more named like a secret than `DATABASE_URL` is.
test("the subgraph query url is treated as a secret even though it is not named like one", () => {
  const original = process.env.GRAPH_QUERY_URL;
  process.env.GRAPH_QUERY_URL = "https://gateway.thegraph.com/api/0123456789abcdef/subgraphs/id/Qm1";
  try {
    assert.equal(
      redact("Graph query failed: https://gateway.thegraph.com/api/0123456789abcdef/subgraphs/id/Qm1"),
      "Graph query failed: [redacted GRAPH_QUERY_URL]"
    );
  } finally {
    if (original === undefined) delete process.env.GRAPH_QUERY_URL;
    else process.env.GRAPH_QUERY_URL = original;
  }
});

/// The limit the matching design actually has, pinned rather than left implied
/// by a suite that only ever feeds it names it chose to match. Coverage is by
/// name: a credential under a name holding none of the words `SECRET_NAME`
/// lists travels intact, and covering it is an edit to that pattern. Asserting
/// what it does not do is the only thing that stops the comment above it
/// drifting back into a promise it cannot keep.
test("a secret under a name the pattern does not know is left in the message", () => {
  process.env.TEST_REDACT_ENDPOINT = "https://operator:hunter2-hunter2@db.example";
  try {
    assert.equal(
      redact("could not open https://operator:hunter2-hunter2@db.example"),
      "could not open https://operator:hunter2-hunter2@db.example"
    );
  } finally {
    delete process.env.TEST_REDACT_ENDPOINT;
  }
});

/// The second limit: the match is on the literal value. A dependency that
/// percent-encodes the URL it failed on has written a different string, and
/// `replaceAll` does not see through the transformation.
test("a value the message re-encoded on its way in is not caught", () => {
  process.env.TEST_REDACT_SECRET = "hunter2/hunter2";
  try {
    assert.equal(
      redact("POST https://rpc.example/hunter2%2Fhunter2 failed"),
      "POST https://rpc.example/hunter2%2Fhunter2 failed"
    );
  } finally {
    delete process.env.TEST_REDACT_SECRET;
  }
});

import assert from "node:assert/strict";
import { after, test } from "node:test";
import { startBrokenDatabase } from "../../../../../test/database";

/// A file of its own, and not a case inside `faults.test.ts`, because the
/// database client keeps the connection it opened: once any query has
/// succeeded, nothing this process can do to `DATABASE_URL` makes the next one
/// fail. The outage has to be true from the first query, which means from
/// before the route is imported - and `node --test` gives each file its own
/// process, so this is the seam.
const database = await startBrokenDatabase();

process.env.MAIL_WEBHOOK_SECRET = "secret";
process.env.APP_URL = "http://localhost";

const { POST } = await import("./route");

const REAL_CONSOLE_ERROR = console.error;
const logged: unknown[][] = [];
console.error = (...line: unknown[]) => {
  logged.push(line);
};

after(async () => {
  console.error = REAL_CONSOLE_ERROR;
  await database.close();
});

function post(): Promise<Response> {
  return POST(
    new Request("http://localhost/api/mail/inbound", {
      method: "POST",
      headers: { "Content-Type": "application/json", "x-postage-secret": "secret" },
      body: JSON.stringify({
        from: "someone@nowhere.example",
        to: "demo@usepostage.com",
        subject: "hello",
        body: "x",
        spf: "pass",
        dkim: "pass",
        dmarc: "pass",
      }),
    })
  );
}

/// The exact production fault, reproduced: the database refuses, and the answer
/// has to carry a reason rather than the zero bytes the worker logged.
test("a database that will not answer is a 500 that says which part fell over", async () => {
  const response = await post();
  const body = await response.text();

  assert.equal(response.status, 500);
  assert.notEqual(body.length, 0, "an empty body is what left the last outage undiagnosable");
  assert.deepEqual(JSON.parse(body), {
    error: "The gateway could not reach its database",
    fault: "database",
  });
});

/// Diagnosable is not the same as talkative. The answer goes back over the wire
/// to whoever called the route, so what the database itself said - which can
/// quote a path, a URL, or a whole upstream response body - stays out of it.
test("the answer to a database fault quotes neither the connection nor its credential", async () => {
  const body = await (await post()).text();

  assert.ok(!body.includes(database.url), "the connection string must not travel in a response");
  assert.ok(!body.includes(database.authToken), "the credential must not travel in a response");
});

/// The other half: the reason the response withholds still has to reach
/// whoever operates this, because the function log is the only place it is
/// ever going to be read.
test("a database fault is logged under its stage, with what the database actually said", async () => {
  logged.length = 0;

  await post();

  const [line] = logged;
  assert.equal(line?.[0], "inbound mail gateway fault");
  const context = line?.[1] as { stage: string; reason: string };
  assert.equal(context.stage, "database");
  assert.match(context.reason, /not accepting queries/);
});

test("nothing logged about a database fault carries the credential either", async () => {
  logged.length = 0;

  await post();

  assert.ok(!JSON.stringify(logged).includes(database.authToken));
});

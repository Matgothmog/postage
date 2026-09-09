import assert from "node:assert/strict";
import { createClient } from "@libsql/client";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { SCHEMA } from "./schema";

/// A cold start against a legacy `inboxes` table — the same missing-column
/// shape that took production down on `claim_sends`, reproduced here since
/// `inboxes.wallet` is new on this branch and has no live legacy instance to
/// read back — and a file of its own to hold it. `db()` keeps the client it
/// opened, so the starting schema a test needs has to be true before
/// anything imports `client.ts` — `node --test` gives each file its own
/// process, the same seam `migrations.test.ts` and `database-fault.test.ts`
/// were split out for.
const workspace = mkdtempSync(join(tmpdir(), "postage-bootstrap-legacy-"));
const url = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_URL = url;
process.env.DATABASE_AUTH_TOKEN = "";

/// Built on a raw connection rather than through `db()`, which applies the
/// current schema and would hand back a table that already has every column.
const setup = createClient({ url });

// Every other table at its current shape. Skipping each statement that names
// `inboxes` leaves out its index as well — a deployment predating `wallet`
// could not have had an index over that column either.
for (const statement of SCHEMA) {
  if (statement.includes("inboxes")) continue;
  await setup.execute(statement);
}

// The gap the live database has: `handle` and `destination` only, the shape
// `inboxes` had before `wallet` was ever added to it.
await setup.execute(`CREATE TABLE inboxes (
  handle TEXT PRIMARY KEY,
  destination TEXT NOT NULL,
  created_at INTEGER NOT NULL
)`);
setup.close();

const { bootstrap, db } = await import("./client");
const { createInbox, inboxByHandle, inboxByWallet } = await import("./inboxes");

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

/// The same shape as the outage, in one assertion, on `inboxes` rather than
/// the `claim_sends` table that actually failed. Applying the schema before
/// migrating puts `CREATE INDEX ... ON inboxes (wallet)` in front of the
/// `ALTER TABLE` that adds the column, so this rejects with `no such column:
/// wallet` and every query in the process rejects with it — including the
/// ones that would have repaired the database.
test("db() brings up a database whose inboxes table predates the wallet column", async () => {
  const client = await db();

  const columns = await client.execute(`PRAGMA table_info(inboxes)`);
  assert.ok(
    columns.rows.some((row) => String(row.name) === "wallet"),
    "the migration must have run against the legacy table"
  );
});

/// The other half of the ordering: healing the table cannot come at the cost of
/// the index the schema declares over the column it just added.
test("the wallet index is created once the legacy table has been healed", async () => {
  const client = await db();

  const indexes = await client.execute({
    sql: `SELECT name FROM sqlite_master WHERE type = ? AND name = ?`,
    args: ["index", "inboxes_by_wallet"],
  });

  assert.equal(indexes.rows.length, 1, "the index has to be created, just later than it was");
});

test("inboxes queries naming wallet answer against a healed legacy database", async () => {
  await createInbox("demo", "owner@example.com", "0xABC");

  const byHandle = await inboxByHandle("demo");
  assert.equal(byHandle?.wallet, "0xabc", "createInbox and inboxByHandle must round-trip the wallet");

  const byWallet = await inboxByWallet("0xabc");
  assert.equal(byWallet?.handle, "demo", "inboxByWallet must find the row it was just written to");
});

test("bootstrapping an already-healed legacy database again does not error", async () => {
  const client = await db();

  await assert.doesNotReject(
    () => bootstrap(client),
    "every cold start after the first one runs this against a database it already repaired"
  );
});

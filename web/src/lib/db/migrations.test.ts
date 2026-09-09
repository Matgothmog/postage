import assert from "node:assert/strict";
import { type Client, createClient } from "@libsql/client";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { SCHEMA } from "./schema";

/// The same missing-column shape as the outage, on `inboxes` rather than the
/// `claim_sends` table that actually failed: a table created before `wallet`
/// was ever added to it — `handle` and `destination` only, the shape the
/// table had before `wallet` was introduced alongside them.
const LEGACY_INBOXES = `CREATE TABLE inboxes (
  handle TEXT PRIMARY KEY,
  destination TEXT NOT NULL,
  created_at INTEGER NOT NULL
)`;

/// A raw connection of its own, opened directly against the table this file
/// builds by hand rather than through `client.ts`'s `db()` — which always
/// runs the *current* `SCHEMA`, so a table it creates already has every
/// column. The only way to reproduce that shape (a table made before a
/// column existed) is to create it ourselves and hand it straight to
/// `addMissingColumns`, the same way `database-fault.test.ts` gets its own
/// file: `node --test` gives each file its own process, and this one needs a
/// database no other test has touched yet.
const workspace = mkdtempSync(join(tmpdir(), "postage-migrations-"));
const url = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_URL = url;
process.env.DATABASE_AUTH_TOKEN = "";

const rawClient = createClient({ url });

// `addMissingColumns` walks every table `ADDED_COLUMNS` names, `inboxes`
// among them, so every one of those tables has to exist before it runs.
// Everything except `inboxes` is created at its current, already-migrated
// shape straight from `SCHEMA` — only `inboxes` is built by hand below, in
// the shape it had before `wallet` existed on it.
for (const statement of SCHEMA) {
  if (statement.includes("inboxes")) continue;
  await rawClient.execute(statement);
}

await rawClient.execute(LEGACY_INBOXES);

const startingColumns = await rawClient.execute(`PRAGMA table_info(inboxes)`);
assert.ok(
  !startingColumns.rows.some((row) => String(row.name) === "wallet"),
  "test setup: the table must start without wallet, the shape a real deployment's did"
);

const { addMissingColumns } = await import("./migrations");

/// Databases the cases below build for themselves, cleaned up together. The
/// connection above is repaired by the first test that touches it, and a race
/// is only a race against a column that is still missing.
const spares: { client: Client; workspace: string }[] = [];

/// A database built the same way as the one above: every table at its
/// current shape except `inboxes`, which predates `wallet`. `absentTable`
/// leaves one table out entirely, for the case where a phase mistake means
/// the migration meets a table nothing has created.
async function legacyDatabase(absentTable?: string): Promise<Client> {
  const directory = mkdtempSync(join(tmpdir(), "postage-migrations-spare-"));
  const spare = createClient({ url: `file:${join(directory, "test.db")}` });
  spares.push({ client: spare, workspace: directory });

  for (const statement of SCHEMA) {
    if (statement.includes("inboxes")) continue;
    if (absentTable && statement.includes(absentTable)) continue;
    await spare.execute(statement);
  }
  await spare.execute(LEGACY_INBOXES);
  return spare;
}

after(async () => {
  rawClient.close();
  rmSync(workspace, { recursive: true, force: true });
  for (const spare of spares) {
    spare.client.close();
    rmSync(spare.workspace, { recursive: true, force: true });
  }
});

test("addMissingColumns adds inboxes.wallet to a table that predates the column", async () => {
  await addMissingColumns(rawClient);

  const columns = await rawClient.execute(`PRAGMA table_info(inboxes)`);
  const wallet = columns.rows.find((row) => String(row.name) === "wallet");

  assert.ok(wallet, "wallet must exist once the migration has run");
  assert.equal(Number(wallet?.notnull), 0, "wallet must stay nullable, matching schema.ts");
  assert.equal(wallet?.dflt_value, null, "wallet must have no default, matching schema.ts");
});

/// One cold start after another, which is the easy half. It says nothing about
/// two at once — see the racing case below, which is the half that bit.
test("addMissingColumns does not error the second time it runs", async () => {
  await addMissingColumns(rawClient);

  await assert.doesNotReject(
    () => addMissingColumns(rawClient),
    "a migration that already ran must be safe to run again on cold start"
  );
});

test("inboxes.ts's queries that reference wallet succeed once the column has been added", async () => {
  await addMissingColumns(rawClient);

  const { createInbox, inboxByHandle, inboxByWallet } = await import("./inboxes");

  await createInbox("demo", "owner@example.com", "0xabc");

  const byHandle = await inboxByHandle("demo");
  assert.equal(byHandle?.wallet, "0xabc", "createInbox and inboxByHandle must round-trip the wallet");

  const byWallet = await inboxByWallet("0xabc");
  assert.equal(byWallet?.handle, "demo", "inboxByWallet must find the row it was just written to");
});

/// The migration now runs on the critical path of every cold start, and a
/// deploy brings several instances up at once. Both read `PRAGMA table_info`,
/// both see the column missing, and both `ALTER` — so the loser meets
/// `duplicate column name` and, unguarded, takes down whatever request it was
/// booting for. It self-heals on the next request, which is no comfort to the
/// message that was in flight during the deploy.
test("two cold starts racing to add the same column both come up", async () => {
  const racing = await legacyDatabase();

  await assert.doesNotReject(
    () => Promise.all([addMissingColumns(racing), addMissingColumns(racing)]),
    "a deploy boots several instances at once and only one of them can win the ALTER"
  );

  const columns = await racing.execute(`PRAGMA table_info(inboxes)`);
  assert.ok(
    columns.rows.some((row) => String(row.name) === "wallet"),
    "the column still has to be there once the race is over"
  );
});

/// A table that lands in the wrong bootstrap phase reaches this with nothing
/// created yet, and `PRAGMA table_info` answers a table that does not exist
/// with no rows rather than an error. Falling through to `ALTER TABLE` then
/// throws `no such table`, which rejects `db()` on every cold start forever —
/// the outage again, this time with nothing that heals it. `inbox_claims` is
/// named in `ADDED_COLUMNS` before `inboxes`, so the cost of getting this
/// wrong is every table after it, not just the one.
test("a table nothing has created costs its own columns and no others", async () => {
  const partial = await legacyDatabase("inbox_claims");
  const complained: unknown[][] = [];
  const realConsoleError = console.error;
  console.error = (...line: unknown[]) => {
    complained.push(line);
  };

  try {
    await assert.doesNotReject(
      () => addMissingColumns(partial),
      "one table nobody created must not cost every table listed after it"
    );
  } finally {
    console.error = realConsoleError;
  }

  const columns = await partial.execute(`PRAGMA table_info(inboxes)`);
  assert.ok(
    columns.rows.some((row) => String(row.name) === "wallet"),
    "the tables after the absent one still have to be migrated"
  );
  assert.match(
    JSON.stringify(complained),
    /inbox_claims/,
    "a skipped table has to be said out loud, or it is a silent half-migration"
  );
});

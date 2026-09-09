import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { TABLE_STATEMENTS } from "./schema";

/// The case that stops the obvious fix. Migrating before the schema runs would
/// heal a legacy database and break this one, where `ALTER TABLE` names a table
/// nothing has created yet — so an empty database needs a cold start of its own
/// to prove it still comes up. A file of its own for the same reason
/// `bootstrap-legacy.test.ts` has one: `db()` keeps the client it opened, and
/// `node --test` gives each file its own process.
const workspace = mkdtempSync(join(tmpdir(), "postage-bootstrap-fresh-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";

const { bootstrap, db } = await import("./client");

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

test("db() creates every table the schema declares on an empty database", async () => {
  const client = await db();

  const tables = await client.execute({
    sql: `SELECT name FROM sqlite_master WHERE type = ? AND name NOT LIKE 'sqlite_%'`,
    args: ["table"],
  });

  assert.equal(
    tables.rows.length,
    TABLE_STATEMENTS.length,
    "one table per CREATE TABLE in the schema, or a phase of the bootstrap did not run"
  );
});

/// The third phase is the one that only runs after the migration, so a fresh
/// database is where it would be quietly skipped.
test("db() creates the schema's indexes on an empty database too", async () => {
  const client = await db();

  const indexes = await client.execute({
    sql: `SELECT name FROM sqlite_master WHERE type = ? AND name = ?`,
    args: ["index", "inboxes_by_wallet"],
  });

  assert.equal(indexes.rows.length, 1);
});

test("bootstrapping a database that was just created does not error", async () => {
  const client = await db();

  await assert.doesNotReject(
    () => bootstrap(client),
    "the second cold start runs this again over everything the first one made"
  );
});

import { type Client, createClient } from "@libsql/client";
import { addMissingColumns } from "./migrations";
import { COLUMN_DEPENDENT_STATEMENTS, TABLE_STATEMENTS } from "./schema";

/// Re-exported rather than defined here: pure-logic modules with no database
/// need it too, and pulling this file in for a timestamp would drag a libsql
/// connection into places — client components among them — that have no
/// business opening one.
export { now } from "../time";

/// Brings a database up to the shape the code expects, in the one order that
/// works for a database created today *and* one created before a column it now
/// has existed.
///
/// The three phases cannot be collapsed into two. Running the whole schema
/// first and migrating afterwards — the order this had — leaves a legacy
/// database permanently unopenable: `CREATE INDEX ... ON claim_sends
/// (wallet, sent_at)` throws `no such column: wallet` against a table that
/// predates the column, before the migration that would have added it ever
/// runs, so `db()` rejects on every cold start and the repair it needs can
/// never happen — this is what took production down. `inboxes (wallet)` hits
/// the same shape and is what the tests below exercise. Migrating first
/// instead breaks the other case, where `ALTER TABLE` names a table nothing
/// has created yet.
///
/// Exported so a test can run it a second time over a database `db()` has
/// already brought up. Every real caller is a cold start and `db()` keeps the
/// client it opened, so one process never reaches this twice on its own.
export async function bootstrap(client: Client): Promise<void> {
  for (const statement of TABLE_STATEMENTS) await client.execute(statement);
  await addMissingColumns(client);
  for (const statement of COLUMN_DEPENDENT_STATEMENTS) await client.execute(statement);
}

/// The one connection every table module in this directory queries through,
/// opened and set up once on first use.
let ready: Promise<Client> | null = null;

export function db(): Promise<Client> {
  if (ready) return ready;
  // Cleared on failure, or one unreachable moment at cold start would reject
  // every query for the life of the process, long after the database recovered.
  ready = (async () => {
    const client = createClient({
      url: process.env.DATABASE_URL ?? "file:.data/postage.db",
      authToken: process.env.DATABASE_AUTH_TOKEN,
    });
    try {
      await bootstrap(client);
    } catch (cause) {
      // The connection opened even though setting it up did not, so it has to
      // be given back rather than left for the retry to leak one per attempt.
      client.close();
      throw cause;
    }
    return client;
  })();
  ready.catch(() => {
    ready = null;
  });
  return ready;
}

export async function all<T>(sql: string, args: unknown[] = []): Promise<T[]> {
  const client = await db();
  const result = await client.execute({ sql, args: args as never });
  return result.rows as unknown as T[];
}

export async function run(sql: string, args: unknown[] = []): Promise<void> {
  const client = await db();
  await client.execute({ sql, args: args as never });
}

/// Empties every table. Test support: the gate's invariants are about what one
/// clearing leaves behind, which can only be asserted from a known-empty start.
///
/// No route calls this today, but it is exported from a module the shipped
/// app imports, so nothing stops a future route from reaching it by mistake.
/// `NODE_ENV` is `"production"` for exactly the builds that matter here —
/// `next build` and `next start` set it, unconditionally, and it stays unset
/// under the test runner — so this is the same class of guard ORMs put in
/// front of destructive dev commands, keyed on the one signal that survives
/// however the app is actually deployed.
export async function reset(): Promise<void> {
  if (process.env.NODE_ENV === "production") {
    throw new Error("reset() empties every table and is refused in production");
  }
  const client = await db();
  const tables = await client.execute({
    sql: `SELECT name FROM sqlite_master WHERE type = ? AND name NOT LIKE 'sqlite_%'`,
    args: ["table"],
  });
  for (const row of tables.rows) {
    await client.execute(`DELETE FROM ${String(row.name)}`);
  }
}

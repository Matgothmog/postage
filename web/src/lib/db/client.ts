import { type Client, createClient } from "@libsql/client";
import { addMissingColumns } from "./migrations";
import { SCHEMA } from "./schema";

/// Re-exported rather than defined here: pure-logic modules with no database
/// need it too, and pulling this file in for a timestamp would drag a libsql
/// connection into places — client components among them — that have no
/// business opening one.
export { now } from "../time";

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
      for (const statement of SCHEMA) await client.execute(statement);
      await addMissingColumns(client);
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

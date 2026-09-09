import type { Client } from "@libsql/client";

/// Columns for a table that already exists somewhere without them.
///
/// `CREATE TABLE IF NOT EXISTS` does nothing at all to a table that is already
/// there, so every column added after a database was first created is a column
/// that database never gets. `held_until` is the one that bit: a deployment whose
/// `challenges` table predates holding kept answering every held message with a
/// 500, because the statement meant to erase expired holds named a column it did
/// not have. Adding a column is not optional work to be done by hand later.
///
/// Every column here is also in `SCHEMA` in `schema.ts`, so a database created
/// today is correct without running any of this. Listing it twice is the price of
/// the two cases being genuinely different: one describes the shape, the other
/// repairs a database that was made before the shape said so.
const ADDED_COLUMNS: { table: string; column: string; type: string; backfill?: string }[] = [
  {
    table: "challenges",
    column: "entitled_at",
    type: "INTEGER",
    // Anything already settled has had whatever it was going to get.
    backfill: `UPDATE challenges SET entitled_at = resolved_at WHERE resolved_at IS NOT NULL`,
  },
  { table: "challenges", column: "settled_by", type: "TEXT" },
  {
    table: "challenges",
    column: "delivered_at",
    type: "INTEGER",
    // Backfilled together with entitled_at, because a challenge settled before
    // either column existed has to read as closed on both counts. Stamping one
    // alone tells a legacy sender their message never arrived and then refuses
    // the paste box the answer sends them to.
    backfill: `UPDATE challenges SET delivered_at = resolved_at WHERE resolved_at IS NOT NULL`,
  },
  { table: "challenges", column: "held_until", type: "INTEGER" },
  { table: "inbox_claims", column: "cf_checked_at", type: "INTEGER" },
  { table: "inbox_claims", column: "cf_checks", type: "INTEGER NOT NULL DEFAULT 0" },
  // Left nullable: rows written before this cannot say which wallet started
  // them, and guessing would throttle a wallet for somebody else's claim.
  { table: "claim_sends", column: "wallet", type: "TEXT" },
  // No backfill: a database from before this column existed has no record of
  // which of its NULL-`uses_left` rows were purely earned versus paid for and
  // only ever extended, and there is no way to recover that after the fact —
  // see `EARNED_PASS_CONDITION` in `passes.ts`. Left NULL, the exposure is
  // bounded to whatever passes happen to be live the moment this migration
  // runs, not a standing gap; guessing "paid" for everything would instead
  // make every pre-existing earned pass permanently unrevokable.
  { table: "passes", column: "paid_extended_at", type: "INTEGER" },
  // Left nullable: a database from before this column existed has no record of
  // which wallet claimed a handle, and guessing one would send someone else's
  // earnings to the wrong owner.
  { table: "inboxes", column: "wallet", type: "TEXT" },
];

/// SQLite's own words for a column that is already there. The whole of the
/// bootstrap is `IF NOT EXISTS` except `ALTER TABLE ADD COLUMN`, which has no
/// such form, so this string is what stands in for one.
const DUPLICATE_COLUMN = /duplicate column name/i;

/// Whether a table already has the column, told apart from every other way an
/// `ALTER TABLE` can fail. Matched on the message rather than on the error
/// code, which `no such table` shares — and that one has to stay loud, because
/// it means a table reached this before anything created it.
function isDuplicateColumn(cause: unknown): boolean {
  return cause instanceof Error && DUPLICATE_COLUMN.test(cause.message);
}

/// The columns a table has, or an empty set if there is no such table —
/// `PRAGMA table_info` answers a name it does not know with no rows rather
/// than an error, so the two cases arrive looking identical and are separated
/// by the caller.
async function existingColumns(client: Client, table: string): Promise<Set<string>> {
  const existing = await client.execute(`PRAGMA table_info(${table})`);
  if (existing.rows.length === 0) {
    console.error("schema migration skipped a table nothing has created", { table });
  }
  return new Set(existing.rows.map((row) => String(row.name)));
}

/// Adds one column, treating a column that is already there as done.
///
/// Every instance of a deploy cold-starts at once and every one of them runs
/// this, so two can read `PRAGMA table_info` before either has altered
/// anything: both see the column missing, both alter, and the loser is handed
/// `duplicate column name`. That used to reject `db()` and 500 whatever request
/// woke the instance — self-healing on the next one, which is no help to the
/// message that was in flight during the deploy. Nothing else is swallowed.
async function addColumn(client: Client, table: string, column: string, type: string): Promise<void> {
  try {
    await client.execute(`ALTER TABLE ${table} ADD COLUMN ${column} ${type}`);
  } catch (cause) {
    if (!isDuplicateColumn(cause)) throw cause;
  }
}

export async function addMissingColumns(client: Client): Promise<void> {
  const known = new Map<string, Set<string>>();

  for (const { table, column, type, backfill } of ADDED_COLUMNS) {
    let columns = known.get(table);
    if (!columns) {
      columns = await existingColumns(client, table);
      known.set(table, columns);
    }
    // A table with no columns is a table that does not exist yet — a statement
    // `createsTable` in `schema.ts` misread, so it runs after this instead of
    // before it. Skipping costs that one table its added columns; falling
    // through to `ALTER TABLE` would throw `no such table` and cost every cold
    // start from here on, with nothing left able to repair it.
    if (columns.size === 0) continue;
    if (columns.has(column)) continue;

    await addColumn(client, table, column, type);
    // Run even when the column was already there a moment ago: the instance
    // that won the race may not have reached its own backfill yet, and every
    // one of these is an idempotent UPDATE over rows that are already settled.
    if (backfill) await client.execute(backfill);
    columns.add(column);
  }
}

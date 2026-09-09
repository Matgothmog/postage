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
];

export async function addMissingColumns(client: Client): Promise<void> {
  const known = new Map<string, Set<string>>();

  for (const { table, column, type, backfill } of ADDED_COLUMNS) {
    let columns = known.get(table);
    if (!columns) {
      const existing = await client.execute(`PRAGMA table_info(${table})`);
      columns = new Set(existing.rows.map((row) => String(row.name)));
      known.set(table, columns);
    }
    if (columns.has(column)) continue;

    await client.execute(`ALTER TABLE ${table} ADD COLUMN ${column} ${type}`);
    if (backfill) await client.execute(backfill);
    columns.add(column);
  }
}

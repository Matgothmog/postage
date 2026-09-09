/// The shape of the database, applied on every cold start. `IF NOT EXISTS`
/// throughout, so this is the whole story only for a database created today —
/// see `migrations.ts` for what an older one is missing.
export const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS inboxes (
     handle TEXT PRIMARY KEY,
     destination TEXT NOT NULL,
     wallet TEXT,
     created_at INTEGER NOT NULL
   )`,
  `CREATE INDEX IF NOT EXISTS inboxes_by_wallet ON inboxes (wallet)`,
  /// A sender's permission to reach an inbox, and it runs out. Proving
  /// personhood buys a short window rather than a standing welcome, because
  /// the proof says a person was there a moment ago, not that this address
  /// belongs to one. Paying buys exactly one delivery.
  ///
  /// `paid_extended_at` is set the moment a payment stretches an
  /// already-unlimited window rather than buying a count — the one case
  /// where money lands on a row and `uses_left` stays NULL regardless, since
  /// there is no count to add a use to. It is what `EARNED_PASS_CONDITION`
  /// (`passes.ts`) checks alongside `uses_left` to tell a window nobody ever
  /// paid for from one that has, permanently, once set.
  `CREATE TABLE IF NOT EXISTS passes (
     handle TEXT NOT NULL,
     sender TEXT NOT NULL,
     reason TEXT NOT NULL,
     expires_at INTEGER NOT NULL,
     uses_left INTEGER,
     created_at INTEGER NOT NULL,
     paid_extended_at INTEGER,
     PRIMARY KEY (handle, sender)
   )`,
  /// A handle someone is part way through claiming. It becomes a row in
  /// `inboxes` only once they have proved they can read the address they are
  /// pointing it at, so an unfinished claim forwards nothing.
  `CREATE TABLE IF NOT EXISTS inbox_claims (
     handle TEXT PRIMARY KEY,
     destination TEXT NOT NULL,
     wallet TEXT NOT NULL,
     code_hash TEXT NOT NULL,
     expires_at INTEGER NOT NULL,
     attempts INTEGER NOT NULL DEFAULT 0,
     code_verified_at INTEGER,
     cf_address_id TEXT,
     cf_verified_at INTEGER,
     cf_checked_at INTEGER,
     cf_checks INTEGER NOT NULL DEFAULT 0,
     created_at INTEGER NOT NULL
   )`,
  /// One row per message we paid a model to read. Kept only long enough to cap
  /// what a flood can cost.
  `CREATE TABLE IF NOT EXISTS classifications (
     id INTEGER PRIMARY KEY AUTOINCREMENT,
     handle TEXT NOT NULL,
     sender TEXT NOT NULL,
     at INTEGER NOT NULL
   )`,
  `CREATE INDEX IF NOT EXISTS classifications_recent ON classifications (handle, at)`,
  /// The purge filters on age alone, which the composite index above cannot
  /// answer without reading the table.
  `CREATE INDEX IF NOT EXISTS classifications_by_age ON classifications (at)`,
  /// One row per claim anyone has started, kept only long enough to throttle on.
  /// `inbox_claims` cannot answer either question it exists for: it holds one
  /// row per handle, so it cannot say how often a mailbox has been asked to
  /// confirm, and it loses the row entirely once the claim goes live.
  `CREATE TABLE IF NOT EXISTS claim_sends (
     id INTEGER PRIMARY KEY AUTOINCREMENT,
     destination TEXT NOT NULL,
     wallet TEXT,
     sent_at INTEGER NOT NULL
   )`,
  `CREATE INDEX IF NOT EXISTS claim_sends_by_destination ON claim_sends (destination, sent_at)`,
  `CREATE INDEX IF NOT EXISTS claim_sends_by_wallet ON claim_sends (wallet, sent_at)`,
  /// The purge filters on age alone, which neither index above can answer.
  `CREATE INDEX IF NOT EXISTS claim_sends_by_age ON claim_sends (sent_at)`,
  /// The wallet a sender last paid from, so the next message they write can be
  /// priced on what The Graph knows about them rather than as a stranger.
  `CREATE TABLE IF NOT EXISTS sender_wallets (
     sender TEXT PRIMARY KEY,
     wallet TEXT NOT NULL,
     linked_at INTEGER NOT NULL
   )`,
  /// Which sender each World ID nullifier belongs to *at the moment*.
  ///
  /// The nullifier names a person, and the primary key is what keeps one person
  /// to one free lane: the row cannot name two senders, so proving again under
  /// a second address takes the lane off the first rather than opening another.
  /// It is not a permanent bond, because the sender a proof binds to is the one
  /// on the challenge token rather than the one who took the selfie, and a
  /// permanent row would let a mailed link cost a stranger their lane for good.
  /// Nothing here identifies anyone: the nullifier is scoped to us and means
  /// nothing anywhere else.
  `CREATE TABLE IF NOT EXISTS nullifiers (
     nullifier_hash TEXT PRIMARY KEY,
     sender TEXT NOT NULL,
     claimed_at INTEGER NOT NULL
   )`,
  /// Every time a nullifier's binding moved from one sender to another.
  ///
  /// The row above holds only the current answer, and overwriting it is exactly
  /// the operation worth being able to look back at: a lane changing hands is
  /// either somebody recovering from a poisoned link or somebody working the
  /// policy, and neither is visible from a table that only ever states today.
  /// Kept in its own table rather than as columns on the row so that the whole
  /// chain survives, not just the last hop.
  ///
  /// Not purged, unlike the other append-only tables here. One row per takeover
  /// is bounded by how often bindings actually move, and volume here is the
  /// abuse signal — deleting it would erase the only reason to keep it.
  `CREATE TABLE IF NOT EXISTS nullifier_rebinds (
     id INTEGER PRIMARY KEY AUTOINCREMENT,
     nullifier_hash TEXT NOT NULL,
     from_sender TEXT NOT NULL,
     to_sender TEXT NOT NULL,
     at INTEGER NOT NULL
   )`,
  `CREATE INDEX IF NOT EXISTS nullifier_rebinds_by_nullifier ON nullifier_rebinds (nullifier_hash, id)`,
  /// A message being held, and the terms for releasing it.
  ///
  /// Nothing anyone wrote is in this table. The message itself is held by the
  /// worker that received it, so that releasing it can put the original bytes
  /// on the wire; `held_until` is only this side's record that it still exists.
  `CREATE TABLE IF NOT EXISTS challenges (
     token TEXT PRIMARY KEY,
     handle TEXT NOT NULL,
     sender TEXT NOT NULL,
     message_id TEXT NOT NULL,
     tier TEXT NOT NULL,
     amount TEXT NOT NULL,
     quote_json TEXT NOT NULL,
     held_until INTEGER,
     created_at INTEGER NOT NULL,
     resolved_at INTEGER,
     entitled_at INTEGER,
     delivered_at INTEGER,
     settled_by TEXT
   )`,
  /// Every rp_context we have signed, against the challenge it was signed for.
  ///
  /// A signed context is a bearer credential naming this relying party and this
  /// action, and the route that mints one used to forget the nonce the moment
  /// it sent it. Keeping it is what caps how many one challenge may mint and
  /// what lets a proof coming back be checked against a request we actually
  /// made.
  `CREATE TABLE IF NOT EXISTS issued_rp_contexts (
     nonce TEXT PRIMARY KEY,
     token TEXT NOT NULL,
     created_at INTEGER NOT NULL,
     expires_at INTEGER NOT NULL,
     consumed_at INTEGER
   )`,
  `CREATE INDEX IF NOT EXISTS issued_rp_contexts_by_token ON issued_rp_contexts (token, expires_at)`,
  /// The purge filters on the window's end alone, which the composite index
  /// above cannot answer without reading the table.
  `CREATE INDEX IF NOT EXISTS issued_rp_contexts_by_age ON issued_rp_contexts (expires_at)`,
];

/// Whether a statement is one of the ones that makes a table exist, which is
/// the whole basis of the split below.
///
/// Deliberately literal, and it only has to be right about the statements in
/// this file. Reading a statement as *not* creating a table is the harmless
/// mistake — it merely runs in the later phase, which is where everything that
/// is not a table belongs anyway. The mistake with teeth is a `CREATE TABLE`
/// this does not recognise: a lower-case `create table`, or a `CREATE VIRTUAL
/// TABLE`, both read as false and would have their table created *after* the
/// migration instead of before it. `addMissingColumns` guards that case rather
/// than this trying to anticipate every spelling, because the guard is cheap
/// and the failure is not: `ALTER TABLE` against a table nothing has created
/// throws `no such table`, and unlike the outage this split fixed, that one
/// never heals.
///
/// `CREATE TABLE ... AS SELECT` is the one that would fail the other way — read
/// as true, run in the first phase, selecting from columns the migration has
/// not added yet. Nothing here does that, and nothing here should.
function createsTable(statement: string): boolean {
  return statement.trimStart().startsWith("CREATE TABLE");
}

/// `SCHEMA` split at the one seam a cold start has to break on: the statements
/// that create a table, and the statements that may name a column on one.
///
/// `client.ts` runs the first list, migrates, then runs the second, because
/// neither half of that order works alone — `bootstrap` there says why. Derived
/// rather than authored as two lists, so a statement is still written once,
/// beside the note explaining it.
export const TABLE_STATEMENTS = SCHEMA.filter(createsTable);

/// Everything else `SCHEMA` holds — today every index, and anything added later
/// that is not a table. That is the safe side of the split to land on by
/// default: it runs after the migration rather than before it.
///
/// The rule this split does *not* enforce, and the one the next person actually
/// needs: **a new indexed column is two edits, not one.** Adding it to the
/// `CREATE TABLE` above is the shape a database made today gets; every database
/// made before today gets it only from an `ADDED_COLUMNS` entry in
/// `migrations.ts`. Without that entry the index below names a column half the
/// world is missing, and running late does not save it — which is exactly how
/// `claim_sends (wallet, sent_at)` took the gateway down.
export const COLUMN_DEPENDENT_STATEMENTS = SCHEMA.filter(
  (statement) => !createsTable(statement)
);

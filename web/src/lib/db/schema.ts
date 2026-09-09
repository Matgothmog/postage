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
  `CREATE TABLE IF NOT EXISTS passes (
     handle TEXT NOT NULL,
     sender TEXT NOT NULL,
     reason TEXT NOT NULL,
     expires_at INTEGER NOT NULL,
     uses_left INTEGER,
     created_at INTEGER NOT NULL,
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
];

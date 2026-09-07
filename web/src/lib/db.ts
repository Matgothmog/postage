import { type Client, createClient } from "@libsql/client";

export interface Inbox {
  handle: string;
  /// Where mail is forwarded. Verified with Cloudflare before anything is sent.
  destination: string;
  wallet: string | null;
  created_at: number;
}

const SCHEMA = [
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
     created_at INTEGER NOT NULL
   )`,
  /// Every address we have asked to confirm, kept only long enough to throttle.
  /// Claims are keyed on handle, so they cannot answer how often one mailbox
  /// has been mailed.
  `CREATE TABLE IF NOT EXISTS claim_sends (
     id INTEGER PRIMARY KEY AUTOINCREMENT,
     destination TEXT NOT NULL,
     sent_at INTEGER NOT NULL
   )`,
  `CREATE INDEX IF NOT EXISTS claim_sends_by_destination ON claim_sends (destination, sent_at)`,
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
  ///
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
     resolved_at INTEGER
   )`,
];

/// Columns added to a table that already existed somewhere.
///
/// `CREATE TABLE IF NOT EXISTS` does nothing at all to a table that is already
/// there, so every column added after a database was first created is a column
/// that database never gets. `held_until` is the one that bit: a deployment whose
/// `challenges` table predates holding kept answering every held message with a
/// 500, because the statement meant to erase expired holds named a column it did
/// not have. Adding a column is not optional work to be done by hand later.
const ADDED_COLUMNS: { table: string; column: string; type: string }[] = [
  { table: "challenges", column: "held_until", type: "INTEGER" },
];

async function addMissingColumns(client: Client): Promise<void> {
  for (const { table, column, type } of ADDED_COLUMNS) {
    const existing = await client.execute(`PRAGMA table_info(${table})`);
    if (existing.rows.some((row) => row.name === column)) continue;
    await client.execute(`ALTER TABLE ${table} ADD COLUMN ${column} ${type}`);
  }
}

let ready: Promise<Client> | null = null;

function db(): Promise<Client> {
  if (ready) return ready;
  ready = (async () => {
    const client = createClient({
      url: process.env.DATABASE_URL ?? "file:.data/postage.db",
      authToken: process.env.DATABASE_AUTH_TOKEN,
    });
    for (const statement of SCHEMA) await client.execute(statement);
    await addMissingColumns(client);
    return client;
  })();
  return ready;
}

async function all<T>(sql: string, args: unknown[] = []): Promise<T[]> {
  const client = await db();
  const result = await client.execute({ sql, args: args as never });
  return result.rows as unknown as T[];
}

async function run(sql: string, args: unknown[] = []): Promise<void> {
  const client = await db();
  await client.execute({ sql, args: args as never });
}

export async function createInbox(
  handle: string,
  destination: string,
  wallet: string | null
): Promise<void> {
  await run(
    `INSERT INTO inboxes (handle, destination, wallet, created_at) VALUES (?, ?, ?, ?)
     ON CONFLICT (handle) DO UPDATE SET destination = excluded.destination, wallet = excluded.wallet`,
    [handle.toLowerCase(), destination.toLowerCase(), wallet?.toLowerCase() ?? null, Math.floor(Date.now() / 1000)]
  );
}

export async function inboxByHandle(handle: string): Promise<Inbox | null> {
  const rows = await all<Inbox>(`SELECT * FROM inboxes WHERE handle = ?`, [handle.toLowerCase()]);
  return rows[0] ?? null;
}

export async function inboxByWallet(wallet: string): Promise<Inbox | null> {
  const rows = await all<Inbox>(`SELECT * FROM inboxes WHERE wallet = ?`, [wallet.toLowerCase()]);
  return rows[0] ?? null;
}

/// How long proving personhood keeps the gate open. Long enough to send the
/// message that was just refused, short enough that the proof is about now.
export const PASS_WINDOW_SECONDS = 15 * 60;

export interface Pass {
  reason: string;
  expires_at: number;
  uses_left: number | null;
}

/// Takes one delivery from a live pass. A pass bought by paying carries a
/// single use and is spent here; one earned by proving personhood carries none,
/// and lasts until it expires.
export async function spendPass(handle: string, sender: string): Promise<Pass | null> {
  const rows = await all<Pass>(
    `SELECT reason, expires_at, uses_left FROM passes
     WHERE handle = ? AND sender = ? AND expires_at > ? AND (uses_left IS NULL OR uses_left > 0)`,
    [handle.toLowerCase(), sender.toLowerCase(), Math.floor(Date.now() / 1000)]
  );
  const pass = rows[0];
  if (!pass) return null;

  if (pass.uses_left !== null) {
    await run(
      `UPDATE passes SET uses_left = uses_left - 1 WHERE handle = ? AND sender = ? AND uses_left > 0`,
      [handle.toLowerCase(), sender.toLowerCase()]
    );
  }
  return pass;
}

export async function grantPass(
  handle: string,
  sender: string,
  reason: string,
  usesLeft: number | null
): Promise<void> {
  const now = Math.floor(Date.now() / 1000);
  await run(
    `INSERT INTO passes (handle, sender, reason, expires_at, uses_left, created_at)
     VALUES (?, ?, ?, ?, ?, ?)
     ON CONFLICT (handle, sender) DO UPDATE SET
       reason = excluded.reason,
       expires_at = excluded.expires_at,
       uses_left = excluded.uses_left`,
    [handle.toLowerCase(), sender.toLowerCase(), reason, now + PASS_WINDOW_SECONDS, usesLeft, now]
  );
}

export interface InboxClaim {
  handle: string;
  destination: string;
  wallet: string;
  code_hash: string;
  expires_at: number;
  attempts: number;
  code_verified_at: number | null;
  cf_address_id: string | null;
  cf_verified_at: number | null;
  created_at: number;
}

export async function startClaim(
  claim: Pick<InboxClaim, "handle" | "destination" | "wallet" | "code_hash" | "expires_at" | "cf_address_id" | "cf_verified_at">
): Promise<void> {
  await run(
    `INSERT INTO inbox_claims
       (handle, destination, wallet, code_hash, expires_at, attempts, code_verified_at,
        cf_address_id, cf_verified_at, created_at)
     VALUES (?, ?, ?, ?, ?, 0, NULL, ?, ?, ?)
     ON CONFLICT (handle) DO UPDATE SET
       destination = excluded.destination,
       wallet = excluded.wallet,
       code_hash = excluded.code_hash,
       expires_at = excluded.expires_at,
       attempts = 0,
       code_verified_at = NULL,
       cf_address_id = excluded.cf_address_id,
       cf_verified_at = excluded.cf_verified_at`,
    [
      claim.handle.toLowerCase(),
      claim.destination.toLowerCase(),
      claim.wallet.toLowerCase(),
      claim.code_hash,
      claim.expires_at,
      claim.cf_address_id,
      claim.cf_verified_at,
      Math.floor(Date.now() / 1000),
    ]
  );
}

export async function recordClaimSend(destination: string): Promise<void> {
  await run(`INSERT INTO claim_sends (destination, sent_at) VALUES (?, ?)`, [
    destination.toLowerCase(),
    Math.floor(Date.now() / 1000),
  ]);
}

export async function recentClaimsTo(destination: string, windowSeconds: number): Promise<number> {
  const rows = await all<{ n: number }>(
    `SELECT COUNT(*) AS n FROM claim_sends WHERE destination = ? AND sent_at > ?`,
    [destination.toLowerCase(), Math.floor(Date.now() / 1000) - windowSeconds]
  );
  return Number(rows[0]?.n ?? 0);
}

export async function claimByHandle(handle: string): Promise<InboxClaim | null> {
  const rows = await all<InboxClaim>(`SELECT * FROM inbox_claims WHERE handle = ?`, [
    handle.toLowerCase(),
  ]);
  return rows[0] ?? null;
}

/// Takes a guess before checking the code rather than after, so concurrent
/// requests cannot all read the same count and slip past the ceiling together.
/// Returns false once the allowance is spent.
export async function consumeAttempt(handle: string, max: number): Promise<boolean> {
  const client = await db();
  const result = await client.execute({
    sql: `UPDATE inbox_claims SET attempts = attempts + 1 WHERE handle = ? AND attempts < ?`,
    args: [handle.toLowerCase(), max],
  });
  return result.rowsAffected > 0;
}

export async function markCodeVerified(handle: string): Promise<void> {
  await run(`UPDATE inbox_claims SET code_verified_at = ? WHERE handle = ? AND code_verified_at IS NULL`, [
    Math.floor(Date.now() / 1000),
    handle.toLowerCase(),
  ]);
}

export async function attachDestination(
  handle: string,
  addressId: string,
  verifiedAt: number | null
): Promise<void> {
  await run(`UPDATE inbox_claims SET cf_address_id = ?, cf_verified_at = ? WHERE handle = ?`, [
    addressId,
    verifiedAt,
    handle.toLowerCase(),
  ]);
}

/// Pinned to the address the status was read for. Without that, a slow reply
/// about one destination could stamp a claim that has since been repointed at
/// another, marking an address Cloudflare never verified as verified.
export async function markCloudflareVerified(
  handle: string,
  addressId: string,
  verifiedAt: number
): Promise<void> {
  await run(`UPDATE inbox_claims SET cf_verified_at = ? WHERE handle = ? AND cf_address_id = ?`, [
    verifiedAt,
    handle.toLowerCase(),
    addressId,
  ]);
}

/// Dropped once the handle is a real inbox, so a used code hash is not kept.
export async function clearClaim(handle: string): Promise<void> {
  await run(`DELETE FROM inbox_claims WHERE handle = ?`, [handle.toLowerCase()]);
}

export async function linkSenderWallet(sender: string, wallet: string): Promise<void> {
  await run(
    `INSERT INTO sender_wallets (sender, wallet, linked_at) VALUES (?, ?, ?)
     ON CONFLICT (sender) DO UPDATE SET wallet = excluded.wallet, linked_at = excluded.linked_at`,
    [sender.toLowerCase(), wallet.toLowerCase(), Math.floor(Date.now() / 1000)]
  );
}

export async function walletForSender(sender: string): Promise<string | null> {
  const rows = await all<{ wallet: string }>(`SELECT wallet FROM sender_wallets WHERE sender = ?`, [
    sender.toLowerCase(),
  ]);
  return rows[0]?.wallet ?? null;
}

/// Longer than the pass window on purpose. A pass measures how recently someone
/// proved they were there; a hold measures how long a person takes to read the
/// mail asking them. Anything shorter and a sender who answers over lunch finds
/// their message gone and has to write it again.
export const HOLD_SECONDS = 24 * 60 * 60;

export interface Challenge {
  token: string;
  handle: string;
  sender: string;
  message_id: string;
  tier: string;
  amount: string;
  /// Set while the worker still holds the message. Null once released, expired,
  /// or never held at all.
  held_until: number | null;
  /// The enclave-signed quote, kept so the sender can pay it from the
  /// challenge page without us re-pricing the message we no longer hold.
  quote_json: string;
  created_at: number;
  resolved_at: number | null;
}

export async function createChallenge(challenge: Omit<Challenge, "resolved_at">): Promise<void> {
  await run(
    `INSERT INTO challenges
       (token, handle, sender, message_id, tier, amount, quote_json, held_until, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    [
      challenge.token,
      challenge.handle,
      challenge.sender,
      challenge.message_id,
      challenge.tier,
      challenge.amount,
      challenge.quote_json,
      challenge.held_until,
      challenge.created_at,
    ]
  );
}

/// Takes the right to release a held message, once. The condition and the write
/// are one statement, so two requests racing on the same token cannot both come
/// away believing they may send it - only the one that changed a row may.
export async function claimHold(token: string): Promise<boolean> {
  const client = await db();
  const result = await client.execute({
    sql: `UPDATE challenges SET held_until = NULL WHERE token = ? AND held_until > ?`,
    args: [token, Math.floor(Date.now() / 1000)],
  });
  return result.rowsAffected > 0;
}

/// Forgets every hold that ran out. The message itself is dropped by the worker
/// at its deadline whatever happens here; this only clears our record that one
/// was outstanding, so a challenge page stops offering to release something that
/// is already gone.
export async function purgeExpiredHolds(): Promise<void> {
  await run(
    `UPDATE challenges SET held_until = NULL WHERE held_until IS NOT NULL AND held_until <= ?`,
    [Math.floor(Date.now() / 1000)]
  );
}

export async function challengeByToken(token: string): Promise<Challenge | null> {
  const rows = await all<Challenge>(`SELECT * FROM challenges WHERE token = ?`, [token]);
  return rows[0] ?? null;
}

export async function resolveChallenge(token: string): Promise<void> {
  await run(`UPDATE challenges SET resolved_at = ? WHERE token = ? AND resolved_at IS NULL`, [
    Math.floor(Date.now() / 1000),
    token,
  ]);
}

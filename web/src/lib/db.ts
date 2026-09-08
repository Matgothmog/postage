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
  { table: "challenges", column: "settled_by", type: "TEXT" },
  { table: "challenges", column: "delivered_at", type: "INTEGER" },
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
  if (pass.uses_left === null) return pass;

  // Spent by the same conditional update `consumeAttempt` uses, so two messages
  // arriving together cannot both read one remaining use and both be delivered.
  const client = await db();
  const spent = await client.execute({
    sql: `UPDATE passes SET uses_left = uses_left - 1
          WHERE handle = ? AND sender = ? AND uses_left > 0 AND expires_at > ?`,
    args: [handle.toLowerCase(), sender.toLowerCase(), Math.floor(Date.now() / 1000)],
  });
  return spent.rowsAffected > 0 ? pass : null;
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
  delivered_at: number | null;
  settled_by: string | null;
}

export async function createChallenge(
  challenge: Omit<Challenge, "resolved_at" | "delivered_at" | "settled_by">
): Promise<void> {
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

/// Claims a challenge, and says whether this caller is the one who got it.
///
/// The link arrives by email and opening it twice is ordinary, so the check and
/// the write have to be one statement. Two callers reading "not settled" and
/// both granting would reset the pass after the first had already spent it —
/// one payment, two deliveries.
export async function claimChallenge(token: string, settledBy: string): Promise<boolean> {
  const client = await db();
  const claimed = await client.execute({
    sql: `UPDATE challenges SET resolved_at = ?, settled_by = ?
          WHERE token = ? AND resolved_at IS NULL`,
    args: [Math.floor(Date.now() / 1000), settledBy, token],
  });
  return claimed.rowsAffected > 0;
}

/// Puts a claimed challenge back. Whatever the claim was taken for did not
/// happen, and a payment that cannot be made twice must not leave the only way
/// through it bought closed behind it.
export async function releaseChallengeClaim(token: string): Promise<void> {
  await run(`UPDATE challenges SET resolved_at = NULL WHERE token = ?`, [token]);
}

/// Gives back a use that was taken for a delivery that never happened.
///
/// Only restores a use that was actually spent. A blind increment would land on
/// whatever pass holds that row by the time it ran, so a sender who cleared a
/// second challenge while a relay was still in flight would be handed a
/// delivery nobody paid for.
export async function refundPass(handle: string, sender: string): Promise<void> {
  await run(
    `UPDATE passes SET uses_left = 1
     WHERE handle = ? AND sender = ? AND uses_left = 0 AND expires_at > ?`,
    [handle.toLowerCase(), sender.toLowerCase(), Math.floor(Date.now() / 1000)]
  );
}

/// Records that the held message actually reached the recipient, so nobody has
/// to guess afterwards. Pass state cannot answer this: a human pass from an
/// earlier message looks the same as one granted because delivery failed.
export async function markDelivered(token: string): Promise<void> {
  await run(`UPDATE challenges SET delivered_at = ? WHERE token = ?`, [
    Math.floor(Date.now() / 1000),
    token,
  ]);
}

/// Pushes a pass's expiry back out, but only once it is nearly gone. Used when
/// the gate is willing and something on our side is not, so an outage cannot
/// quietly run out the clock on someone who has already paid — while polling
/// cannot hold a window open forever either.
const EXTEND_WHEN_UNDER_SECONDS = 5 * 60;

export async function extendPassIfExpiring(handle: string, sender: string): Promise<void> {
  const now = Math.floor(Date.now() / 1000);
  await run(
    `UPDATE passes SET expires_at = ?
     WHERE handle = ? AND sender = ? AND expires_at > ? AND expires_at < ?`,
    [
      now + PASS_WINDOW_SECONDS,
      handle.toLowerCase(),
      sender.toLowerCase(),
      now,
      now + EXTEND_WHEN_UNDER_SECONDS,
    ]
  );
}

/// What one handle, and one sender writing to it, may cost in model calls per
/// hour. Reading every message is what makes the gate work, but nothing else
/// stands between a flood and an unbounded bill, because classification now
/// happens before any pass is consulted.
export const CLASSIFY_PER_HANDLE_HOURLY = 200;
export const CLASSIFY_PER_SENDER_HOURLY = 20;

/// Drops rows past the window they are counted over. Called on the way past,
/// like the held-message purge, rather than left to grow a row per message
/// forever under a COUNT that every inbound message pays for.
export async function purgeOldClassifications(): Promise<void> {
  await run(`DELETE FROM classifications WHERE at <= ?`, [
    Math.floor(Date.now() / 1000) - 60 * 60,
  ]);
}

/// Takes a slice of the hourly budget, and says whether there was one to take.
///
/// Counting and recording are one statement on purpose. Read-then-write lets a
/// burst — which is the case the budget exists for — all see room and all spend
/// it, so the real ceiling becomes the limit plus however many arrived at once.
export async function claimClassification(handle: string, sender: string): Promise<boolean> {
  const now = Math.floor(Date.now() / 1000);
  const since = now - 60 * 60;
  const client = await db();
  const claimed = await client.execute({
    sql: `INSERT INTO classifications (handle, sender, at)
          SELECT ?, ?, ?
          WHERE (SELECT COUNT(*) FROM classifications WHERE handle = ? AND at > ?)
                  < ?
            AND (SELECT COUNT(*) FROM classifications WHERE handle = ? AND sender = ? AND at > ?)
                  < ?`,
    args: [
      handle.toLowerCase(),
      sender.toLowerCase(),
      now,
      handle.toLowerCase(),
      since,
      CLASSIFY_PER_HANDLE_HOURLY,
      handle.toLowerCase(),
      sender.toLowerCase(),
      since,
      CLASSIFY_PER_SENDER_HOURLY,
    ],
  });
  return claimed.rowsAffected > 0;
}

/// Adds one paid delivery. Never reduces what is already there: an unlimited
/// window earned by proving personhood is left alone, a counted pass gains a
/// use, and a sender with neither gets one. Overwriting instead would let a
/// second payment land on a pass that already had a use and buy nothing.
export async function addPaidUse(handle: string, sender: string): Promise<void> {
  const now = Math.floor(Date.now() / 1000);
  const client = await db();
  const topped = await client.execute({
    sql: `UPDATE passes SET uses_left = uses_left + 1, expires_at = ?
          WHERE handle = ? AND sender = ? AND expires_at > ? AND uses_left IS NOT NULL`,
    args: [now + PASS_WINDOW_SECONDS, handle.toLowerCase(), sender.toLowerCase(), now],
  });
  if (topped.rowsAffected > 0) return;

  if (await hasLivePass(handle, sender)) return;
  await grantPass(handle, sender, "paid", 1);
}

/// Whether a usable pass exists, without spending it.
export async function hasLivePass(handle: string, sender: string): Promise<boolean> {
  const rows = await all(
    `SELECT 1 FROM passes
     WHERE handle = ? AND sender = ? AND expires_at > ? AND (uses_left IS NULL OR uses_left > 0)`,
    [handle.toLowerCase(), sender.toLowerCase(), Math.floor(Date.now() / 1000)]
  );
  return rows.length > 0;
}

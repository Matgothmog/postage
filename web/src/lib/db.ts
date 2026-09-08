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
     resolved_at INTEGER,
     entitled_at INTEGER,
     delivered_at INTEGER,
     settled_by TEXT
   )`,
];

/// Columns for a table that already exists somewhere without them.
///
/// `CREATE TABLE IF NOT EXISTS` does nothing at all to a table that is already
/// there, so every column added after a database was first created is a column
/// that database never gets. `held_until` is the one that bit: a deployment whose
/// `challenges` table predates holding kept answering every held message with a
/// 500, because the statement meant to erase expired holds named a column it did
/// not have. Adding a column is not optional work to be done by hand later.
///
/// Every column here is also in `SCHEMA` above, so a database created today is
/// correct without running any of this. Listing it twice is the price of the two
/// cases being genuinely different: one describes the shape, the other repairs
/// a database that was made before the shape said so.
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
];

async function addMissingColumns(client: Client): Promise<void> {
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

/// Seconds, which is what every timestamp column here holds. This was written
/// out by hand twenty-one times, and one `Date.now()` among them would have
/// stored milliseconds into a column the next query reads as seconds — an
/// expiry fifty thousand years out that nothing would ever report.
function now(): number {
  return Math.floor(Date.now() / 1000);
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

/// Empties every table. Test support: the gate's invariants are about what one
/// clearing leaves behind, which can only be asserted from a known-empty start.
export async function reset(): Promise<void> {
  const client = await db();
  const tables = await client.execute({
    sql: `SELECT name FROM sqlite_master WHERE type = ? AND name NOT LIKE 'sqlite_%'`,
    args: ["table"],
  });
  for (const row of tables.rows) {
    await client.execute(`DELETE FROM ${String(row.name)}`);
  }
}

export async function createInbox(
  handle: string,
  destination: string,
  wallet: string | null
): Promise<void> {
  await run(
    `INSERT INTO inboxes (handle, destination, wallet, created_at) VALUES (?, ?, ?, ?)
     ON CONFLICT (handle) DO UPDATE SET destination = excluded.destination, wallet = excluded.wallet`,
    [handle.toLowerCase(), destination.toLowerCase(), wallet?.toLowerCase() ?? null, now()]
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
export async function spendPass(
  handle: string,
  sender: string,
  options: { countedOnly?: boolean } = {}
): Promise<Pass | null> {
  const rows = await all<Pass>(
    `SELECT reason, expires_at, uses_left FROM passes
     WHERE handle = ? AND sender = ? AND expires_at > ?
       AND (uses_left > 0 OR (uses_left IS NULL AND ? = 0))`,
    [
      handle.toLowerCase(),
      sender.toLowerCase(),
      now(),
      options.countedOnly ? 1 : 0,
    ]
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
    args: [handle.toLowerCase(), sender.toLowerCase(), now()],
  });
  return spent.rowsAffected > 0 ? pass : null;
}

export async function grantPass(
  handle: string,
  sender: string,
  reason: string,
  usesLeft: number | null
): Promise<void> {
  const at = now();
  await run(
    `INSERT INTO passes (handle, sender, reason, expires_at, uses_left, created_at)
     VALUES (?, ?, ?, ?, ?, ?)
     ON CONFLICT (handle, sender) DO UPDATE SET
       reason = excluded.reason,
       expires_at = MAX(passes.expires_at, excluded.expires_at),
       -- Never takes away a delivery already bought. An unlimited window is
       -- more permissive than a count while it lasts, but overwriting the count
       -- with it means the payment is gone the moment the window lapses.
       uses_left = CASE
         WHEN passes.uses_left IS NOT NULL AND passes.uses_left > 0 THEN passes.uses_left
         ELSE excluded.uses_left
       END,
       created_at = excluded.created_at`,
    [handle.toLowerCase(), sender.toLowerCase(), reason, at + PASS_WINDOW_SECONDS, usesLeft, at]
  );
}

export interface InboxClaim {
  handle: string;
  destination: string;
  wallet: string;
  code_hash: string;
  /// When the emailed code stops being accepted, and until then how long the
  /// handle is held against another wallet. It is not a deadline on the claim:
  /// promotion deliberately ignores it, because the code is checked against it
  /// when it is entered and Cloudflare's own link has no deadline of ours. A
  /// claim whose code went in at minute fourteen must still go live when its
  /// owner clicks that link over lunch.
  expires_at: number;
  attempts: number;
  code_verified_at: number | null;
  cf_address_id: string | null;
  cf_verified_at: number | null;
  cf_checked_at: number | null;
  cf_checks: number;
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
       cf_verified_at = excluded.cf_verified_at,
       -- Starting again is starting again. Without this a claim that spent its
       -- budget could never be retried, only abandoned.
       cf_checked_at = NULL,
       cf_checks = 0`,
    [
      claim.handle.toLowerCase(),
      claim.destination.toLowerCase(),
      claim.wallet.toLowerCase(),
      claim.code_hash,
      claim.expires_at,
      claim.cf_address_id,
      claim.cf_verified_at,
      now(),
    ]
  );
}

export async function recordClaimSend(destination: string, wallet: string): Promise<void> {
  await run(`INSERT INTO claim_sends (destination, wallet, sent_at) VALUES (?, ?, ?)`, [
    destination.toLowerCase(),
    wallet.toLowerCase(),
    now(),
  ]);
}

/// How often this mailbox has been asked to confirm. The address is the victim
/// of an email bomb, so it is counted whoever aimed it.
export async function recentClaimsTo(destination: string, windowSeconds: number): Promise<number> {
  return await countClaims("destination", destination, windowSeconds);
}

/// How many claims this wallet has started. The destination throttle cannot see
/// this: one wallet naming a different address each time passes it every time,
/// and every claim that gets as far as a code registers a Cloudflare
/// destination, which the account has a hard cap on and no way to delete.
export async function recentClaimsFrom(wallet: string, windowSeconds: number): Promise<number> {
  return await countClaims("wallet", wallet, windowSeconds);
}

async function countClaims(
  column: "destination" | "wallet",
  value: string,
  windowSeconds: number
): Promise<number> {
  const rows = await all<{ n: number }>(
    `SELECT COUNT(*) AS n FROM claim_sends WHERE ${column} = ? AND sent_at > ?`,
    [value.toLowerCase(), now() - windowSeconds]
  );
  return Number(rows[0]?.n ?? 0);
}

/// Drops rows past the longest window anything throttles over. Called on the way
/// past, like the classification purge, rather than left to grow a row per
/// signup attempt forever under a COUNT every later attempt pays for.
export async function purgeOldClaimSends(windowSeconds: number): Promise<void> {
  await run(`DELETE FROM claim_sends WHERE sent_at <= ?`, [now() - windowSeconds]);
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
    now(),
    handle.toLowerCase(),
  ]);
}

/// Registering the address with Cloudflare answers the same question a check
/// does, so it is recorded as one. Otherwise `settleClaim`, which runs moments
/// later on both signup paths, spends a second call asking what we just learned.
export async function attachDestination(
  handle: string,
  addressId: string,
  verifiedAt: number | null
): Promise<void> {
  await run(
    `UPDATE inbox_claims
     SET cf_address_id = ?, cf_verified_at = ?, cf_checked_at = ?, cf_checks = cf_checks + 1
     WHERE handle = ?`,
    [addressId, verifiedAt, now(), handle.toLowerCase()]
  );
}

/// The least time between two questions to Cloudflare about one claim, and the
/// most questions a single claim may ever cause.
///
/// `GET /api/inbox/verify` is polled by an anonymous browser, and it used to
/// spend one Cloudflare API call per request against a limit that belongs to the
/// whole account. Anyone who knew a handle mid-claim could spend it in a loop.
///
/// The interval matches the page's own poll, so one person waiting is not slowed
/// down at all, and a thousand requests for the same claim now cost what one
/// does. The budget is what makes it bounded rather than merely slow: nobody can
/// keep a claim answering questions forever, and after it the claim simply stops
/// being asked about until it is started again.
export const CF_CHECK_INTERVAL_SECONDS = 4;
export const CF_CHECK_BUDGET = 200;

/// Takes the right to ask Cloudflare about this claim, once.
///
/// The condition and the write are one statement, so a burst of pollers cannot
/// all read the same last-checked time and all go and ask. A claim already
/// verified never needs asking again, so it is refused here rather than in the
/// caller.
export async function takeCloudflareCheck(handle: string): Promise<boolean> {
  const at = now();
  const client = await db();
  const taken = await client.execute({
    sql: `UPDATE inbox_claims
          SET cf_checked_at = ?, cf_checks = cf_checks + 1
          WHERE handle = ?
            AND cf_verified_at IS NULL
            AND cf_checks < ?
            AND (cf_checked_at IS NULL OR cf_checked_at <= ?)`,
    args: [at, handle.toLowerCase(), CF_CHECK_BUDGET, at - CF_CHECK_INTERVAL_SECONDS],
  });
  return taken.rowsAffected > 0;
}

/// Whether this claim has spent its whole budget without Cloudflare ever
/// confirming. Nothing will ask again, so the page is told rather than left
/// polling something that has stopped answering.
export async function cloudflareChecksExhausted(handle: string): Promise<boolean> {
  const rows = await all<{ cf_checks: number }>(
    `SELECT cf_checks FROM inbox_claims WHERE handle = ? AND cf_verified_at IS NULL`,
    [handle.toLowerCase()]
  );
  return rows.length > 0 && Number(rows[0].cf_checks) >= CF_CHECK_BUDGET;
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
    [sender.toLowerCase(), wallet.toLowerCase(), now()]
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
  entitled_at: number | null;
  settled_by: string | null;
}

export async function createChallenge(
  challenge: Omit<Challenge, "resolved_at" | "delivered_at" | "entitled_at" | "settled_by">
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
    args: [token, now()],
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
    [now()]
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
  const at = now();
  const client = await db();
  const claimed = await client.execute({
    // A paid claim takes the entitlement with it. Deciding that separately let
    // the loser of the race read "nobody has been entitled yet" during the
    // hundreds of milliseconds the winner spends handing the message to the
    // worker, and hand out a second delivery for one payment.
    sql: `UPDATE challenges
          SET resolved_at = ?, settled_by = ?,
              entitled_at = CASE WHEN ? = 'paid' THEN ? ELSE entitled_at END
          WHERE token = ? AND resolved_at IS NULL`,
    args: [at, settledBy, settledBy, at, token],
  });
  return claimed.rowsAffected > 0;
}

/// Puts a claimed challenge back. Whatever the claim was taken for did not
/// happen, and a payment that cannot be made twice must not leave the only way
/// through it bought closed behind it.
///
/// Gives back only the claim this caller took. A blind rollback also cleared an
/// entitlement another request had taken in the meantime, which let that request
/// take it a second time and grant a second delivery for one payment.
export async function releaseChallengeClaim(token: string, settledBy: string): Promise<void> {
  await run(
    `UPDATE challenges
     SET resolved_at = NULL, settled_by = NULL,
         entitled_at = CASE WHEN ? = 'paid' THEN NULL ELSE entitled_at END
     WHERE token = ? AND settled_by = ?`,
    [settledBy, token, settledBy]
  );
}

/// Gives back a use that was taken for a delivery that never happened.
///
/// Only restores a use that was actually spent. A blind increment would land on
/// whatever pass holds that row by the time it ran, so a sender who cleared a
/// second challenge while a relay was still in flight would be handed a
/// delivery nobody paid for.
export async function refundPass(handle: string, sender: string): Promise<void> {
  await run(
    `UPDATE passes SET uses_left = uses_left + 1
     WHERE handle = ? AND sender = ? AND uses_left IS NOT NULL AND expires_at > ?`,
    [handle.toLowerCase(), sender.toLowerCase(), now()]
  );
}

/// Records that this challenge has issued what it owed, so it cannot issue it
/// again. A payment settles onchain forever, and without this the sender could
/// spend the delivery it bought and then ask for another.
export async function markEntitled(token: string): Promise<boolean> {
  const client = await db();
  const marked = await client.execute({
    sql: `UPDATE challenges SET entitled_at = ? WHERE token = ? AND entitled_at IS NULL`,
    args: [now(), token],
  });
  return marked.rowsAffected > 0;
}

/// Records that the held message actually reached the recipient, so nobody has
/// to guess afterwards. Pass state cannot answer this: a human pass from an
/// earlier message looks the same as one granted because delivery failed.
export async function markDelivered(token: string): Promise<void> {
  await run(`UPDATE challenges SET delivered_at = ? WHERE token = ?`, [
    now(),
    token,
  ]);
}

/// Pushes a pass's expiry back out, but only once it is nearly gone. Used when
/// the gate is willing and something on our side is not, so an outage cannot
/// quietly run out the clock on someone who has already paid — while polling
/// cannot hold a window open forever either.
const EXTEND_WHEN_UNDER_SECONDS = 5 * 60;

/// The furthest a pass may be carried past when it was earned. An outage should
/// not cost someone the delivery they paid for, but a window that renews on
/// demand is not a window — the proof behind it described one moment, and this
/// is how long that moment is allowed to be stretched.
const EXTEND_NO_LATER_THAN_SECONDS = 60 * 60;

export async function extendPassIfExpiring(handle: string, sender: string): Promise<void> {
  const at = now();
  await run(
    `UPDATE passes SET expires_at = ?
     WHERE handle = ? AND sender = ? AND expires_at > ? AND expires_at < ?
       AND created_at > ?`,
    [
      at + PASS_WINDOW_SECONDS,
      handle.toLowerCase(),
      sender.toLowerCase(),
      at,
      at + EXTEND_WHEN_UNDER_SECONDS,
      at - EXTEND_NO_LATER_THAN_SECONDS,
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
    now() - 60 * 60,
  ]);
}

/// Why a message was not read, when it was not.
///
/// Which limit bit matters, because the two are caused by different people. A
/// sender can spend their own slice whenever they like, so anything it unlocks
/// is something they chose. A handle's pool is spent by whoever writes to that
/// inbox, forged addresses included, so treating it as the recipient's fault
/// hands a stranger a lever over their mail.
export type BudgetState = "spent-by-sender" | "spent-by-handle" | null;

/// Takes a slice of the hourly budget, and says whether there was one to take.
///
/// Counting and recording are one statement on purpose. Read-then-write lets a
/// burst — which is the case the budget exists for — all see room and all spend
/// it, so the real ceiling becomes the limit plus however many arrived at once.
/// That statement is also the only place the thresholds are compared, so asking
/// first would be the same rule written twice with a race between them.
export async function claimClassification(
  handle: string,
  sender: string
): Promise<BudgetState> {
  if (await takeClassificationSlot(handle, sender)) return null;
  return await whichLimitBit(handle, sender);
}

/// Which of the two ceilings refused the slot. Looked up rather than assumed:
/// reporting a sender's own exhaustion as the handle's would unlock the two
/// things that state exists to shut.
async function whichLimitBit(handle: string, sender: string): Promise<BudgetState> {
  const rows = await all<{ forSender: number }>(
    `SELECT SUM(CASE WHEN sender = ? THEN 1 ELSE 0 END) AS forSender
     FROM classifications WHERE handle = ? AND at > ?`,
    [sender.toLowerCase(), handle.toLowerCase(), now() - 60 * 60]
  );
  return Number(rows[0]?.forSender ?? 0) >= CLASSIFY_PER_SENDER_HOURLY
    ? "spent-by-sender"
    : "spent-by-handle";
}

/// Hands back the most recent slot taken by this pair, for work that never
/// reached the model.
export async function releaseClassificationSlot(handle: string, sender: string): Promise<void> {
  await run(
    `DELETE FROM classifications
     WHERE id = (SELECT id FROM classifications
                 WHERE handle = ? AND sender = ? ORDER BY at DESC, id DESC LIMIT 1)`,
    [handle.toLowerCase(), sender.toLowerCase()]
  );
}

async function takeClassificationSlot(handle: string, sender: string): Promise<boolean> {
  const at = now();
  const since = at - 60 * 60;
  const inbox = handle.toLowerCase();
  const writer = sender.toLowerCase();

  const client = await db();
  const claimed = await client.execute({
    sql: `INSERT INTO classifications (handle, sender, at)
          SELECT ?, ?, ?
          WHERE (SELECT COUNT(*) FROM classifications
                 WHERE handle = ? AND at > ?) < ?
            AND (SELECT COUNT(*) FROM classifications
                 WHERE handle = ? AND sender = ? AND at > ?) < ?`,
    args: [
      inbox, writer, at,
      inbox, since, CLASSIFY_PER_HANDLE_HOURLY,
      inbox, writer, since, CLASSIFY_PER_SENDER_HOURLY,
    ],
  });
  return claimed.rowsAffected > 0;
}

/// Adds one paid delivery. Never reduces what is already there: an unlimited
/// window earned by proving personhood is left alone, a counted pass gains a
/// use, and a sender with neither gets one. Overwriting instead would let a
/// second payment land on a pass that already had a use and buy nothing.
export async function addPaidUse(handle: string, sender: string): Promise<void> {
  const at = now();
  const client = await db();
  const topped = await client.execute({
    sql: `UPDATE passes SET uses_left = uses_left + 1, expires_at = ?
          WHERE handle = ? AND sender = ? AND expires_at > ? AND uses_left IS NOT NULL`,
    args: [at + PASS_WINDOW_SECONDS, handle.toLowerCase(), sender.toLowerCase(), at],
  });
  if (topped.rowsAffected > 0) return;

  // An unlimited window is already better than a use, so it is not replaced.
  // It is pushed out instead, because the payment has to buy something: without
  // this a sender who paid a minute before their free window lapsed would be
  // left holding nothing for money that has already left their wallet.
  const extended = await client.execute({
    sql: `UPDATE passes SET expires_at = ?
          WHERE handle = ? AND sender = ? AND expires_at > ? AND uses_left IS NULL`,
    args: [at + PASS_WINDOW_SECONDS, handle.toLowerCase(), sender.toLowerCase(), at],
  });
  if (extended.rowsAffected > 0) return;

  await grantPass(handle, sender, "paid", 1);
}

/// Whether a usable pass exists, without spending it.
export async function hasLivePass(handle: string, sender: string): Promise<boolean> {
  const rows = await all(
    `SELECT 1 FROM passes
     WHERE handle = ? AND sender = ? AND expires_at > ? AND (uses_left IS NULL OR uses_left > 0)`,
    [handle.toLowerCase(), sender.toLowerCase(), now()]
  );
  return rows.length > 0;
}

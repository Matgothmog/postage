import { type Client, createClient } from "@libsql/client";

export interface Inbox {
  handle: string;
  /// Where mail is forwarded. Verified with Cloudflare before anything is sent.
  destination: string;
  /// Wallet that earnings accrue to and that can claim them.
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
  /// Senders that cleared the gate once for this inbox and never see it again.
  `CREATE TABLE IF NOT EXISTS allowlist (
     handle TEXT NOT NULL,
     sender TEXT NOT NULL,
     reason TEXT NOT NULL,
     added_at INTEGER NOT NULL,
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
  /// The wallet a sender last paid from, so the next message they write can be
  /// priced on what The Graph knows about them rather than as a stranger.
  `CREATE TABLE IF NOT EXISTS sender_wallets (
     sender TEXT PRIMARY KEY,
     wallet TEXT NOT NULL,
     linked_at INTEGER NOT NULL
   )`,
  /// Challenges are the only thing resembling a message we keep, and they hold
  /// no content: just who was writing to whom, and what it would cost.
  `CREATE TABLE IF NOT EXISTS challenges (
     token TEXT PRIMARY KEY,
     handle TEXT NOT NULL,
     sender TEXT NOT NULL,
     message_id TEXT NOT NULL,
     tier TEXT NOT NULL,
     amount TEXT NOT NULL,
     quote_json TEXT NOT NULL,
     created_at INTEGER NOT NULL,
     resolved_at INTEGER
   )`,
];

let ready: Promise<Client> | null = null;

function db(): Promise<Client> {
  if (ready) return ready;
  ready = (async () => {
    const client = createClient({
      url: process.env.DATABASE_URL ?? "file:.data/postage.db",
      authToken: process.env.DATABASE_AUTH_TOKEN,
    });
    for (const statement of SCHEMA) await client.execute(statement);
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

export async function isAllowlisted(handle: string, sender: string): Promise<boolean> {
  const rows = await all(`SELECT 1 FROM allowlist WHERE handle = ? AND sender = ?`, [
    handle.toLowerCase(),
    sender.toLowerCase(),
  ]);
  return rows.length > 0;
}

export async function allowlist(handle: string, sender: string, reason: string): Promise<void> {
  await run(
    `INSERT OR IGNORE INTO allowlist (handle, sender, reason, added_at) VALUES (?, ?, ?, ?)`,
    [handle.toLowerCase(), sender.toLowerCase(), reason, Math.floor(Date.now() / 1000)]
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

export async function claimByHandle(handle: string): Promise<InboxClaim | null> {
  const rows = await all<InboxClaim>(`SELECT * FROM inbox_claims WHERE handle = ?`, [
    handle.toLowerCase(),
  ]);
  return rows[0] ?? null;
}

export async function countClaimAttempt(handle: string): Promise<void> {
  await run(`UPDATE inbox_claims SET attempts = attempts + 1 WHERE handle = ?`, [
    handle.toLowerCase(),
  ]);
}

export async function markCodeVerified(handle: string): Promise<void> {
  await run(`UPDATE inbox_claims SET code_verified_at = ? WHERE handle = ? AND code_verified_at IS NULL`, [
    Math.floor(Date.now() / 1000),
    handle.toLowerCase(),
  ]);
}

export async function markCloudflareVerified(handle: string, verifiedAt: number): Promise<void> {
  await run(`UPDATE inbox_claims SET cf_verified_at = ? WHERE handle = ?`, [
    verifiedAt,
    handle.toLowerCase(),
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

export interface Challenge {
  token: string;
  handle: string;
  sender: string;
  message_id: string;
  tier: string;
  amount: string;
  /// The enclave-signed quote, kept so the sender can pay it from the
  /// challenge page without us re-pricing the message we no longer hold.
  quote_json: string;
  created_at: number;
  resolved_at: number | null;
}

export async function createChallenge(challenge: Omit<Challenge, "resolved_at">): Promise<void> {
  await run(
    `INSERT INTO challenges (token, handle, sender, message_id, tier, amount, quote_json, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
    [
      challenge.token,
      challenge.handle,
      challenge.sender,
      challenge.message_id,
      challenge.tier,
      challenge.amount,
      challenge.quote_json,
      challenge.created_at,
    ]
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

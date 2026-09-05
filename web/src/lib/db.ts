import { type Client, createClient } from "@libsql/client";

export type MessageStatus = "held" | "delivered";
export type ReleaseReason = "human" | "stamp" | "known";

export interface HeldMessage {
  id: string;
  message_hash: string;
  sender: string;
  recipient_local: string;
  subject: string;
  body: string;
  status: MessageStatus;
  unlock_token: string;
  received_at: number;
  delivered_at: number | null;
  released_by: ReleaseReason | null;
}

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS inboxes (
     local_part TEXT PRIMARY KEY,
     wallet TEXT NOT NULL,
     created_at INTEGER NOT NULL
   )`,
  `CREATE TABLE IF NOT EXISTS messages (
     id TEXT PRIMARY KEY,
     message_hash TEXT NOT NULL,
     sender TEXT NOT NULL,
     recipient_local TEXT NOT NULL,
     subject TEXT NOT NULL,
     body TEXT NOT NULL,
     status TEXT NOT NULL,
     unlock_token TEXT NOT NULL UNIQUE,
     received_at INTEGER NOT NULL,
     delivered_at INTEGER,
     released_by TEXT
   )`,
  `CREATE INDEX IF NOT EXISTS messages_by_recipient ON messages (recipient_local, status)`,
  `CREATE TABLE IF NOT EXISTS known_senders (
     recipient_local TEXT NOT NULL,
     sender TEXT NOT NULL,
     PRIMARY KEY (recipient_local, sender)
   )`,
];

/// libsql speaks both `file:` and `libsql://`, so local development and the
/// deployed app run the same queries against the same engine. Only the URL
/// changes.
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

export async function claimInbox(localPart: string, wallet: string): Promise<void> {
  await run(
    `INSERT INTO inboxes (local_part, wallet, created_at) VALUES (?, ?, ?)
     ON CONFLICT (local_part) DO UPDATE SET wallet = excluded.wallet`,
    [localPart.toLowerCase(), wallet.toLowerCase(), Math.floor(Date.now() / 1000)]
  );
}

export async function walletForInbox(localPart: string): Promise<string | null> {
  const rows = await all<{ wallet: string }>(
    `SELECT wallet FROM inboxes WHERE local_part = ?`,
    [localPart.toLowerCase()]
  );
  return rows[0]?.wallet ?? null;
}

export async function isKnownSender(localPart: string, sender: string): Promise<boolean> {
  const rows = await all(
    `SELECT 1 FROM known_senders WHERE recipient_local = ? AND sender = ?`,
    [localPart.toLowerCase(), sender.toLowerCase()]
  );
  return rows.length > 0;
}

export async function rememberSender(localPart: string, sender: string): Promise<void> {
  await run(`INSERT OR IGNORE INTO known_senders (recipient_local, sender) VALUES (?, ?)`, [
    localPart.toLowerCase(),
    sender.toLowerCase(),
  ]);
}

export async function insertMessage(
  message: Omit<HeldMessage, "delivered_at" | "released_by">
): Promise<void> {
  await run(
    `INSERT INTO messages
       (id, message_hash, sender, recipient_local, subject, body, status, unlock_token, received_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    [
      message.id,
      message.message_hash,
      message.sender,
      message.recipient_local,
      message.subject,
      message.body,
      message.status,
      message.unlock_token,
      message.received_at,
    ]
  );
}

export async function messageByToken(token: string): Promise<HeldMessage | null> {
  const rows = await all<HeldMessage>(`SELECT * FROM messages WHERE unlock_token = ?`, [token]);
  return rows[0] ?? null;
}

export async function deliver(token: string, reason: ReleaseReason): Promise<void> {
  await run(
    `UPDATE messages SET status = 'delivered', delivered_at = ?, released_by = ?
     WHERE unlock_token = ? AND status = 'held'`,
    [Math.floor(Date.now() / 1000), reason, token]
  );
}

export async function inboxMessages(localPart: string): Promise<HeldMessage[]> {
  return all<HeldMessage>(
    `SELECT * FROM messages WHERE recipient_local = ? ORDER BY received_at DESC`,
    [localPart.toLowerCase()]
  );
}

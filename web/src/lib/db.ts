import { DatabaseSync } from "node:sqlite";
import { mkdirSync } from "node:fs";
import { dirname } from "node:path";

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

const DB_PATH = process.env.POSTAGE_DB_PATH ?? ".data/postage.db";

let database: DatabaseSync | null = null;

function db(): DatabaseSync {
  if (database) return database;

  mkdirSync(dirname(DB_PATH), { recursive: true });
  database = new DatabaseSync(DB_PATH);
  database.exec(`
    CREATE TABLE IF NOT EXISTS inboxes (
      local_part TEXT PRIMARY KEY,
      wallet TEXT NOT NULL,
      created_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS messages (
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
    );
    CREATE INDEX IF NOT EXISTS messages_by_recipient ON messages (recipient_local, status);
    CREATE TABLE IF NOT EXISTS known_senders (
      recipient_local TEXT NOT NULL,
      sender TEXT NOT NULL,
      PRIMARY KEY (recipient_local, sender)
    );
  `);
  return database;
}

export function claimInbox(localPart: string, wallet: string): void {
  db()
    .prepare(
      `INSERT INTO inboxes (local_part, wallet, created_at) VALUES (?, ?, ?)
       ON CONFLICT (local_part) DO UPDATE SET wallet = excluded.wallet`
    )
    .run(localPart.toLowerCase(), wallet.toLowerCase(), Math.floor(Date.now() / 1000));
}

export function walletForInbox(localPart: string): string | null {
  const row = db()
    .prepare(`SELECT wallet FROM inboxes WHERE local_part = ?`)
    .get(localPart.toLowerCase()) as { wallet: string } | undefined;
  return row?.wallet ?? null;
}

export function isKnownSender(localPart: string, sender: string): boolean {
  const row = db()
    .prepare(`SELECT 1 FROM known_senders WHERE recipient_local = ? AND sender = ?`)
    .get(localPart.toLowerCase(), sender.toLowerCase());
  return row !== undefined;
}

export function rememberSender(localPart: string, sender: string): void {
  db()
    .prepare(`INSERT OR IGNORE INTO known_senders (recipient_local, sender) VALUES (?, ?)`)
    .run(localPart.toLowerCase(), sender.toLowerCase());
}

export function insertMessage(message: Omit<HeldMessage, "delivered_at" | "released_by">): void {
  db()
    .prepare(
      `INSERT INTO messages
         (id, message_hash, sender, recipient_local, subject, body, status, unlock_token, received_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`
    )
    .run(
      message.id,
      message.message_hash,
      message.sender,
      message.recipient_local,
      message.subject,
      message.body,
      message.status,
      message.unlock_token,
      message.received_at
    );
}

export function messageByToken(token: string): HeldMessage | null {
  const row = db().prepare(`SELECT * FROM messages WHERE unlock_token = ?`).get(token);
  return (row as HeldMessage | undefined) ?? null;
}

export function deliver(token: string, reason: ReleaseReason): void {
  db()
    .prepare(
      `UPDATE messages SET status = 'delivered', delivered_at = ?, released_by = ?
       WHERE unlock_token = ? AND status = 'held'`
    )
    .run(Math.floor(Date.now() / 1000), reason, token);
}

export function inboxMessages(localPart: string): HeldMessage[] {
  return db()
    .prepare(`SELECT * FROM messages WHERE recipient_local = ? ORDER BY received_at DESC`)
    .all(localPart.toLowerCase()) as unknown as HeldMessage[];
}

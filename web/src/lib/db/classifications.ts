import { all, db, now, run } from "./client";

/// What one handle, and one sender writing to it, may cost in model calls per
/// hour. Reading every message is what makes the gate work, but nothing else
/// stands between a flood and an unbounded bill, because classification now
/// happens before any pass is consulted.
export const CLASSIFY_PER_HANDLE_HOURLY = 200;
export const CLASSIFY_PER_SENDER_HOURLY = 20;

/// The one window both hourly ceilings above are counted over.
const CLASSIFY_WINDOW_SECONDS = 60 * 60;

/// Drops rows past the window they are counted over. Called on the way past,
/// like the held-message purge, rather than left to grow a row per message
/// forever under a COUNT that every inbound message pays for.
export async function purgeOldClassifications(): Promise<void> {
  await run(`DELETE FROM classifications WHERE at <= ?`, [
    now() - CLASSIFY_WINDOW_SECONDS,
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
    [sender.toLowerCase(), handle.toLowerCase(), now() - CLASSIFY_WINDOW_SECONDS]
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
  const since = at - CLASSIFY_WINDOW_SECONDS;
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

import { all, db, now, run } from "./client";

/// What one handle, one domain writing to it, and one address at that domain
/// may cost in model calls per hour. Reading every message is what makes the
/// gate work, but nothing else stands between a flood and an unbounded bill,
/// because classification now happens before any pass is consulted.
///
/// Three ceilings rather than two, and the middle one is why. Only mail the
/// receiving server could authenticate may spend any of this, and
/// authentication is a statement about a domain — so a domain is the smallest
/// unit of a sender that cannot be cycled for nothing. A local part is free to
/// invent; a domain that passes DKIM costs a registration and a published key.
/// Keyed on the address alone, one party emptied a handle's whole hour by
/// inventing ten local parts, and everything downstream that read "this pool is
/// empty" as "somebody else's doing" was reading a state that party had
/// arranged. Keyed on the domain as well, emptying the pool takes as many
/// separate registered domains as the ratio between these two numbers.
export const CLASSIFY_PER_HANDLE_HOURLY = 200;
export const CLASSIFY_PER_DOMAIN_HOURLY = 60;
export const CLASSIFY_PER_SENDER_HOURLY = 20;

/// The one window all three hourly ceilings above are counted over.
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
/// Which ceiling bit matters, because the three are reached by different
/// people. An address's own slice is spent by that address alone, so anything
/// it unlocks is something that sender chose. A domain's slice rations a cost
/// across everyone who writes from one name — it is the unit a flood is paid
/// for in, but it is not a culprit: `gmail.com` is millions of unrelated
/// people. A handle's pool is spent by every domain writing to that inbox
/// between them.
///
/// None of the three buys a delivery — see `deliveredFree` in
/// `api/mail/inbound/forwarding.ts`, where a budget that ran out is held apart
/// from a model that failed, and only the second is an outage this gateway owes
/// anyone free mail for.
export type BudgetState = "spent-by-sender" | "spent-by-domain" | "spent-by-handle" | null;

/// Everything from the last `@` on — `"@example.com"` for `"a@example.com"`.
/// The separator is the last `@` and not the first, because a quoted local part
/// may contain one and a domain may not.
///
/// Kept as a suffix including the `@` so that the comparison below is an exact
/// tail match: `"@example.com"` cannot be reached from `"notexample.com"`, and
/// a subdomain is a different name that is counted as one.
///
/// An address with no `@` at all is its own suffix, so it buckets with itself
/// rather than being handed a domain's allowance it never named.
function domainSuffix(sender: string): string {
  const at = sender.lastIndexOf("@");
  return at === -1 ? sender : sender.slice(at);
}

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

/// Which of the three ceilings refused the slot, narrowest first. Looked up
/// rather than assumed: reporting a sender's own exhaustion as the handle's
/// would unlock the things that state exists to shut.
async function whichLimitBit(handle: string, sender: string): Promise<BudgetState> {
  const writer = sender.toLowerCase();
  const domain = domainSuffix(writer);
  const rows = await all<{ forSender: number; forDomain: number }>(
    `SELECT SUM(CASE WHEN sender = ? THEN 1 ELSE 0 END) AS forSender,
            SUM(CASE WHEN substr(sender, -length(?)) = ? THEN 1 ELSE 0 END) AS forDomain
     FROM classifications WHERE handle = ? AND at > ?`,
    [writer, domain, domain, handle.toLowerCase(), now() - CLASSIFY_WINDOW_SECONDS]
  );

  const row = rows[0];
  if (Number(row?.forSender ?? 0) >= CLASSIFY_PER_SENDER_HOURLY) return "spent-by-sender";
  if (Number(row?.forDomain ?? 0) >= CLASSIFY_PER_DOMAIN_HOURLY) return "spent-by-domain";
  return "spent-by-handle";
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
  const domain = domainSuffix(writer);

  const client = await db();
  const claimed = await client.execute({
    // The per-domain count is a tail comparison rather than a stored column:
    // the domain is derivable from what is already recorded, and the rows this
    // reads are one handle's single hour, which the purge keeps small.
    sql: `INSERT INTO classifications (handle, sender, at)
          SELECT ?, ?, ?
          WHERE (SELECT COUNT(*) FROM classifications
                 WHERE handle = ? AND at > ?) < ?
            AND (SELECT COUNT(*) FROM classifications
                 WHERE handle = ? AND at > ? AND substr(sender, -length(?)) = ?) < ?
            AND (SELECT COUNT(*) FROM classifications
                 WHERE handle = ? AND sender = ? AND at > ?) < ?`,
    args: [
      inbox, writer, at,
      inbox, since, CLASSIFY_PER_HANDLE_HOURLY,
      inbox, since, domain, domain, CLASSIFY_PER_DOMAIN_HOURLY,
      inbox, writer, since, CLASSIFY_PER_SENDER_HOURLY,
    ],
  });
  return claimed.rowsAffected > 0;
}

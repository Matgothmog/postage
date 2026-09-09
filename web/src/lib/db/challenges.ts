import { all, db, now, run } from "./client";

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
///
/// Unguarded, where `markEntitled` just above refuses to write twice, because
/// the two are reached under different rules. An entitlement is taken by
/// whoever answers the challenge, and the sender's page answers repeatedly
/// while it waits for the payment to mine, so there the check and the write
/// have to be one statement. A delivery is recorded only by the caller that
/// just made one, and the right to make one is taken upstream by `claimHold` -
/// a single conditional update turning a live hold into a released one, which
/// nothing turns back. It succeeds once per token for good, so this statement
/// runs once per token for good and `delivered_at IS NULL` would always hold.
/// Pinned in `hold.test.ts`, which fails if `claimHold` stops being the gate.
export async function markDelivered(token: string): Promise<void> {
  await run(`UPDATE challenges SET delivered_at = ? WHERE token = ?`, [
    now(),
    token,
  ]);
}

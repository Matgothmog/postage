import { all, db, now, run } from "./client";

/// How long proving personhood keeps the gate open. Long enough to send the
/// message that was just refused, short enough that the proof is about now.
const PASS_WINDOW_SECONDS = 15 * 60;

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

/// Adds one paid delivery. Never reduces what is already there: an unlimited
/// window earned by proving personhood is left alone, a counted pass gains a
/// use, and a sender with neither gets one. Overwriting instead would let a
/// second payment land on a pass that already had a use and buy nothing.
///
/// The unlimited-window branch also stamps `paid_extended_at`. There is no
/// count to add a use to there, so the payment cannot show up as anything but
/// a later `expires_at` — leaving `uses_left` NULL, same as a window nobody
/// ever paid for. `paid_extended_at` is the only record that money touched
/// this row; `EARNED_PASS_CONDITION` reads it for exactly that reason.
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
    sql: `UPDATE passes SET expires_at = ?, paid_extended_at = ?
          WHERE handle = ? AND sender = ? AND expires_at > ? AND uses_left IS NULL`,
    args: [at + PASS_WINDOW_SECONDS, at, handle.toLowerCase(), sender.toLowerCase(), at],
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

/// What marks a pass as earned by proving personhood rather than ever having
/// taken money. Two columns, not one — either alone can lie.
///
/// `reason` cannot be trusted at all: `grantPass`'s own `ON CONFLICT` keeps
/// whatever `uses_left` a row already had whenever it is positive, even while
/// overwriting `reason` to `"human"` on top of it, so a row that reads
/// `reason = 'human'` can still be sitting on a paid, unspent balance.
///
/// `uses_left IS NULL` alone is not enough either, for the mirror-image
/// reason: `addPaidUse`'s unlimited-window branch pays for a window that is
/// already better than a count by pushing `expires_at` out and nothing else —
/// there is no count there to add a use to. That row now holds a payment and
/// still reads `uses_left IS NULL`, indistinguishable from a window nobody
/// ever paid for unless something else says otherwise. `paid_extended_at` is
/// that something else: stamped the moment that branch runs, never cleared
/// once set.
///
/// Exported as text, not just used inline, because `claimNullifier`
/// (`./nullifiers.ts`) needs this exact condition inside its own transaction
/// and a `client.batch` statement cannot call a function in this module —
/// only the SQL travels. Keeping both call sites reading the same constant is
/// what stops the two from quietly drifting into different definitions of
/// "earned."
export const EARNED_PASS_CONDITION = "uses_left IS NULL AND paid_extended_at IS NULL";

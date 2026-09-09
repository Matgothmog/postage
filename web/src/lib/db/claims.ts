import { all, db, now, run } from "./client";

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

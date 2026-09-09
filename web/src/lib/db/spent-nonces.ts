import { db, now, run } from "./client";

/// The wallet nonces that have already been answered, which is what turns a
/// signature into a single-use credential rather than one good for as long as
/// its timestamp stays fresh.
///
/// Kept here rather than in a module variable because that answer has to
/// survive the request that produced it: this deploys to Vercel, where the
/// replay may well land on an instance that has never seen the original.
///
/// Nothing here identifies anyone. A nonce is a server-minted random value and
/// its own MAC, spent by the time it is written, and it names no wallet.

/// Records that this nonce has now been answered, and says whether this caller
/// is the one who got to answer it.
///
/// The condition and the write are one statement, so two requests racing with
/// the same captured signature cannot both come away believing they spent it —
/// `INSERT OR IGNORE` leaves the primary key to decide, and only one insert
/// affects a row.
export async function spendWalletNonce(nonce: string, expiresAt: number): Promise<boolean> {
  const client = await db();
  const spent = await client.execute({
    sql: `INSERT OR IGNORE INTO spent_wallet_nonces (nonce, expires_at) VALUES (?, ?)`,
    args: [nonce, expiresAt],
  });
  return spent.rowsAffected > 0;
}

/// Drops nonces whose window has closed, like the held-message and
/// classification purges, rather than leaving a row per sign-in forever.
///
/// Called from the spend, not from the route that mints — unlike
/// `purgeExpiredContexts`, whose route already writes a row per issue. Minting
/// a wallet nonce touches no database at all and must keep not touching one,
/// or an open endpoint gains a `DELETE` anyone can call as often as they like.
///
/// Nothing depends on it having run: `verifyWalletNonce` refuses an expired
/// nonce on the MAC's own timestamp before this table is ever consulted.
export async function purgeSpentWalletNonces(): Promise<void> {
  await run(`DELETE FROM spent_wallet_nonces WHERE expires_at <= ?`, [now()]);
}

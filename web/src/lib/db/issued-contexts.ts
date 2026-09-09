import { db, now, run } from "./client";

/// Every rp_context this server has signed, and which challenge it was signed
/// for.
///
/// The signing route used to generate a nonce, hand it out and forget it, so
/// there was no answer to either question that matters about a proof coming
/// back: did we ask for this one, and have we already been paid with it. Both
/// are answered here rather than in the route, because the answer has to
/// survive the request that asked — this deploys to Vercel, where the next
/// call may land on an instance that has never seen the previous one, and any
/// ceiling kept in a module variable would count to one forever.
///
/// Nothing here identifies anyone. A nonce is 32 random bytes and a challenge
/// token is already a bearer capability the sender holds.

/// How many unexpired contexts one challenge token may hold at a time.
///
/// Set above a retry rather than at it: a sender who dismisses World App,
/// loses signal and tries again has spent three before anything is wrong.
/// What it stops is the other case — one leaked token minting signed requests
/// for as long as anyone cares to ask, which is what an unauthenticated route
/// allowed for every token at once.
export const MAX_LIVE_CONTEXTS_PER_TOKEN = 5;

/// Writes down a context we are about to hand out, and says whether this token
/// had a slot left for it.
///
/// Counting and recording are one statement, the way every other ceiling in
/// this directory is written. Read-then-write lets a burst — which is the case
/// a ceiling exists for — all see room and all take it, so the real limit
/// becomes the cap plus however many arrived together.
export async function recordIssuedContext(
  token: string,
  nonce: string,
  createdAt: number,
  expiresAt: number
): Promise<boolean> {
  const client = await db();
  const issued = await client.execute({
    sql: `INSERT INTO issued_rp_contexts (nonce, token, created_at, expires_at)
          SELECT ?, ?, ?, ?
          WHERE (SELECT COUNT(*) FROM issued_rp_contexts
                 WHERE token = ? AND expires_at > ?) < ?`,
    args: [nonce, token, createdAt, expiresAt, token, now(), MAX_LIVE_CONTEXTS_PER_TOKEN],
  });
  return issued.rowsAffected > 0;
}

/// Spends the context a proof was produced under, and says whether this caller
/// is the one who got to spend it.
///
/// Every World ID proof carries back the nonce of the request it answers, so
/// this is what turns a signed context into something single use: a proof
/// harvested by a third party under our rp and action can only be presented
/// against the token it was signed for, and only before another presentation
/// of the same proof. The condition and the write are one statement, so two
/// requests racing on one nonce cannot both come away believing they spent it.
export async function consumeIssuedContext(token: string, nonce: string): Promise<boolean> {
  const at = now();
  const client = await db();
  const consumed = await client.execute({
    sql: `UPDATE issued_rp_contexts SET consumed_at = ?
          WHERE nonce = ? AND token = ? AND consumed_at IS NULL AND expires_at > ?`,
    args: [at, nonce, token, at],
  });
  return consumed.rowsAffected > 0;
}

/// Drops contexts whose window has closed. Called on the way past, like the
/// held-message and classification purges, rather than left to grow a row per
/// verification attempt forever under a COUNT every attempt pays for. Nothing
/// depends on it having run: an expired row is already refused by both
/// statements above.
export async function purgeExpiredContexts(): Promise<void> {
  await run(`DELETE FROM issued_rp_contexts WHERE expires_at <= ?`, [now()]);
}

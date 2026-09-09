import { all, db, now } from "./client";
import { EARNED_PASS_CONDITION } from "./passes";

/// Which sender a World ID nullifier is bound to right now, and every time that
/// binding moved.
///
/// A nullifier is one person. The free lane grants one distinct human one
/// standing way through, so the ledger holds one sender per nullifier — never
/// two — and that exclusivity is the whole of what stops one person farming a
/// lane per address they own.
///
/// The binding is exclusive but not permanent, and the difference matters. A
/// proof binds to the sender named on the challenge token, which is whoever
/// mailed the handle rather than whoever took the selfie: anyone can mail a
/// handle from their own address, hand the resulting `/c/<token>` link to a
/// stranger under any pretext, and have that stranger's nullifier written down
/// against the attacker's address. A permanent binding made that theft
/// unrecoverable — there is no delete and no expiry — and cost the victim their
/// free lane for one email. Letting the binding move turns it into a nuisance:
/// the person verifies again from their own address and takes it back.
///
/// Moving it does not reopen farming, because taking a nullifier onto a second
/// address is the same write that takes it off the first — and, as of this
/// version, `claimNullifier` also revokes whatever passes that address earned
/// by proving personhood, in the same transaction as the move. A pass lives
/// in `passes`, keyed on `(handle, sender)`, entirely separate state from this
/// ledger; without also reaching it, a displaced sender kept every earned
/// pass they had already been granted until it lapsed on its own, so the
/// guarantee used to be "one nullifier, one bound sender, always" rather than
/// "one live pass". Now the two are kept in step: the address that loses the
/// nullifier loses the free lane it bought with it, at the same moment,
/// atomically — see the note on `bindNullifierToSender` in
/// `api/world/verify/route.ts` and on `EARNED_PASS_CONDITION` in `./passes.ts`.
///
/// Recording the holder rather than the proof is deliberate. Storing "this
/// exact proof was seen" would refuse a person their own second challenge,
/// which is the thing the credential is supposed to buy them.

/// What a claim did to the ledger.
///
/// `releasedFrom` is the sender the nullifier was taken off. It is reported
/// because it is the one thing a caller cannot work out for itself afterwards:
/// by the time the write has returned, the ledger names only the new holder.
export type NullifierClaim = { rebound: false } | { rebound: true; releasedFrom: string };

/// One move of a binding from one sender to another.
export interface NullifierRebind {
  from_sender: string;
  to_sender: string;
  at: number;
}

/// Binds this nullifier to this sender, taking it off whoever held it before
/// and revoking whatever passes that address earned by proving personhood,
/// and says whether anybody was displaced.
///
/// Three statements rather than one because three tables have to move
/// together, and one transaction rather than three calls because none of them
/// may move without the other two: a rebind that revoked no passes would
/// leave a farmer standing in a lane the ledger claims to have closed, and a
/// revocation with no matching rebind would take a pass from someone the
/// ledger never actually displaced. The first statement is what makes this
/// safe under concurrency: it reads the outgoing holder and writes the hop in
/// the same breath, so the answer it hands back is the holder that this write
/// displaced rather than whoever happened to be there when we last looked. A
/// read followed by a write loses that — racing claims all read the same
/// holder, and the trail ends up recording a move that never happened or
/// missing one that did. The second statement reads the same pre-move row the
/// first one does, for the same reason: it must fire exactly when a hop is
/// really happening, on the sender the hop is really taking it from, and
/// never on a same-sender re-presentation.
export async function claimNullifier(nullifierHash: string, sender: string): Promise<NullifierClaim> {
  const key = nullifierHash.toLowerCase();
  const claimant = sender.toLowerCase();
  const at = now();

  const client = await db();
  const [recorded] = await client.batch(
    [
      {
        // Selects from `nullifiers`, so it writes nothing at all when the
        // nullifier is free or already this sender's — a hop is only a hop
        // when the holder actually changes.
        sql: `INSERT INTO nullifier_rebinds (nullifier_hash, from_sender, to_sender, at)
              SELECT nullifier_hash, sender, ?, ? FROM nullifiers
              WHERE nullifier_hash = ? AND sender <> ?
              RETURNING from_sender`,
        args: [claimant, at, key, claimant],
      },
      {
        // Revokes the outgoing holder's earned passes before `nullifiers`
        // itself is rewritten below, so this still sees who the outgoing
        // holder is rather than the claimant the next statement is about to
        // install. `EARNED_PASS_CONDITION` (`./passes.ts`) is what keeps a
        // paid pass off this statement's reach no matter what `reason` says,
        // or whether the money only ever stretched an already-open window
        // rather than buying a count.
        sql: `DELETE FROM passes
              WHERE ${EARNED_PASS_CONDITION}
                AND sender IN (
                  SELECT sender FROM nullifiers WHERE nullifier_hash = ? AND sender <> ?
                )`,
        args: [key, claimant],
      },
      {
        // `claimed_at` is when the current holder took it, so presenting again
        // leaves it where it was: nothing was claimed that was not already
        // held, and moving the stamp would erase how long they have had it.
        sql: `INSERT INTO nullifiers (nullifier_hash, sender, claimed_at) VALUES (?, ?, ?)
              ON CONFLICT (nullifier_hash) DO UPDATE SET
                sender = excluded.sender,
                claimed_at = CASE WHEN nullifiers.sender = excluded.sender
                                  THEN nullifiers.claimed_at ELSE excluded.claimed_at END`,
        args: [key, claimant, at],
      },
    ],
    "write"
  );

  const releasedFrom = recorded.rows[0]?.from_sender;
  if (typeof releasedFrom !== "string") return { rebound: false };
  return { rebound: true, releasedFrom };
}

export async function senderHoldingNullifier(nullifierHash: string): Promise<string | null> {
  const rows = await all<{ sender: string }>(
    `SELECT sender FROM nullifiers WHERE nullifier_hash = ?`,
    [nullifierHash.toLowerCase()]
  );
  return rows[0]?.sender ?? null;
}

/// Every hand this nullifier has passed through, oldest first. An audit trail
/// nothing can read is not one, and this is what answers "was this lane taken
/// from someone, and how many times has it changed hands".
export async function rebindsOf(nullifierHash: string): Promise<NullifierRebind[]> {
  return await all<NullifierRebind>(
    `SELECT from_sender, to_sender, at FROM nullifier_rebinds
     WHERE nullifier_hash = ? ORDER BY id`,
    [nullifierHash.toLowerCase()]
  );
}

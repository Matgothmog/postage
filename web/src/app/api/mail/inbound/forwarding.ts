import type { Verdict } from "@/lib/classify";
import type { BudgetState } from "@/lib/db/classifications";
import { spendPass } from "@/lib/db/passes";

/// How a message reached the destination without being held: the tier that is
/// never charged, or the pass that carried it.
export interface Forwarded {
  reason: string;
}

/// Held to a higher bar when the verdict is degraded, and shut entirely when
/// the model was never asked.
///
/// The header fallback calls anything transactional-sounding important as long
/// as authentication did not outright fail, so a degraded verdict is a free
/// delivery waiting to be arranged. That is tolerable for exactly one reason to
/// be degraded: the model was reachable enough to ask, was asked, and failed
/// anyway. Real login codes have to keep arriving through an outage of ours,
/// which is the whole point of this tier, and a sender cannot cause that outage.
///
/// Every other degraded verdict on this path is one where the classifier was
/// skipped because a budget had run out, and a budget is a ceiling we chose
/// rather than a failure that befell us. All three can be reached deliberately —
/// an address's own slice by that sender, a domain's by that domain, a handle's
/// pool by a few domains between them — so none of them may unlock the tier
/// nobody pays for. Draining the pool used to do precisely that, and not only
/// for the sender who drained it: it bought free delivery for every
/// authenticated sender writing a transactional-sounding subject, for the rest
/// of the hour.
///
/// The cost of shutting it is real and falls on the recipient: while a pool is
/// drained, transactional mail that would have been guessed important from its
/// headers is held for a challenge instead of delivered. That is the trade —
/// mail held during a flood somebody arranged, rather than a flood being the
/// way through the gate. The per-domain ceiling is what keeps a pool from being
/// drainable cheaply enough for that to be an easy thing to arrange.
export function deliveredFree(
  verdict: Verdict,
  authenticated: boolean,
  budgetRefusal: BudgetState
): boolean {
  if (verdict.tier !== "important") return false;
  if (!verdict.degraded) return true;
  return authenticated && budgetRefusal === null;
}

/// The two ways a message goes straight to the destination, in the order they
/// have to be tried. Anything this returns null for is priced and challenged
/// instead.
export async function forwardWithoutChallenge(mail: {
  handle: string;
  sender: string;
  verdict: Verdict;
  authenticated: boolean;
  budgetRefusal: BudgetState;
}): Promise<Forwarded | null> {
  const { handle, sender, verdict, authenticated, budgetRefusal } = mail;

  // Checked before any pass is spent. This tier grants nothing, so taking a
  // paid use for it would charge someone twice for one delivery.
  if (deliveredFree(verdict, authenticated, budgetRefusal)) return { reason: verdict.tier };

  if (!authenticated || verdict.tier === "dangerous") return null;

  // A live pass, and the envelope it was earned with. Passes run out, so this
  // is a sender who cleared the gate minutes ago rather than ever.
  //
  // Narrowed only when the sender spent their own slice. An unlimited window
  // plus a budget they exhausted themselves is a licence to deliver anything
  // unread, so that one shuts. A single paid use cannot flood by construction —
  // one message, already paid for — and refusing it would take the money and
  // demand it again.
  //
  // Only that one, because only that one names the party who spent it. A
  // domain's slice and a handle's pool are shared: `gmail.com` is millions of
  // unrelated senders, and a handle's pool is every domain writing to that
  // inbox between them. Narrowing on either would take an unlimited window away
  // from someone who paid or proved themselves because of what a stranger did,
  // which is an hour of leverage over their mail for the price of a flood.
  const pass = await spendPass(handle, sender, {
    countedOnly: budgetRefusal === "spent-by-sender",
  });
  return pass ? { reason: pass.reason } : null;
}

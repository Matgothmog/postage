import type { Verdict } from "@/lib/classify";
import type { BudgetState } from "@/lib/db/classifications";
import { spendPass } from "@/lib/db/passes";

/// How a message reached the destination without being held: the tier that is
/// never charged, or the pass that carried it.
export interface Forwarded {
  reason: string;
}

/// Held to a higher bar when the verdict is degraded, and shut entirely when
/// the sender is the reason it is degraded.
///
/// The header fallback calls anything transactional-sounding important as long
/// as authentication did not outright fail, so during an outage a sender who
/// signs their own domain could write "your verification code" and be
/// delivered free. That is tolerable when the outage is ours — real login
/// codes have to keep arriving, which is the whole point of this tier. It is
/// not tolerable when the sender put us here on purpose: their own hourly
/// slice is theirs to spend, so spending it must not unlock anything.
export function deliveredFree(
  verdict: Verdict,
  authenticated: boolean,
  budgetRefusal: BudgetState
): boolean {
  if (verdict.tier !== "important") return false;
  if (!verdict.degraded) return true;
  return authenticated && budgetRefusal !== "spent-by-sender";
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
  // A handle's pool being empty is somebody else's doing: anyone can spend it
  // with forged addresses, and letting that re-challenge everyone who proved
  // themselves would hand a stranger an hour of leverage over someone's mail.
  const pass = await spendPass(handle, sender, {
    countedOnly: budgetRefusal === "spent-by-sender",
  });
  return pass ? { reason: pass.reason } : null;
}

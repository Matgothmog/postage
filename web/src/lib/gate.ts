import {
  addPaidUse,
  challengeByToken,
  claimChallenge,
  grantPass,
  hasLivePass,
  markEntitled,
  releaseChallengeClaim,
  type Challenge,
} from "./db";
import { releaseHeldMessage } from "./hold";

/// How a sender got through. The two are not variations on one thing:
///
///   human  a proof of personhood buys a window. It says someone was there a
///          moment ago, so it opens the gate for a short while and however many
///          messages fit in it.
///   paid   a payment buys one delivery. It is a price for a specific message,
///          so it must yield exactly one, and a second payment must yield a
///          second.
///
/// Every ordering bug in this area came from those two being written out by
/// hand in separate routes and drifting apart, so they are stated here once.
export type Lane = "human" | "paid";

export type GateResult =
  | { status: "unknown" }
  | { status: "charged"; reason: "dangerous" }
  | { status: "cleared"; reason: string; delivered: boolean };

/// The one place a challenge is settled.
///
/// The invariants it exists to hold, all of which were previously re-derived in
/// each caller:
///
///   1. A challenge settles once. The claim is a single conditional update, so
///      two tabs cannot both believe they are the one settling it.
///   2. Clearing yields exactly one entitlement. Either the held message goes,
///      or the sender can send one — never both, never neither.
///   3. Dangerous mail is delivered by no route and no lane.
///   4. A failure part way through leaves nothing granted and the challenge
///      openable again, because a payment cannot be made twice.
///   5. Answering a challenge that is already settled reports what actually
///      happened, and repairs a sender who holds nothing.
export async function openGate(token: string, lane: Lane): Promise<GateResult> {
  const challenge = await challengeByToken(token);
  if (!challenge) return { status: "unknown" };
  if (challenge.tier === "dangerous") return { status: "charged", reason: "dangerous" };

  if (!(await claimChallenge(token, lane))) return recover(token, challenge, lane);

  try {
    return await settle(challenge, token, lane);
  } catch (cause) {
    // Nothing was granted before this point on either lane, so giving the claim
    // back leaves the sender exactly as they were rather than holding a settled
    // challenge and nothing to show for it.
    await releaseChallengeClaim(token).catch(() => {});
    throw cause;
  }
}

async function settle(challenge: Challenge, token: string, lane: Lane): Promise<GateResult> {
  // The message goes out before anything is granted, on both lanes. Granting
  // first leaves a pass standing if this throws, and the rollback below would
  // then reopen a challenge whose sender is already holding what it owed.
  const released = challenge.delivered_at != null
    ? { delivered: true }
    : await releaseHeldMessage(token, challenge.handle);

  await grant(challenge, lane, released.delivered);
  await markEntitled(token);
  return { status: "cleared", reason: lane, delivered: released.delivered };
}

/// What each lane is owed once the message has been dealt with.
///
/// A proof buys a window, so it is opened whether or not the held message went:
/// being able to write is the thing that was earned. A payment buys one
/// delivery, so it owes nothing further if that delivery already happened.
async function grant(challenge: Challenge, lane: Lane, delivered: boolean): Promise<void> {
  if (lane === "human") {
    await grantPass(challenge.handle, challenge.sender, "human", null);
    return;
  }
  if (!delivered) await addPaidUse(challenge.handle, challenge.sender);
}

/// Someone else settled this, or we did and the answer was lost. Report what
/// actually happened rather than a refusal — and if they are holding nothing
/// while nothing was delivered, give back what their lane earned. The caller
/// has already proved the lane to reach this point.
async function recover(token: string, before: Challenge, lane: Lane): Promise<GateResult> {
  // Re-read, because `before` was loaded ahead of the claim and will not carry
  // whatever the winner wrote while we were asking.
  const settled = (await challengeByToken(token)) ?? before;
  if (settled.tier === "dangerous") return { status: "charged", reason: "dangerous" };

  const delivered = settled.delivered_at != null;
  const holdsNothing = !(await hasLivePass(settled.handle, settled.sender));

  if (!delivered && holdsNothing) {
    if (lane === "human") {
      // A fresh proof was presented to reach here, and that is the price of a
      // window. Re-earning one is the design, not a leak.
      await grantPass(settled.handle, settled.sender, "human", null);
    } else if (settled.entitled_at == null) {
      // A payment settles onchain forever, so "have they paid" is true for good
      // and cannot decide this. Only whether this challenge has already handed
      // over what that payment bought.
      await addPaidUse(settled.handle, settled.sender);
      await markEntitled(token);
    }
  }

  return { status: "cleared", reason: settled.settled_by ?? lane, delivered };
}

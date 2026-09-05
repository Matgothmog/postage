import type { SenderSignals } from "./reputation";

export interface Quote {
  price: bigint;
  basePrice: bigint;
  multiplierBps: number;
  free: boolean;
  reasons: string[];
}

const ONE = 10_000;
/// The inbox price is a floor the contract enforces, so a quote can never go
/// under it however good the sender looks. Reputation earns its way down to
/// the floor and no further; the way to pay nothing is to verify.
const FLOOR = ONE;
const CEILING = 50_000;
const YEAR_SECONDS = 365 * 24 * 60 * 60;

/// Turns what The Graph knows about a sender into what they pay. The inbox
/// owner sets the base; reputation moves it between a quarter of that and five
/// times it. Every adjustment carries the sentence that explains it, because a
/// price nobody can interpret just reads as arbitrary.
export function quote(basePrice: bigint, signals: SenderSignals): Quote {
  if (signals.isHuman) {
    return {
      price: 0n,
      basePrice,
      multiplierBps: 0,
      free: true,
      reasons: ["Verified as a person, so postage is waived"],
    };
  }

  const reasons: string[] = [];
  let bps = ONE;

  if (signals.settledCount > 0 && signals.spamRate > 0) {
    bps += Math.round(signals.spamRate * 4 * ONE);
    reasons.push(
      `Marked as spam on ${signals.claimedCount} of ${signals.settledCount} settled messages`
    );
  }

  if (signals.settledCount >= 3 && signals.spamRate < 0.2) {
    bps = Math.round(bps * 0.5);
    reasons.push(`Well received here across ${signals.settledCount} messages`);
  }

  if (signals.ensNames > 0) {
    bps = Math.round(bps * 0.7);
    reasons.push(`Holds ${signals.ensNames} ENS name${signals.ensNames === 1 ? "" : "s"}`);

    const age = signals.oldestEnsAt ? Math.floor(Date.now() / 1000) - signals.oldestEnsAt : 0;
    if (age > YEAR_SECONDS) {
      bps = Math.round(bps * 0.8);
      reasons.push(`Oldest of them registered ${Math.floor(age / YEAR_SECONDS)} years ago`);
    }
  }

  if (signals.ensNames === 0 && signals.stampsPosted === 0) {
    bps *= 2;
    reasons.push("No onchain history to go on, so this is priced as a stranger");
  }

  if (bps < FLOOR) {
    reasons.push("Already at this inbox's minimum, so the discount stops here");
  }
  bps = Math.min(Math.max(bps, FLOOR), CEILING);

  return {
    price: (basePrice * BigInt(bps)) / BigInt(ONE),
    basePrice,
    multiplierBps: bps,
    free: false,
    reasons,
  };
}

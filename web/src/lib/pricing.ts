import type { Tier } from "./classify";
import type { SenderSignals } from "./reputation";

export interface Quote {
  amount: bigint;
  floor: bigint;
  multiplierBps: number;
  free: boolean;
  reasons: string[];
}

const ONE = 10_000;
const CEILING = 100_000;
const YEAR_SECONDS = 365 * 24 * 60 * 60;

/// What each verdict costs before reputation is considered, in basis points of
/// the inbox's floor price.
const TIER_BPS: Record<Tier, number> = {
  human: 0,
  important: 0,
  commercial: ONE,
  /// Deliberately punitive. Dangerous mail is blocked either way; this is what
  /// a sender pays if they have a wallet attached, not a price for delivery.
  dangerous: 10 * ONE,
};

/// Turns a verdict and what The Graph knows about a sender into a price.
///
/// The tier sets the base and reputation moves it. A quote can never fall below
/// the inbox's floor, because the escrow enforces that and would revert.
export function quote(
  floor: bigint,
  tier: Tier,
  signals: SenderSignals | null,
  degraded: boolean
): Quote {
  if (tier === "human") {
    return { amount: 0n, floor, multiplierBps: 0, free: true, reasons: ["Written by a verified person"] };
  }
  if (tier === "important") {
    return {
      amount: 0n,
      floor,
      multiplierBps: 0,
      free: true,
      reasons: ["Something the recipient is waiting for, so it goes through free"],
    };
  }

  const reasons: string[] = [];
  let bps = TIER_BPS[tier];

  if (tier === "dangerous") {
    reasons.push("Classified as an attempt to deceive the recipient");
  } else {
    reasons.push("Automated mail the recipient did not ask for");
  }

  // A header-only verdict is not confident enough to charge punitively.
  if (degraded && tier === "dangerous") {
    bps = 2 * ONE;
    reasons.push("Priced down because the classifier was unavailable and this came from headers alone");
  }

  if (signals) {
    if (signals.paidCount > 0 && signals.spamRate > 0) {
      bps += Math.round(signals.spamRate * 4 * ONE);
      reasons.push(`Reported as spam on ${signals.spamReports} of ${signals.paidCount} past messages`);
    }
    if (signals.paidCount >= 3 && signals.spamRate < 0.2) {
      bps = Math.round(bps * 0.5);
      reasons.push(`Well received here across ${signals.paidCount} messages`);
    }
    if (signals.ensNames > 0) {
      bps = Math.round(bps * 0.7);
      reasons.push(`Sender wallet holds ${signals.ensNames} ENS name${signals.ensNames === 1 ? "" : "s"}`);

      const age = signals.oldestEnsAt ? Math.floor(Date.now() / 1000) - signals.oldestEnsAt : 0;
      if (age > YEAR_SECONDS) {
        bps = Math.round(bps * 0.8);
        reasons.push(`Oldest registered ${Math.floor(age / YEAR_SECONDS)} years ago`);
      }
    }
  }

  if (bps < ONE) {
    reasons.push("Already at this inbox's minimum, so the discount stops here");
  }
  bps = Math.min(Math.max(bps, ONE), CEILING);

  return {
    amount: (floor * BigInt(bps)) / BigInt(ONE),
    floor,
    multiplierBps: bps,
    free: false,
    reasons,
  };
}

/// The four verdicts, and the order the escrow's `Tier` enum puts them in.
///
/// Kept in a module of its own because both ends of the app need it and they
/// cannot share the one it used to live in: the classifier pulls in the model
/// SDK, and the pay button runs in the browser. Written out twice, the two
/// drifted apart silently — a reordered enum would have mispriced every message
/// rather than failing.
export const TIERS = ["human", "important", "commercial", "dangerous"] as const;

export type Tier = (typeof TIERS)[number];

export const TIER_INDEX: Record<Tier, number> = {
  human: 0,
  important: 1,
  commercial: 2,
  dangerous: 3,
};

/// A tier read back off a stored quote, where it is only a string. Anything
/// unrecognised is treated as commercial, which is the tier an ordinary
/// stranger pays.
export function tierIndexOf(tier: string): number {
  return TIER_INDEX[tier as Tier] ?? TIER_INDEX.commercial;
}

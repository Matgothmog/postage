/// The signed-quote shape as it exists on either side of a serialization
/// boundary: parsed back out of `challenges.quote_json`, or received as props
/// by a client component across the server/client split. Both crossings lose
/// the branded `Hex` and `Tier` types `signQuote` (in `./quote.ts`) produces —
/// JSON has no notion of them, and neither does the wire between a Server and
/// a Client Component — so every field here is the plain string that survives
/// the trip. `tierIndexOf` in `./tiers.ts` makes the same call explicitly for
/// `tier` alone; this type states it for the whole quote.
///
/// Kept apart from `quote.ts` on purpose: that module pulls in `node:crypto`
/// and a private-key signer for `signQuote`, neither of which a client
/// component like `ChallengeActions` should ever risk carrying into its
/// bundle just to know the shape of its `quote` prop.
export interface QuoteFields {
  messageId: string;
  inbox: string;
  tier: string;
  amount: string;
  expiresAt: number;
  signature: string;
}

/// `QuoteFields` plus the reasons the classifier and pricing gave, stored
/// alongside the quote and shown to the sender on the challenge page.
export interface StoredQuote extends QuoteFields {
  reasons: string[];
}

/// The widths the escrow call built from this quote reads off it: `messageId`
/// goes in as a `bytes32`, `inbox` as an `address`, and `signature` as the 65
/// bytes of r, s and v that `signQuote`'s signer produces. Each is cast to
/// `0x${string}` at the call site, and a cast checks nothing — viem is what
/// finds out, by throwing, that a value is not the width its type says.
const MESSAGE_ID = /^0x[0-9a-fA-F]{64}$/;
const ADDRESS = /^0x[0-9a-fA-F]{40}$/;
const SIGNATURE = /^0x[0-9a-fA-F]{130}$/;

/// A whole non-negative number written out in digits, which is the only thing
/// `BigInt` reads the way this column means it. `BigInt` throws outright on
/// `"1e18"` — and `BigInt(quote.amount)` runs inside a Server Component, so
/// that throw is the page failing rather than the row being refused. It is
/// also quietly wrong on more than it refuses: `"0x10"` is sixteen to it, and
/// surrounding whitespace is trimmed rather than rejected.
const AMOUNT = /^[0-9]+$/;

/// The escrow takes the deadline as a `uint40`. Above that there is no call to
/// build, and `typeof === "number"` admits every value JavaScript calls one -
/// a fraction, a negative, and `Infinity`, which is what `JSON.parse` returns
/// for an exponent too large to hold.
const MAX_EXPIRES_AT = 2 ** 40 - 1;

function matches(pattern: RegExp, value: unknown): value is string {
  return typeof value === "string" && pattern.test(value);
}

function isExpiry(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= MAX_EXPIRES_AT;
}

function isStoredQuote(value: unknown): value is StoredQuote {
  if (typeof value !== "object" || value === null) return false;
  const quote = value as Record<string, unknown>;
  return (
    matches(MESSAGE_ID, quote.messageId) &&
    matches(ADDRESS, quote.inbox) &&
    typeof quote.tier === "string" &&
    matches(AMOUNT, quote.amount) &&
    isExpiry(quote.expiresAt) &&
    matches(SIGNATURE, quote.signature) &&
    Array.isArray(quote.reasons) &&
    quote.reasons.every((reason) => typeof reason === "string")
  );
}

/// Turns the raw `challenges.quote_json` column back into a `StoredQuote`, or
/// hands back `null` rather than letting a row an older schema wrote, a
/// truncated write, or anything else that isn't the shape we expect take the
/// page down for whoever is reading it.
///
/// Values, not only types. Everything a caller does with this quote it does by
/// handing a field to something that reads it — `BigInt`, `encodeFunctionData`
/// — and each of those throws on a string of the right type carrying the wrong
/// value. Checking the type alone moves the failure from this function, where
/// it renders "This link is broken", into the render, where it is a 500.
export function parseStoredQuote(raw: string): StoredQuote | null {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }
  return isStoredQuote(value) ? value : null;
}

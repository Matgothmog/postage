/// How long a signed rp_context lives, and how long the browser may wait on
/// the World App under one.
///
/// The two belong in one file because they are one number in two units. The
/// SDK defaults disagree: `signRequest` signs a five minute window while
/// `pollUntilCompletion` waits fifteen, so a sender who took six minutes over
/// their selfie got `rp_signature_expired` back from World — surfaced as a
/// generic "try again" that would then have failed identically forever.
/// Whichever way the TTL moves, the polling window has to move with it, so
/// neither may be stated anywhere else.

/// Short on purpose. A signed context is a bearer credential naming this
/// relying party and this action, and anyone holding one can put it in front
/// of their own visitors, so how much a stolen one is worth is capped by how
/// briefly it lives.
export const RP_CONTEXT_TTL_SECONDS = 300;

/// The gap between the browser giving up and the signature actually expiring.
/// It absorbs the round trip that carried the context to the browser and any
/// skew between our clock and World's, so a slow sender is told "that took too
/// long" by their own poll loop rather than being rejected by World.
export const POLL_SAFETY_MARGIN_SECONDS = 30;

/// Floors the window rather than letting a very short TTL produce a timeout a
/// poll loop would treat as already elapsed. Exported so tests can assert
/// against it directly instead of restating `5` as a second copy of it.
export const MIN_POLL_SECONDS = 5;

/// The half of an `RpContext` that says when it was signed and when it stops
/// being worth anything. Described structurally rather than imported from
/// `@worldcoin/idkit`, so the server route that signs one can size the window
/// without pulling the browser SDK in behind it.
export interface RpContextWindow {
  created_at: number;
  expires_at: number;
}

/// How long the client may wait for the World App under this context.
///
/// Read off the signature's own two timestamps rather than any wall clock, so
/// a browser whose clock is minutes out still polls for the window it was
/// actually granted instead of one it computed against the wrong present.
export function pollTimeoutMs(context: RpContextWindow): number {
  const granted = context.expires_at - context.created_at;
  // A context missing a timestamp, or carrying one that isn't really a
  // number, makes `granted` NaN — and `Math.max(NaN, floor)` is NaN, not the
  // floor, so the guard below it would silently stop working. Falling back
  // to 0 here lets that floor do its job for a malformed window exactly as
  // it already does for a genuinely short one, rather than handing a poll
  // loop a timeout that fires instantly. A well-formed but inverted window
  // (expires_at before created_at) needs no special case: `granted` is a
  // real, finite, negative number there, and the floor already wins that
  // `Math.max` on its own.
  const safeGranted = Number.isFinite(granted) ? granted : 0;
  return Math.max(safeGranted - POLL_SAFETY_MARGIN_SECONDS, MIN_POLL_SECONDS) * 1000;
}

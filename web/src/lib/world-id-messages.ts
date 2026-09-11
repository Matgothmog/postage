import type { IDKitErrorCodes } from "@worldcoin/idkit";

/// Sender-safe copy for a World ID failure, keyed by the code that named it.
///
/// Its own module, and deliberately not part of `@/lib/world-id`. That file's
/// first line is a value import of `@worldcoin/idkit`, so reading a message
/// out of it would drag the whole browser widget in behind the table: the
/// IDKit request builder, the QR renderer, its icons and translations, the
/// `qrcode` package, and `idkit_wasm_bg.wasm` traced into the route's file
/// dependencies — none of which a route that only maps a code to a sentence
/// has any use for. `rp-context.ts` already splits along the same line for
/// the same reason.
///
/// Not a trim against `main`, though: `main`'s `/api/world/verify` never
/// imported `world-id.ts` at all, and answered a rejected proof with World's
/// own `detail` field directly, so its bundle already carried none of the
/// above. The curated lookup this file backs
/// (`logAndDescribeWorldVerifyFailure`, `@/app/api/world/verify/route.ts`) is
/// new on this branch; the split is what keeps that new lookup from being the
/// thing that drags idkit into the route for the first time, not a repair of
/// weight `main` was already paying. Measured with `next build`: this
/// branch's route chunks land at 472,067 bytes against `main`'s 468,890 — a
/// net +3,177 bytes (+0.68%) from the curated table and its lookup, not a
/// reduction.
///
/// `IDKitErrorCodes` below is imported as a *type* alone. `import type` is
/// erased before bundling, so naming it costs this module nothing at runtime
/// while still giving the table the one thing a plain `Record<string, string>`
/// cannot: a misspelled key fails the build.

/// Every key this table may carry.
///
/// `Partial` on the IDKit half, because most of that enum has no curated copy
/// and needs none. Exact on the other two, because they are the only keys here
/// IDKit's vocabulary does not contain — World's v2-era names for the
/// verification-limit failure (docs/world-feedback.md:186-196) — so nothing
/// but this clause would notice if one were dropped, and whichever name the
/// verify endpoint actually sends would fall to the generic fallback.
type WorldIdFailureMessages = Partial<Record<IDKitErrorCodes, string>> &
  Record<"exceeded_max_verifications" | "already_verified", string>;

/// The one table both `describeWorldIdFailure` (`@/lib/world-id`) and
/// `logAndDescribeWorldVerifyFailure` (`@/app/api/world/verify/route.ts`) read
/// from, rather than two independently-worded copies of the same event that
/// could drift apart. The two learn about failures from different directions —
/// IDKit's own client-side completion codes versus the codes World's
/// server-side verify endpoint puts in its response body — but the
/// vocabularies overlap: `rp_signature_expired` and `max_verifications_reached`
/// are both documented `IDKitErrorCodes` members and both codes the verify
/// endpoint can send, so a request that fails the same way is described the
/// same way regardless of which side caught it.
///
/// `exceeded_max_verifications` and `already_verified` are older names for the
/// verification-limit failure, seen only in World's v2-era material and never
/// issued by IDKit's own SDK — `IDKitErrorCodes` has no member for either, so
/// they exist here purely for `logAndDescribeWorldVerifyFailure`'s benefit;
/// `describeWorldIdFailure` can never be called with one, since its parameter
/// type already rules it out.
///
/// Every key here is a code this codebase has direct evidence for, either from
/// `IDKitErrorCodes` itself or from World's own documentation cited alongside
/// `logAndDescribeWorldVerifyFailure`. A code reaching either function from
/// outside this table is not a surprise it needs to anticipate — it falls to
/// `GENERIC_WORLD_ID_FAILURE_MESSAGE` and is logged loudly enough on the server
/// side to notice and add.
export const WORLD_ID_FAILURE_MESSAGES = {
  user_rejected: "You closed the World App before finishing. Try again when you're ready.",
  cancelled: "You closed the World App before finishing. Try again when you're ready.",
  verification_rejected: "World ID could not verify you for this. Nothing was charged or sent.",
  nullifier_replayed: "World ID could not verify you for this. Nothing was charged or sent.",
  identity_attributes_not_matched: "World ID could not verify you for this. Nothing was charged or sent.",
  timeout: "That took too long and the request expired. Try again.",
  rp_signature_expired: "That took too long and the request expired. Try again.",
  connection_failed: "Could not reach the World App. Check your connection and try again.",
  credential_unavailable: "Selfie Check isn't available for this app yet. Pay instead, or try again later.",
  feature_unavailable: "Selfie Check isn't available for this app yet. Pay instead, or try again later.",
  world_id_4_not_available: "Selfie Check isn't available for this app yet. Pay instead, or try again later.",
  world_id_3_not_available: "Selfie Check isn't available for this app yet. Pay instead, or try again later.",
  // Unlike every other bucket here, retrying does not merely risk failing
  // again — World's own limit is spent for good the moment this fires, so
  // offering "try again" would be advice this sender cannot follow. Paying
  // is the only door this message may honestly point at.
  max_verifications_reached: "This World ID has already been used. Nothing was charged or sent. Pay instead.",
  exceeded_max_verifications: "This World ID has already been used. Nothing was charged or sent. Pay instead.",
  already_verified: "This World ID has already been used. Nothing was charged or sent. Pay instead.",
} satisfies WorldIdFailureMessages;

/// What every failure outside `WORLD_ID_FAILURE_MESSAGES` gets, from both
/// functions that consult that table. One constant rather than two literal
/// copies of the same sentence is what makes "the two fallbacks are the same
/// line" a fact the type checker and a `grep` both agree on, not a claim a
/// comment has to be trusted for.
export const GENERIC_WORLD_ID_FAILURE_MESSAGE = "Verification failed. Try again, or pay instead.";

/// The table again as a `Map`, which is what every lookup below goes through.
///
/// An object literal carries `Object.prototype`, so indexing one with a code
/// that came off the wire answered `WORLD_ID_FAILURE_MESSAGES["toString"]`
/// with `Object.prototype.toString` — a function rather than `undefined`, past
/// which no `??` fallback can rescue anything, and which reached the sender as
/// "function toString() { [native code] }". A `Map` has no inherited keys to
/// find, so that is gone by construction rather than held off by a guard a
/// later edit could drop.
const CURATED_COPY: ReadonlyMap<string, string> = new Map(Object.entries(WORLD_ID_FAILURE_MESSAGES));

/// The curated copy for `code`, or `undefined` when the table has never heard
/// of it.
///
/// Deliberately not folded together with `GENERIC_WORLD_ID_FAILURE_MESSAGE`:
/// a caller that has to *say* whether the code was recognised — the verify
/// route logs a different line for each — can then tell the two apart outright,
/// rather than comparing the returned copy against the generic sentence and
/// trusting that no curated message ever equals it.
export function worldIdFailureMessage(code: string): string | undefined {
  return CURATED_COPY.get(code);
}

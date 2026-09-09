import { IDKit, selfieCheckLegacy } from "@worldcoin/idkit";
import type {
  IDKitErrorCodes,
  IDKitRequestConfig,
  IDKitResult,
  RpContext as SignedRpContext,
} from "@worldcoin/idkit";
import { pollTimeoutMs } from "./rp-context";

/// Selfie Check issues World ID 3.0 proofs, so IDKit must be told explicitly
/// to accept them - the v4 default rejects anything but a v4 proof. See
/// docs/world-feedback.md.
const ALLOW_LEGACY_PROOFS = true;

/// `@worldcoin/idkit` re-exports `IDKitRequestConfig`, `IDKitResult`,
/// `IDKitErrorCodes` and `RpContext` from `@worldcoin/idkit-core`, but not the
/// two shapes `IDKit.request(...).preset(...)` actually resolves to
/// (`IDKitRequest`, `IDKitCompletionResult`) — checked against the installed
/// package's own `dist/index.d.ts`, not assumed. `web/package.json` declares
/// only `idkit` and `idkit-server`, not `idkit-core` itself, so importing the
/// missing names from there directly would depend on an undeclared package.
/// These two are the minimum reconstruction of what this module actually
/// uses, built from names IDKit does export rather than redeclaring what an
/// `IDKitResult` looks like inside. `WaitOptions` (for `pollUntilCompletion`)
/// gets the same treatment below, for the same reason.
export interface SelfieCheckHandle {
  readonly connectorURI: string;
  pollUntilCompletion(options?: { timeout?: number }): Promise<SelfieCheckCompletion>;
}

export type SelfieCheckCompletion =
  | { success: true; result: IDKitResult }
  | { success: false; error: IDKitErrorCodes };

export type SelfieCheckOutcome = { ok: true; proof: IDKitResult } | { ok: false; message: string };

/// What `/api/world/context` signs and returns. A superset of IDKit's own
/// `RpContext` (`rp_id`/`nonce`/`created_at`/`expires_at`/`signature`) plus
/// the `action` the server signed it for.
///
/// The action lives here, and nowhere else, on purpose: it used to also be
/// set independently as `NEXT_PUBLIC_WORLD_ACTION` for the browser to send to
/// IDKit, and the two copies could drift — the server signs one action while
/// the browser requests another, which World rejects as an invalid signature
/// with nothing in the client to explain why. Reading it back out of the
/// signature the server already produced makes drift impossible rather than
/// merely unlikely.
export type RpContext = SignedRpContext & { action: string };

/// Talks to the real World App via IDKit. Kept as the one place that does, so
/// `runSelfieCheck` below can be driven by a fake in tests instead.
///
/// The signal is what binds the proof to one challenge. It is hashed into the
/// proof World App produces and comes back as `signal_hash` on the response,
/// so `/api/world/verify` can refuse a proof that was made for a different
/// token. Left empty, the resulting proof says only "a person did this" and
/// clears whatever challenge it is pasted into — which is the same proof
/// bytes the sender can read out of their own network tab.
export function openSelfieCheckWithIDKit(
  config: IDKitRequestConfig,
  signal: string
): Promise<SelfieCheckHandle> {
  return IDKit.request(config).preset(selfieCheckLegacy({ signal }));
}

/// Thrown by `fetchRpContext` for a non-ok response, carrying the sender-safe
/// message the route already wrote for exactly this refusal rather than a
/// second, independently-worded copy of it here. Exported so a test can
/// construct one directly to exercise how `runSelfieCheck` treats a known,
/// curated refusal differently from an arbitrary thrown `Error` — a plain
/// `Error` still gets papered over with a generic message, since its text was
/// never vetted as safe to show a sender.
export class RpContextError extends Error {
  readonly status: number;

  constructor(message: string, status: number) {
    super(message);
    this.name = "RpContextError";
    this.status = status;
  }
}

/// `/api/world/context` already answers every refusal — settled, dangerous,
/// unknown, rate-limited — with a message written for the sender reading it,
/// not an internal detail (see `route.ts`). Relaying that text is what keeps
/// "you can retry this" and "you cannot" distinguishable without a second,
/// separately maintained copy of the same wording that could drift from it.
/// Only a response with nothing to read — a network failure, a non-JSON error
/// page from in front of the app — falls back to a message written here.
async function describeContextRefusal(response: Response): Promise<RpContextError> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  const message =
    typeof body?.error === "string" ? body.error : "Could not reach World ID. Check your connection and try again.";
  return new RpContextError(message, response.status);
}

/// True only when `value` actually has every field `RpContext` promises.
///
/// `/api/world/context`'s own route only ever returns a well-formed context,
/// but its response is the one place on this path that can leave the app's
/// control before reaching here — a proxy, a CDN interstitial, or a captive
/// portal can all answer with a 200 and a JSON body of their own. Checking
/// the shape here, rather than trusting it with a cast, is what keeps that
/// body from reaching `pollTimeoutMs` at all: `created_at` and `expires_at`
/// are required to be finite numbers specifically because that function
/// subtracts them, and anything looser would come out the other side as
/// `NaN` instead of a bounded window.
function isRpContext(value: unknown): value is RpContext {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.rp_id === "string" &&
    typeof candidate.nonce === "string" &&
    typeof candidate.signature === "string" &&
    typeof candidate.action === "string" &&
    typeof candidate.created_at === "number" &&
    Number.isFinite(candidate.created_at) &&
    typeof candidate.expires_at === "number" &&
    Number.isFinite(candidate.expires_at)
  );
}

/// Fetches the rp_context every World ID 4.0-shaped proof request must carry,
/// signed server-side so the signing key never reaches the browser. Posts the
/// same challenge token every other route on this path is judged against —
/// the context endpoint uses it to decide whether this caller may have one at
/// all, not merely to remember whose proof it will turn out to be.
export async function fetchRpContext(token: string): Promise<RpContext> {
  const response = await fetch("/api/world/context", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ token }),
  });
  if (!response.ok) throw await describeContextRefusal(response);

  const body: unknown = await response.json();
  if (!isRpContext(body)) {
    throw new RpContextError("World ID sent back something we didn't understand. Try again.", response.status);
  }
  return body;
}

/// Fails loudly rather than letting an unset or malformed app id reach IDKit,
/// which types it as `app_${string}` and would otherwise surface as an
/// unhelpful error deep inside the SDK.
function requireWorldAppId(raw: string | undefined): `app_${string}` {
  if (!raw || !raw.startsWith("app_")) throw new Error("NEXT_PUBLIC_WORLD_APP_ID is not set");
  return raw as `app_${string}`;
}

/// Every value idkit-core's own `IDKitRequestConfig.environment` field
/// accepts — checked against the installed package's own
/// `@worldcoin/idkit-core/dist/index.d.ts:65` ("Optional environment
/// override. Defaults to \"production\".") rather than assumed, the same way
/// the rest of this file treats that package's re-exports. Declared here
/// rather than imported, since `idkit-core` is itself an undeclared
/// dependency of `web/package.json` (see the note above
/// `SelfieCheckHandle`).
type WorldEnvironment = "production" | "staging" | "sandbox";

/// This client never named an `environment` before this flag existed, and
/// idkit-core's own default is "production" — so unset or empty stays
/// exactly what every deployment already does, silently. Anything else must
/// be one of idkit-core's three accepted values: a near-miss is refused
/// rather than coerced to "production", because a silent fallback to
/// production is exactly the sandbox/production mismatch
/// `NEXT_PUBLIC_WORLD_ENVIRONMENT` exists to catch — a sandbox World App
/// build that quietly requested a production-targeted Selfie Check would
/// fail with nothing here to explain why.
function requireWorldEnvironment(raw: string | undefined): WorldEnvironment {
  if (!raw) return "production";
  if (raw === "production" || raw === "staging" || raw === "sandbox") return raw;
  throw new Error(
    `NEXT_PUBLIC_WORLD_ENVIRONMENT is set to an unrecognised value: "${raw}". Expected "production", "staging", or "sandbox".`
  );
}

/// Refuses to open a Selfie Check that is bound to nothing.
///
/// A caller that forgets the signal gets no proof rather than an unbound one,
/// because an unbound proof is exactly the artefact this whole change exists
/// to stop anyone holding — and `/api/world/verify` will refuse it anyway, so
/// producing one only spends the sender's time to reach the same answer.
function requireSignal(raw: string | undefined): string {
  if (!raw) throw new Error("a Selfie Check needs a signal to bind the proof to");
  return raw;
}

/// Maps what can go wrong obtaining a Selfie Check proof to copy a sender can
/// act on. Kept as its own function, not folded into a catch-all, because
/// "you stopped", "we couldn't reach World", and "World said no" call for
/// different next steps and only one of them is worth retrying immediately.
export function describeWorldIdFailure(errorCode: IDKitErrorCodes): string {
  switch (errorCode) {
    case "user_rejected":
    case "cancelled":
      return "You closed the World App before finishing. Try again when you're ready.";
    case "verification_rejected":
    case "nullifier_replayed":
    case "identity_attributes_not_matched":
      return "World ID could not verify you for this. Nothing was charged or sent.";
    case "timeout":
      return "That took too long and the request expired. Try again.";
    case "connection_failed":
      return "Could not reach the World App. Check your connection and try again.";
    case "credential_unavailable":
    case "feature_unavailable":
    case "world_id_4_not_available":
    case "world_id_3_not_available":
      return "Selfie Check isn't available for this app yet. Pay instead, or try again later.";
    default:
      return "Verification failed. Try again, or pay instead.";
  }
}

export interface SelfieCheckDeps {
  appId: string | undefined;
  /// The challenge token this proof is for. Optional in the type and required
  /// in practice: a caller that omits it gets a refusal, not an unbound proof.
  /// See `requireSignal`. Also the token `fetchRpContext` is called with,
  /// since both name the same challenge.
  ///
  /// There is deliberately no separate `action` field here — the action IDKit
  /// is told to use comes back from `fetchRpContext` on the signed `RpContext`
  /// itself. See its doc comment for why a second, independently-set action
  /// is exactly the bug this shape exists to rule out.
  signal: string | undefined;
  fetchRpContext: (token: string) => Promise<RpContext>;
  openSelfieCheck: (config: IDKitRequestConfig, signal: string) => Promise<SelfieCheckHandle>;
  /// Fires once the request is ready to be answered, so the caller can show
  /// the World App link while `pollUntilCompletion` is still waiting on it —
  /// otherwise a live-mode sender has a spinner and no way to open World App.
  onConnectorReady?: (connectorURI: string) => void;
}

/// Everything between "the sender clicked the button" and "we have a proof or
/// we don't", isolated from the fetch/IDKit calls that do it so it can be
/// tested against fakes for both. Never throws: every failure path — bad
/// client config, an unreachable World ID context endpoint, IDKit itself
/// rejecting the request, or the sender never completing it — resolves to
/// `{ ok: false, message }` rather than leaving the caller to guess which
/// catch clause applies.
export async function runSelfieCheck(deps: SelfieCheckDeps): Promise<SelfieCheckOutcome> {
  let appId: `app_${string}`;
  let signal: string;
  let environment: WorldEnvironment;
  try {
    appId = requireWorldAppId(deps.appId);
    signal = requireSignal(deps.signal);
    // Read directly here, as a literal `process.env.NEXT_PUBLIC_*` access,
    // rather than threaded through `SelfieCheckDeps` like `appId` is — Next
    // only inlines `NEXT_PUBLIC_` variables where the literal property access
    // itself appears in code that reaches the client bundle, and this module
    // is that code (see `ChallengeActions.tsx`'s import of `runSelfieCheck`).
    environment = requireWorldEnvironment(process.env.NEXT_PUBLIC_WORLD_ENVIRONMENT);
  } catch (cause) {
    console.error("Selfie Check cannot start: client is not configured", cause);
    return { ok: false, message: "World ID isn't configured yet. Pay instead, or try again shortly." };
  }

  let rpContext: RpContext;
  try {
    rpContext = await deps.fetchRpContext(signal);
  } catch (cause) {
    console.error("failed to fetch rp_context for a live Selfie Check", cause);
    return {
      ok: false,
      message:
        cause instanceof RpContextError ? cause.message : "Could not reach World ID. Check your connection and try again.",
    };
  }

  let handle: SelfieCheckHandle;
  try {
    handle = await deps.openSelfieCheck(
      {
        app_id: appId,
        action: rpContext.action,
        rp_context: rpContext,
        allow_legacy_proofs: ALLOW_LEGACY_PROOFS,
        environment,
      },
      signal
    );
  } catch (cause) {
    console.error("IDKit rejected opening a Selfie Check request", cause);
    return { ok: false, message: "Could not start World ID verification. Try again." };
  }

  deps.onConnectorReady?.(handle.connectorURI);

  const completion = await handle.pollUntilCompletion({ timeout: pollTimeoutMs(rpContext) });
  if (!completion.success) return { ok: false, message: describeWorldIdFailure(completion.error) };
  return { ok: true, proof: completion.result };
}

/// The body `/api/world/verify` expects: a bare token under mock mode, the
/// raw IDKit result alongside it under live mode. Forwarded byte-for-byte —
/// the route expects World's own field names, not ours, so nothing here may
/// reshape `proof`.
export function verifyRequestBody(token: string, proof?: IDKitResult): { token: string; proof?: IDKitResult } {
  return proof === undefined ? { token } : { token, proof };
}

export interface WorldVerifyResult {
  delivered: boolean;
}

/// Posts to `/api/world/verify` and turns its response into either a result
/// or a thrown error, matching the shape `ChallengeActions.verifyHuman` has
/// always expected from this endpoint.
export async function postWorldVerify(token: string, proof?: IDKitResult): Promise<WorldVerifyResult> {
  const response = await fetch("/api/world/verify", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(verifyRequestBody(token, proof)),
  });
  const result = (await response.json().catch(() => null)) as
    | { status?: string; delivered?: boolean; error?: string }
    | null;
  if (result?.status !== "cleared") throw new Error(result?.error ?? "Verification failed");
  return { delivered: result.delivered === true };
}

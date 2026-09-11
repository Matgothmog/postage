import { randomUUID } from "node:crypto";
import { hashSignal } from "@worldcoin/idkit/hashing";
import { type Hex, createWalletClient, getAddress, keccak256, publicActions, stringToBytes } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { redact } from "@/app/api/mail/inbound/faults";
import { publicClient, rpcTransport } from "@/lib/client";
import { HUMAN_REGISTRY, chain, registryAbi } from "@/lib/contracts";
import { challengeByToken, type Challenge } from "@/lib/db/challenges";
import { consumeIssuedContext, purgeExpiredContexts, recordIssuedContext } from "@/lib/db/issued-contexts";
import { claimNullifier, type NullifierClaim } from "@/lib/db/nullifiers";
import { causeMessage } from "@/lib/errors";
import { openGate } from "@/lib/gate";
import { identityMode, required } from "@/lib/env";
import { RP_CONTEXT_TTL_SECONDS } from "@/lib/rp-context";
import { now } from "@/lib/time";
import { GENERIC_WORLD_ID_FAILURE_MESSAGE, worldIdFailureMessage } from "@/lib/world-id-messages";

/// Matches the Selfie Check credential lifetime, so the free lane lapses when
/// the credential does rather than outliving it.
const CREDENTIAL_LIFETIME_SECONDS = 90 * 24 * 60 * 60;

/// Bounds how long `recordPersonhood` will hold this request open waiting for
/// the attestation to mine, well under viem's own 180s default — not because
/// a shorter platform ceiling would otherwise kill the wait. Per Vercel's own
/// documentation (Functions → Configuring → Duration), fluid compute — on by
/// default — puts the maximum duration for a serverless function at 300s on
/// Hobby, Pro and Enterprise alike: longer than viem's own 180s, so a wait
/// that ran the full 180s would finish before any platform timeout, not get
/// killed by one. (The one setup that is shorter — a project still on the
/// legacy, pre-fluid Hobby plan, capped at 10s — kills a 20s wait exactly as
/// readily as a 180s one, so it does not single out 20s as the safe number
/// either.) What actually justifies stopping this early: a sender is
/// watching `/c/[token]` for this to resolve, not polling a queue, so the
/// bound has to be short enough that "pay instead" below is a fallback they
/// actually reach, not one sitting behind a wait nobody stays for.
///
/// What 20s actually buys, counted off viem 2.56's own defaults rather than
/// estimated: `createClient` sizes the polling interval as
/// `min(max(floor(blockTime / 2), 500), 4_000)`, and `arcTestnet` declares no
/// `blockTime`, so viem's 12s fallback puts it at the 4s ceiling.
/// `waitForTransactionReceipt` reads the receipt once immediately, and again
/// each time its block watcher reports a block number it has not seen before —
/// polled every 4s, with the first poll fired straight away (`emitOnBegin`).
/// So 20s is that first read plus up to five more, one per poll that finds the
/// chain has moved on. Reading per new block rather than per poll is the right
/// cadence, since the transaction cannot appear in a block that has already
/// been looked at; the bound only caps how many of those this request waits
/// for, and five is enough to absorb a couple of rounds of ordinary
/// propagation lag without changing what counts as success.
///
/// That accounting holds only because `checkReplacement` is turned off at the
/// call site below; left at its default it spends up to 12.6s of this budget
/// before the first watched receipt read happens at all. See the wait itself.
///
/// Hit the bound and viem throws `WaitForTransactionReceiptTimeoutError`,
/// which the `catch` below treats like every other way this poll can fail:
/// logged with the hash and with what actually went wrong, answered as "the
/// outcome is unknown" rather than as "nothing was saved". The attestation may
/// still land seconds later — the log line is how that gets reconciled by hand
/// — but nothing here grants personhood on a transaction this request never
/// confirmed itself.
const ATTESTATION_RECEIPT_TIMEOUT_MS = 20_000;

const SELFIE_IDENTIFIER = "selfie";
const VERIFY_ENDPOINT = "https://developer.world.org/api/v4";

const attestationTypes = {
  Attestation: [
    { name: "wallet", type: "address" },
    { name: "nullifierHash", type: "bytes32" },
    { name: "expiresAt", type: "uint40" },
  ],
} as const;

/// One credential response inside a proof's `responses` array. World ID 3.0
/// (what Selfie Check still issues, per docs/world-feedback.md) and 4.0 both
/// shape a proof this way. Only the fields the route inspects are named here —
/// the rest of the object passes through to World untouched, since checking
/// the cryptographic parts is World's job, not ours.
///
/// `signal_hash` is present only when the request that produced this proof
/// carried a signal, which is why it is optional here and refused below.
interface WorldIdProofResponse {
  identifier: string;
  signal_hash?: string;
}

/// The shape of the IDKit result the client forwards for verification. This
/// is exactly the object posted on to World's v4 verify endpoint, so its
/// field names are World's: a `responses` array, not a bare flat proof, with
/// each entry identified by `identifier` rather than a verification level.
///
/// `nonce` is the one field every `IDKitResult` variant carries regardless of
/// protocol version — the request this proof answers — and it is what ties
/// the proof back to a signed rp_context this server actually issued.
interface WorldIdProof {
  nonce: string;
  responses: WorldIdProofResponse[];
}

function isWorldIdProof(value: unknown): value is WorldIdProof {
  if (value === null || typeof value !== "object" || !("responses" in value) || !("nonce" in value)) {
    return false;
  }
  const { responses, nonce } = value as { responses: unknown; nonce: unknown };
  return (
    typeof nonce === "string" &&
    Array.isArray(responses) &&
    responses.length > 0 &&
    responses.every((item): item is WorldIdProofResponse => {
      if (item === null || typeof item !== "object") return false;
      const { identifier, signal_hash } = item as { identifier?: unknown; signal_hash?: unknown };
      return typeof identifier === "string" && (signal_hash === undefined || typeof signal_hash === "string");
    })
  );
}

/// Picks out the one Selfie Check credential a proof may carry, and refuses
/// anything else — zero of them or more than one, the same refusal either
/// way, because both are equally not "the one credential this route can
/// reason about."
///
/// The reason more than one matters: the signal check below reads the first
/// `"selfie"` entry in the *request*, and `verifyWithWorld` separately reads
/// the first `"selfie"` entry in World's *response* — two different arrays,
/// walked independently. World's response never echoes back which request
/// entry a result came from (no shared field survives the round trip; see the
/// note on `verifyWithWorld`), so if a request carried two such entries there
/// would be no way to prove the one this route vouched for is the one World's
/// nullifier actually names. A second entry — a harvested proof made for some
/// other challenge, riding alongside a stub that merely carries this
/// challenge's `signal_hash` — could pass the check below on the stub and
/// still hand back the harvested proof's nullifier. Pinning the count to
/// exactly one closes that off by construction: with only one candidate,
/// "the entry checked" and "the entry consumed" cannot be two different
/// things.
function requireSingleSelfieCredential(responses: WorldIdProofResponse[]): WorldIdProofResponse {
  const selfies = responses.filter((item) => item.identifier === SELFIE_IDENTIFIER);
  if (selfies.length !== 1) {
    throw new WorldVerificationError("Proof must include exactly one Selfie Check credential", 400);
  }
  return selfies[0];
}

/// Refuses a proof that was made for some other challenge.
///
/// The signal is the only thing tying a proof to the token it was produced
/// for. World App hashes it into the proof and returns it as `signal_hash`,
/// and the hash is a public input to the zero-knowledge proof — so it cannot
/// be edited without the proof failing at World. That is what makes comparing
/// it here worth anything: a proof that both verifies at World and carries our
/// hash was made for this token and no other.
///
/// World's verify response does not echo the signal back (its result entries
/// are `identifier`, `success`, `nullifier`, `code`, `detail`), so the proof
/// the sender presented is the only place this can be read from.
///
/// A proof carrying no `signal_hash` at all is refused rather than waved
/// through: unbound is the state this exists to reject, not a legacy shape to
/// tolerate.
function requireBoundToChallenge(proof: WorldIdProof, token: string): void {
  const selfie = requireSingleSelfieCredential(proof.responses);
  if (selfie.signal_hash?.toLowerCase() !== hashSignal(token).toLowerCase()) {
    throw new WorldVerificationError("This proof was not made for this challenge", 400);
  }
}

/// Fails loudly and specifically rather than letting a missing or malformed
/// proof reach `verifyWithWorld`. This is an identity-verification boundary,
/// so the request body is untrusted input all the way down to its shape, not
/// just its contents.
function requireWorldIdProof(proof: unknown): WorldIdProof {
  if (proof === undefined || proof === null) {
    throw new WorldVerificationError("A World ID proof is required", 400);
  }
  if (!isWorldIdProof(proof)) {
    throw new WorldVerificationError("Malformed World ID proof", 400);
  }
  return proof;
}

/// Distinguishes a proof World rejected (the sender's fault, 400) from a
/// failure to reach World at all, or a response that made no sense once we
/// got there (ours, 502). Collapsing both into one status would blame a
/// sender for an outage that was never theirs.
///
/// 429 is mock mode's own refusal below, which is not a verdict on a proof at
/// all — there is none — but on how often one token has been answered for. It
/// is raised as one of these rather than returned directly so that everything
/// decided inside the verification block in `POST` leaves it by one path. Only
/// that block: `POST` answers 400, 403, 404 and 502 directly in several other
/// places, before and after it, and those were never routed through here.
class WorldVerificationError extends Error {
  readonly status: 400 | 429 | 502;

  constructor(message: string, status: 400 | 429 | 502) {
    super(message);
    this.status = status;
  }
}

/// Thrown by `recordPersonhood` in exactly one situation: a transaction was
/// broadcast and its outcome never came back — the receipt wait threw,
/// `ATTESTATION_RECEIPT_TIMEOUT_MS` most commonly, but anything else the poll
/// itself can fail with lands here too. Kept distinct from every other
/// failure `recordPersonhood` can throw, which happens either before any hash
/// exists or after a receipt has confirmed a revert, because this is the one
/// case where "nothing was saved" cannot honestly be asserted: the
/// transaction may still mine after this request has already answered.
class AttestationOutcomeUnknown extends Error {
  constructor(hash: Hex, cause: unknown) {
    super(`attestation ${hash} did not confirm before the wait bound`, { cause });
    this.name = "AttestationOutcomeUnknown";
  }
}

/// How far `describeFailure` will follow a `cause` chain. Bounded because a
/// cycle in one would otherwise be an infinite loop inside an error handler,
/// and nothing this route wraps nests anywhere near this deep. `links.length`
/// is checked before every push and grows by exactly one each pass below, so
/// the bound holds regardless of what the chain actually contains — a cycle
/// cannot run this past four rounds to find out.
const MAX_CAUSE_DEPTH = 4;

/// One link in a cause chain, rendered for a log line. `causeMessage`
/// (`@/lib/errors.ts`) answers "what does this `Error` say", so an `Error`
/// link reads through it unchanged; a non-`Error` link — a thrown string,
/// number, or plain object — has no `.message` for it to read, and
/// `causeMessage`'s own `String()` fallback for that case collapses a plain
/// object down to the unhelpful `"[object Object]"`. JSON is what this route
/// already reaches for to serialize a value it does not otherwise know the
/// shape of (`JSON.stringify(proof)` in `verifyWithWorld`), so a non-`Error`
/// link is rendered the same way here, falling back to `causeMessage` only if
/// `JSON.stringify` itself cannot take the value — a circular reference, a
/// bigint.
function chainLinkMessage(link: unknown): string {
  if (link instanceof Error) return causeMessage(link);
  try {
    return JSON.stringify(link) ?? causeMessage(link);
  } catch {
    return causeMessage(link);
  }
}

/// An error and everything it wraps, flattened into one redacted log line.
///
/// Every caught error this file logs goes through here, and
/// `AttestationOutcomeUnknown` is why it has to: its own message names the
/// transaction hash and nothing else, so logging just the top message handed
/// an operator a hash to reconcile by hand with no account of why it needed
/// reconciling — a wait that timed out, an RPC that went away and a receipt
/// read that errored all read identically. Following `.cause` is what puts
/// the diagnosis next to the hash.
///
/// Walks the same way `reasonChain` does in `mail/inbound/faults.ts` — every
/// link rendered by `causeMessage`, the walk never stopped early just because
/// one link happens not to be an `Error` — because both files are logging the
/// same shape of problem: a caught value of unknown type, wrapped who knows
/// how many times. `reasonChain` is not exported, and stays that way; it is
/// private to the mail fault path and out of scope here. So this is that walk
/// written a second time rather than shared, with one addition it does not
/// need — see `chainLinkMessage` for why.
///
/// `redact()` (`mail/inbound/faults.ts`) runs over the joined chain, for the
/// same reason `faultResponse` runs it over `reasonChain`'s output there:
/// `recordPersonhood`'s first act is an RPC read against `ARC_RPC_URL`
/// (`readHumanUntil`, through `publicClient`), and a transport failure's
/// message can carry the full request URL — `SECRET_NAME` already lists
/// `RPC_URL` for exactly this, since a keyed endpoint carries its key in the
/// path. Every caller below only ever passes this return value to
/// `console.error`, never returns it, so redacting once here covers all of
/// them rather than trusting each call site to remember.
function describeFailure(cause: unknown): string {
  const links: string[] = [];
  let link: unknown = cause;
  while (link !== undefined && link !== null && links.length < MAX_CAUSE_DEPTH) {
    links.push(chainLinkMessage(link));
    link = link instanceof Error ? link.cause : undefined;
  }
  return redact(links.length > 0 ? links.join(": ") : chainLinkMessage(cause));
}

/// A stand-in for the challenge token wherever this file logs about a
/// request — the token itself never appears in a log line here. The token is
/// the bearer capability that clears the challenge (see `challengeByToken`),
/// minted from 122 random bits (`mail/inbound/challenge.ts:58`,
/// `randomUUID()`), so anyone with log access who read it back could act as
/// though they held it. Hashing is one-way and the input already has enough
/// entropy that the hash cannot be walked back to it either, so what survives
/// in the log is exactly what an operator needs — several lines carrying the
/// same fingerprint are about the same request — and nothing that works as a
/// credential.
function tokenFingerprint(token: string): string {
  return keccak256(stringToBytes(token));
}

/// Spends the signed context this proof was issued under, and refuses the
/// proof outright if there is none left to spend.
///
/// This is what makes a signed rp_context single-use rather than a bearer
/// credential good for as long as anyone cares to replay it: a proof
/// harvested under our rp and action, or one presented a second time, finds
/// no context left to consume and is refused before World is ever asked
/// about it. A failure to reach the ledger is treated the same as every
/// other ledger check in this file — not knowing whether this nonce is still
/// good is not a reason to let the proof through.
async function spendIssuedContext(token: string, nonce: string): Promise<void> {
  let spent: boolean;
  try {
    spent = await consumeIssuedContext(token, nonce);
  } catch (cause) {
    console.error("issued rp_context ledger unavailable", {
      tokenRef: tokenFingerprint(token),
      reason: describeFailure(cause),
    });
    throw new WorldVerificationError("Could not verify this proof's signing context", 502);
  }
  if (!spent) {
    throw new WorldVerificationError(
      "This proof's signing context was already used, or was never issued for this challenge",
      400
    );
  }
}

/// Whether this challenge has already been answered — and so, in mock mode,
/// whether the chain write below must be skipped.
///
/// `claimChallenge` — the settle-once guard — is the last thing this route
/// reaches, inside `openGate`, so on its own it stops nothing that costs
/// money: the attestation below is signed, sent and mined long before the
/// claim is even attempted, and a challenge settled an hour ago still bought
/// another one. Live mode never had this problem, because `/api/world/context`
/// refuses to sign for a settled challenge (`resolved_at != null`) and so a
/// second proof cannot be produced for one at all.
///
/// Skipping the write rather than refusing the request outright is what keeps
/// `openGate`'s fifth invariant (`gate.ts:41-42` — "answering a challenge that
/// is already settled reports what actually happened, and repairs a sender who
/// holds nothing") true in the one mode production runs. A refusal here would
/// have reached `postWorldVerify` (`world-id.ts`), which throws on any status
/// but `"cleared"`, so a sender whose answer was lost got a hard failure where
/// the repair path was supposed to be. The relayer still pays nothing either
/// way, which was the whole of what refusing was for.
///
/// Read only on the mock branch, so live mode keeps exactly the behaviour it
/// has: there, a context minted while the challenge was still open and
/// presented just after it settled reaches that same recover path already, and
/// it arrives carrying a proof the sender really did just make.
function isAlreadyAnswered(challenge: Challenge): boolean {
  // Loose, like every other settled check in this tree: a column read back as
  // undefined rather than null has to count as answered, so the doubtful case
  // is the one that spends no gas.
  return challenge.resolved_at != null;
}

/// What mock mode has instead of a signed context, and the reason it needs
/// one: without it, a single ordinary challenge token is an unbounded draw on
/// the relayer's vault.
///
/// Live mode never reaches this, and does not need to. Its cost is already
/// bounded by three refusals the mock branch presents nothing to be judged by
/// — `/api/world/context` will not sign for a challenge already answered,
/// `recordIssuedContext` refuses past `MAX_LIVE_CONTEXTS_PER_TOKEN` live
/// contexts for one token, and `spendIssuedContext` burns one per proof. Mock
/// mode posts a bare token, so it skipped all three, and every request that
/// got past them signed and sent a relayer-funded `attest`. Nothing further
/// down catches it either: `recordPersonhood`'s "already fresh" early return
/// only fires while `humanUntil` is at or past this attempt's `expiresAt`,
/// and `expiresAt` moves with the clock, so a request a second later is a
/// genuine extension and mines as one. That is the configuration the deployed
/// app runs under, which is what makes this worth more than tidiness.
///
/// So the same ceiling is taken here, out of the same table and against the
/// same limit rather than as a second mechanism that could drift from it. A
/// slot is taken per attempt and never given back, so what is counted is
/// attempts rather than successes — a request that fails at the chain has
/// still cost the relayer the gas that failed.
///
/// The window is the rp_context TTL because these are the same rows being
/// counted: a mock attempt that outlived the contexts it shares a ceiling
/// with would be counting on a different clock from the live-mode limit it
/// stands in for.
///
/// Keyed on the challenge token, and not on the sender: the token is the
/// bearer capability this whole route is already judged against, so the only
/// person who can spend a token's allowance is whoever holds it, and there is
/// nothing here a stranger could aim at somebody else to lock them out of
/// their own challenge. The sender address would not have that property —
/// it is whatever address mailed the handle.
async function spendMockVerificationSlot(token: string): Promise<void> {
  const at = now();
  let taken: boolean;
  try {
    // Prefixed rather than random alone, so an operator reading the table can
    // tell a slot nothing ever signed from a context that was really issued.
    taken = await recordIssuedContext(token, `mock:${randomUUID()}`, at, at + RP_CONTEXT_TTL_SECONDS);
  } catch (cause) {
    // Fails closed, like every other ledger check in this file: not knowing
    // how much this token has already spent is not a reason to spend more.
    console.error("verification attempt ledger unavailable", {
      tokenRef: tokenFingerprint(token),
      reason: describeFailure(cause),
    });
    throw new WorldVerificationError("Could not verify this challenge", 502);
  }
  if (!taken) {
    // The signing route's own wording for the same ceiling, because it is the
    // same ceiling: which of the two refused a sender is not their business
    // and not a difference they should be able to read.
    throw new WorldVerificationError("Too many verification attempts. Wait a moment and try again.", 429);
  }

  // Purged on the way past, like the held-message and classification purges,
  // and only once a slot has actually been taken — so a caller who is being
  // refused cannot still make this DELETE run at whatever rate they care to
  // post. The signing route purges too, but ahead of its insert rather than
  // after it (`api/world/context/route.ts:59-60`); the order differs on
  // purpose and nothing here is claiming to match it.
  //
  // Outside the fail-closed `try` above, and swallowed, because the slot is
  // already committed by this point and `purgeExpiredContexts` is documented
  // as optional — "nothing depends on it having run", since an expired row is
  // already refused by both statements it deletes for
  // (`db/issued-contexts.ts:70-73`). Inside that `try`, a failed DELETE turned
  // a slot the sender had genuinely paid for into a 502, costing them a fifth
  // of the allowance for a housekeeping step that changes nothing.
  await purgeExpiredContexts().catch((cause: unknown) => {
    console.error("expired rp_context purge failed", {
      tokenRef: tokenFingerprint(token),
      reason: describeFailure(cause),
    });
  });
}

/// The free lane, and it asks for no wallet.
///
/// Someone proving they are a person is not paying for anything, so making them
/// hold an account to do it is a toll on the one path that is supposed to be
/// free. The attestation still goes onchain, against an address derived from
/// the nullifier rather than a wallet: an identity nobody holds the key to,
/// which is all the registry needs it to be. One person still maps to one
/// record, the relayer still pays the gas out of the vault, and the subgraph
/// still sees it.
export async function POST(request: Request) {
  let body: unknown;
  try {
    body = await request.json();
  } catch {
    return Response.json({ error: "Body must be JSON" }, { status: 400 });
  }

  const { token, proof } = body as { token?: string; proof?: unknown };
  if (!token) return Response.json({ error: "A challenge token is required" }, { status: 400 });

  const challenge = await challengeByToken(token);
  if (!challenge) return Response.json({ error: "Unknown challenge" }, { status: 404 });

  // Judged before the replay guard, so an already-answered dangerous token is
  // still refused as dangerous rather than reported as cleared.
  if (challenge.tier === "dangerous") {
    return Response.json(
      { error: "This will not be delivered whoever sends it. Being a person does not change that" },
      { status: 403 }
    );
  }

  let nullifier: string;
  // Cleared on the one path that skips the chain write: mock mode answering a
  // challenge that has already been settled. Everything after this block still
  // runs, including `openGate`. See `isAlreadyAnswered`.
  let attestOnChain = true;
  try {
    if (identityMode() === "live") {
      const presented = requireWorldIdProof(proof);
      // Checked before World is called at all, so a proof pasted in from
      // another challenge costs one string comparison rather than a round trip.
      requireBoundToChallenge(presented, token);
      // Cheaper than the round trip below, and it is what stops a harvested
      // proof — or this same proof presented twice — from ever reaching World.
      await spendIssuedContext(token, presented.nonce);
      nullifier = await verifyWithWorld(presented, token);
    } else {
      // Nothing above ran, because there is no proof to run it against — so
      // what bounds the relayer's exposure to one token is decided here
      // instead, and both halves are mock-only on purpose; see each one.
      //
      // The ceiling is skipped along with the write on an answered challenge,
      // rather than spent anyway: it exists to bound "an unbounded draw on the
      // relayer's vault", and a request that sends nothing is no draw at all.
      // Spending a slot there would only burn allowance the live-mode signing
      // route shares (`MAX_LIVE_CONTEXTS_PER_TOKEN`) on a path that costs
      // nothing, and would put a 429 back in front of the repair this branch
      // exists to let through.
      attestOnChain = !isAlreadyAnswered(challenge);
      if (attestOnChain) await spendMockVerificationSlot(token);
      nullifier = mockNullifier(challenge.sender);
    }
  } catch (cause) {
    if (cause instanceof WorldVerificationError) {
      return Response.json({ error: cause.message }, { status: cause.status });
    }
    // Not a verdict on the proof: this is our own misconfiguration (a
    // `required()` env var missing) or a bug nobody anticipated, and the
    // sender gets to see neither. Logged here, since this is the only place
    // that will ever know it happened, then rethrown so Next answers with a
    // bare 500 and nothing in the body — the same contract
    // `api/world/context/route.test.ts` already pins for the sibling route.
    console.error("world id verification failed unexpectedly", {
      tokenRef: tokenFingerprint(token),
      reason: describeFailure(cause),
    });
    throw cause;
  }

  const nullifierHash = toBytes32(nullifier);
  const identity = identityFor(nullifierHash);
  const expiresAt = now() + CREDENTIAL_LIFETIME_SECONDS;

  // The chain write goes first, and nothing of ours moves until it has landed.
  // Which of the two writes below happens first is the whole of what makes a
  // failed attestation harmless, because they are not equally undoable.
  //
  // An attestation that lands and is then abandoned costs nobody anything:
  // `humanUntil` is read by this route and by the subgraph behind the /network
  // page, and by nothing that opens a gate, so an orphaned record is a true
  // statement about a person that grants them nothing. `claimNullifier` is the
  // opposite. It takes the nullifier off whoever held it and deletes the
  // earned passes they were standing on, in one transaction — so running it
  // first meant a failed attestation answered "could not record the
  // attestation" over a ledger it had already rewritten, stripping a third
  // party's free lane and handing the sender nothing in exchange.
  //
  // Ordering the harmless write in front of the harmful one is what fail-closed
  // means here: every request that reaches `claimNullifier` now is a strict
  // subset of those that reached it before, never a wider one. The price is a
  // chain write that a ledger failure a moment later can orphan — gas, and a
  // record nobody is worse off for. That is the cheap side of the trade.
  //
  // It does not widen the window two senders can race through either, it
  // narrows it: the gap between a claim and the pass `openGate` grants for it
  // used to span a chain write and its receipt, and now spans only this
  // route's remaining database work.
  try {
    if (attestOnChain) await recordPersonhood(identity, nullifierHash, expiresAt);
  } catch (cause) {
    // `recordPersonhood`'s first act is an RPC read against `ARC_RPC_URL`,
    // and a transport failure's `message` carries the full request URL —
    // which can hold a key if the operator points it at a keyed endpoint.
    // Logged for whoever operates this, never returned to whoever called it.
    console.error("world id attestation failed", {
      tokenRef: tokenFingerprint(token),
      identity,
      reason: describeFailure(cause),
    });
    // Two different truths, not one. `AttestationOutcomeUnknown` means a hash
    // was sent and this request never learned what happened to it — "nothing
    // was saved" would be a guess dressed up as fact, since the transaction
    // may still mine after this response goes out. Every other failure here
    // happens before a hash exists at all, or after a receipt has confirmed a
    // revert, and both really do leave the registry exactly where they
    // started, so that claim stays true for them.
    //
    // Neither branch offers "try again". In live mode, `verifyWithWorld` has
    // already spent this sender's one-time World verification by the time
    // `recordPersonhood` can fail at all — World does not give it back
    // because our own infrastructure dropped the ball afterwards — so a
    // retry here almost always means reopening World App only to be refused
    // by the verification-limit rejection above, not a fresh attempt. Mock
    // mode's own ceiling (`spendMockVerificationSlot`) is not permanent the
    // same way, but this response has no way to know which mode produced it,
    // so it is written for the case where "try again" would be advice the
    // sender cannot follow rather than promising something that only
    // sometimes holds. Paying is named because it is ours, not theirs, and it
    // does not depend on the credential that just failed.
    if (cause instanceof AttestationOutcomeUnknown) {
      return Response.json(
        { error: "Could not confirm the attestation in time. It may still complete on its own. Pay instead" },
        { status: 502 }
      );
    }
    return Response.json(
      { error: "Could not record the attestation. Nothing was saved. Pay instead" },
      { status: 502 }
    );
  }

  // Still ahead of the gate, because the ledger is the only account of which
  // address holds this person's free lane: a pass granted with no binding
  // behind it is exactly the farming the ledger exists to stop.
  const ledgerFailure = await bindNullifierToSender(nullifierHash, challenge.sender, token);
  if (ledgerFailure !== null) return ledgerFailure;

  const result = await openGate(token, "human");
  if (result.status === "unknown") {
    return Response.json({ error: "Unknown challenge" }, { status: 404 });
  }

  return Response.json({ ...result, identity, nullifierHash, expiresAt });
}

/// Ties the person the nullifier names to the sender presenting them, taking
/// the binding off whoever held it before. Returns a response to send back if
/// the ledger could not be reached, or `null` to carry on.
///
/// The policy is "one distinct human, one free lane, and the human decides
/// which address holds it". A person coming back is expected — a pass lasts
/// fifteen minutes, so anyone sending a second message an hour later must prove
/// themselves again — so the same sender re-presenting passes. A *different*
/// sender presenting the same nullifier also passes, and the lane moves to
/// them, because the sender a proof binds to is the address on the challenge
/// token rather than the person who took the selfie. Refusing the second sender
/// was the earlier rule, and it made a poisoned binding permanent: mail a
/// handle from your own address, pass the `/c/<token>` link to someone under
/// any pretext, and their nullifier is spent on you for good.
///
/// It does not hand back the farming the ledger exists to stop, because the
/// same write that binds the second address releases the first — one nullifier
/// names one sender, always — and `claimNullifier` now also revokes whatever
/// passes the released address earned by proving personhood, in the same
/// transaction as the move. The pass `openGate` grants lives in `passes` under
/// `(handle, sender)`, separate state from this ledger; earlier this function
/// left it alone, so a released address kept standing in the lane it had
/// already been granted for up to a full window after losing the nullifier
/// that was supposed to account for it. See `EARNED_PASS_CONDITION` in
/// `@/lib/db/passes` for what "earned" means there — it takes two columns,
/// not just `uses_left`, to keep a pass that has ever taken money, including
/// one where the money only stretched an already-unlimited window, out of
/// this DELETE's reach.
async function bindNullifierToSender(nullifierHash: Hex, sender: string, token: string): Promise<Response | null> {
  let claim: NullifierClaim;
  try {
    claim = await claimNullifier(nullifierHash, sender);
  } catch (cause) {
    // Fails closed. The ledger is the only account of which address holds this
    // person's free lane, so opening the gate without writing to it would hand
    // out a lane nothing can later see, move or reason about. Logged for
    // diagnosis, not returned — the rule every internal failure here follows.
    //
    // `sender` is deliberately left out: on its own `nullifierHash` names
    // nobody outside this system (see `schema.ts`'s note on the `nullifiers`
    // table), but paired with the sender address it is exactly the
    // email-to-pseudonym linkage that table exists to avoid persisting
    // anywhere wider than itself — and a log line, once shipped, is wider.
    // `tokenRef` still ties this line back to the same request as whatever
    // else got logged about it.
    console.error("nullifier ledger unavailable", {
      tokenRef: tokenFingerprint(token),
      nullifierHash,
      reason: describeFailure(cause),
    });
    return Response.json({ error: "Could not check this World ID" }, { status: 502 });
  }

  // Logged, never returned: which address a lane came off is somebody else's,
  // and on the path worth worrying about the one asking is the attacker. A move
  // is legitimate often enough not to be an error and consequential enough not
  // to be silent — a burst of them is either a poisoned link being recovered
  // from or somebody working the policy.
  if (claim.rebound) {
    console.error("world id nullifier rebound to a new sender", {
      released_from: claim.releasedFrom,
      claimed_by: sender.toLowerCase(),
    });
  }
  return null;
}

/// An address standing for a person rather than an account. Derived from the
/// nullifier, so one person is one record by construction and nobody holds a
/// key to it — it is a name, not a wallet.
function identityFor(nullifierHash: Hex): Hex {
  return getAddress(`0x${nullifierHash.slice(-40)}`);
}

/// What `humanUntil[identity]` reads on chain right now.
///
/// One helper for both places that ask, and the divergence it exists to stop
/// is not hypothetical: these used to be two verbatim copies of the same read
/// with two different thresholds applied to them — `>= expiresAt` before
/// sending, `>= now()` after a revert — and that gap is exactly what let the
/// revert rescue below wave through failures it was never built for.
async function readHumanUntil(identity: Hex): Promise<number> {
  const humanUntil = await publicClient.readContract({
    address: HUMAN_REGISTRY,
    abi: registryAbi,
    functionName: "humanUntil",
    args: [identity],
  });
  return Number(humanUntil);
}

async function recordPersonhood(identity: Hex, nullifierHash: Hex, expiresAt: number): Promise<void> {
  // Attesting again inside the same second reverts as a non-extension, and a
  // record written moments ago says everything a new one would.
  if ((await readHumanUntil(identity)) >= expiresAt) return;

  const attester = privateKeyToAccount(required("ATTESTER_PRIVATE_KEY") as Hex);
  const signature = await attester.signTypedData({
    domain: { name: "Postage", version: "1", chainId: chain.id, verifyingContract: HUMAN_REGISTRY },
    types: attestationTypes,
    primaryType: "Attestation",
    message: { wallet: identity, nullifierHash, expiresAt },
  });

  const relayer = privateKeyToAccount(required("RELAYER_PRIVATE_KEY") as Hex);
  // Built on the same transport `publicClient` reads through (`rpcTransport`,
  // `@/lib/client`) and extended with the read actions `waitForTransactionReceipt`
  // needs, so the client that waits for this transaction is the same one that
  // signed and sent it — not a second provider that may never have seen it.
  const relayerClient = createWalletClient({ account: relayer, chain, transport: rpcTransport }).extend(
    publicActions
  );
  const hash = await relayerClient.writeContract({
    address: HUMAN_REGISTRY,
    abi: registryAbi,
    functionName: "attest",
    args: [identity, nullifierHash, expiresAt, signature],
  });

  // A receipt is not a success. `waitForTransactionReceipt` resolves a reverted
  // transaction exactly as happily as a mined one, so without this every revert
  // `attest` can throw — `NotAnExtension`, `NullifierAlreadyBound`,
  // `InvalidSignature`, all in `contracts/src/HumanRegistry.sol` — returned
  // from here as though the record had been written, and the caller went on to
  // grant a lane with nothing onchain to account for it. The hash is the one
  // thing that makes such a revert diagnosable afterwards, and it is already
  // public.
  //
  // `checkReplacement` is turned off, against viem's own default of on
  // (`arcTestnet` declares no `supportsTransactionReplacementDetection`, so
  // viem assumes `true`). Two reasons, both read off viem 2.56's source rather
  // than guessed at.
  //
  // What it costs. On the block watcher's first tick it wraps `getTransaction`
  // in `withRetry({ retryCount: 6, retryDelay: ({ count }) => ~~(1 << count) * 200 })`
  // — seven attempts with 200+400+800+1600+3200+6400 ≈ 12.6s of sleeping
  // between them — and the receipt read sits *after* it inside the same tick,
  // so for as long as that runs nothing is asking the question this wait
  // exists to answer. Against a node that has not yet indexed the pending
  // transaction, which a load-balanced `ARC_RPC_URL` makes an ordinary event
  // rather than an exotic one, that spends 12.6s of the 20s bound above before
  // the first watched receipt read happens at all, and reports a transaction
  // that mined at 6s as an outcome nobody knows.
  //
  // What it would buy, here, is worse than nothing. With no `onReplaced`
  // handler passed, the detection just resolves with the replacing
  // transaction's receipt. The relayer key holds no nonce manager — viem reads
  // `eth_getTransactionCount` per transaction — so two `recordPersonhood`
  // calls in flight together can take the same nonce, and the transaction that
  // replaced this one is then some other request's `attest` for some other
  // identity. Reading `status === "success"` off that receipt would be a true
  // statement about the wrong person. Timing out and saying the outcome is
  // unknown is the honest answer to a superseded hash, and it is the one left
  // in place.
  //
  // Wrapped so the timeout above (or any other way this poll can fail) is
  // distinguishable from every other failure this function can throw: a hash
  // exists at this point, and whatever comes back — a mined receipt, a
  // revert, nothing at all within the bound — is genuinely unknown until this
  // resolves. `AttestationOutcomeUnknown` carries that unresolved state out
  // rather than letting it read as an ordinary write failure with nothing
  // sent.
  let receipt: Awaited<ReturnType<typeof relayerClient.waitForTransactionReceipt>>;
  try {
    receipt = await relayerClient.waitForTransactionReceipt({
      hash,
      timeout: ATTESTATION_RECEIPT_TIMEOUT_MS,
      checkReplacement: false,
    });
  } catch (cause) {
    throw new AttestationOutcomeUnknown(hash, cause);
  }
  if (receipt.status !== "success") {
    // A revert here does not always mean nothing was recorded. `attest`'s
    // `NotAnExtension` guard (`contracts/src/HumanRegistry.sol:73`) fires on
    // exactly `expiresAt <= humanUntil[wallet]`, which is what happens when
    // two verifications of the same nullifier race each other: both read
    // `humanUntil` above before either has written anything, both pass the
    // "already fresh" check because neither has landed yet, both sign and
    // send, and whichever lands second reverts against a `humanUntil` the
    // first one just set. That is not a failure to record personhood —
    // personhood is already recorded, by the race's winner, which this same
    // request was equally entitled to be.
    //
    // So the re-read is judged against `expiresAt`: the threshold the pre-send
    // check already used, and the one `NotAnExtension` is itself written in.
    // The two lining up is the whole of what makes this a rescue for that
    // revert rather than an amnesty for every revert. Asking `>= now()`
    // instead asked a different question — "does this identity hold any live
    // record at all" — which every returning sender inside the 90-day
    // credential lifetime answers yes to, so an `InvalidSignature` from a
    // rotated attester key, a `NullifierAlreadyBound`, or a redeployed
    // registry all returned from here as success for anyone already attested
    // and failed only people who had never been.
    //
    // Re-reading chain state rather than decoding the revert is what makes
    // this safe to do at all: `identity` was derived above from the nullifier
    // *this* request already carried through the full proof chain
    // (`requireWorldIdProof` → `requireBoundToChallenge` →
    // `spendIssuedContext` → `verifyWithWorld`), so finding it fresh can only
    // mean some request presenting this exact nullifier already got a valid
    // `attester` signature past `attest` — never that a caller pointed this
    // check at somebody else's identity, which nothing here lets them do.
    const settled = await readHumanUntil(identity);
    const alreadyAtLeastThisFresh = settled >= expiresAt;
    // Logged whichever way it goes, because a rescued revert is still a
    // revert. An attester key that has stopped producing valid signatures
    // fails only senders with no record yet and succeeds silently for everyone
    // who has one, which is the version of this outage hardest to notice; this
    // line is what makes it visible before the pattern is.
    console.error("attestation transaction reverted", {
      hash,
      identity,
      humanUntil: settled,
      expiresAt,
      grantedAnyway: alreadyAtLeastThisFresh,
    });
    if (alreadyAtLeastThisFresh) return;
    throw new Error(`attestation transaction ${hash} reverted`);
  }
}

interface WorldVerifyResponse {
  success?: boolean;
  detail?: string;
  code?: string;
  results?: { identifier: string; success: boolean; nullifier?: string }[];
}

/// Sender-safe copy for a proof World's verify endpoint rejected, looked up
/// from `payload.code` through `worldIdFailureMessage`
/// (`@/lib/world-id-messages.ts`) — the same table `describeWorldIdFailure`
/// reads for IDKit's client-side completion codes, so a code both sides can
/// send (`rp_signature_expired`, `max_verifications_reached`) is described
/// identically regardless of which caught it, and the unrecognised-code
/// fallback is the same `GENERIC_WORLD_ID_FAILURE_MESSAGE` constant rather
/// than a second literal copy of that sentence that could drift from it.
///
/// Through that function rather than by indexing the table directly: `code` is
/// whatever World put in a response body, and an object literal indexed with
/// an inherited key answers with the inherited value — `"toString"` came back
/// as `Object.prototype.toString`, which is not `undefined`, so the fallback
/// below never fired and the sender was handed
/// "function toString() { [native code] }" as their explanation.
///
/// World's own `detail` is prose about its own API, written for a developer
/// debugging an integration rather than a sender reading their mail client
/// (see docs/world-feedback.md:267 for World emitting exactly that register of
/// sentence-form text), so it is logged here and never returned.
///
/// Named for what it does, not just what it returns: the log line below is
/// this function's only record of which code World actually sent, so calling
/// it twice for the same rejection would double it, and moving the logging
/// out to the caller would either lose that record or force the caller to
/// redo the same table lookup just to know what happened. There is exactly
/// one call site today, so the honest fix was the name, not a split that
/// would buy nothing yet.
///
/// Only two buckets are backed by evidence for this specific endpoint:
/// `rp_signature_expired`, whose 300-second signing-context window and
/// failure mode are documented directly (docs/world-feedback.md:325-332),
/// and the verification-limit failure, documented as recurring under three
/// different names across World's own material — `max_verifications_reached`
/// is the one actually compiled into IDKit's shipped binary as a literal enum
/// variant, `exceeded_max_verifications` and `already_verified` are older
/// names still seen in v2-era docs and search results
/// (docs/world-feedback.md:186-196, 314-320) — all three are matched here so
/// whichever name this endpoint happens to send still lands on the right
/// copy. Neither bucket is confirmed against this exact endpoint by any
/// worked example in World's own reference; both are inferred from
/// `idkit-core`'s own `IDKitErrorCode` type, whose doc comment says it
/// "mirrors Rust AppError" — the same backend vocabulary, not a proven
/// shared response shape. A code outside both buckets, known to World or
/// not, is therefore not a surprise: it is logged loudly enough to notice and
/// teach `WORLD_ID_FAILURE_MESSAGES` (`@/lib/world-id-messages.ts`) the new
/// code.
function logAndDescribeWorldVerifyFailure(code: string | undefined, detail: string | undefined, token: string): string {
  const context = { tokenRef: tokenFingerprint(token), code, detail };
  const message = code === undefined ? undefined : worldIdFailureMessage(code);
  console.error(
    message !== undefined
      ? `world id verify endpoint rejected a proof: ${code}`
      : "world id verify endpoint rejected a proof with a code this route does not recognise",
    context
  );
  return message ?? GENERIC_WORLD_ID_FAILURE_MESSAGE;
}

/// Forwards the IDKit result to the Developer Portal and pulls out the selfie
/// credential's nullifier. Selfie Check issues World ID 3.0 proofs, so the
/// result carries a `responses` array rather than a single flat proof.
///
/// Three ways this fails, kept distinct rather than folded into one catch:
/// reaching World at all can fail (network error, ours to answer for), World
/// can reach back and reject the proof (a verdict on what the sender sent),
/// and World can answer 2xx with a body that never mentions Selfie Check
/// (the sender proved something, just not the credential this route needs).
async function verifyWithWorld(proof: WorldIdProof, token: string): Promise<string> {
  const rpId = required("WORLD_RP_ID");

  let response: Response;
  try {
    response = await fetch(`${VERIFY_ENDPOINT}/verify/${rpId}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(proof),
    });
  } catch (cause) {
    console.error("could not reach world id verify endpoint", {
      tokenRef: tokenFingerprint(token),
      reason: describeFailure(cause),
    });
    throw new WorldVerificationError("Could not reach World ID", 502);
  }

  let payload: WorldVerifyResponse;
  try {
    payload = (await response.json()) as WorldVerifyResponse;
  } catch (cause) {
    console.error("world id verify endpoint returned an unreadable response", {
      tokenRef: tokenFingerprint(token),
      status: response.status,
      reason: describeFailure(cause),
    });
    throw new WorldVerificationError("World ID returned an unreadable response", 502);
  }

  // World's own outage is not a verdict on the proof: the taxonomy this route
  // promises is sender's fault at 4xx, World being down at 502, so a 5xx here
  // must not fall through to the rejection branch below and read as a bad
  // request. World's own detail on an outage describes its own failure, not
  // the sender's proof, so it is logged rather than handed back.
  if (response.status >= 500) {
    console.error("world id verify endpoint returned a server error", {
      tokenRef: tokenFingerprint(token),
      status: response.status,
      detail: payload.detail,
      code: payload.code,
    });
    throw new WorldVerificationError("World ID is currently unavailable", 502);
  }

  if (!response.ok || !payload.success) {
    throw new WorldVerificationError(logAndDescribeWorldVerifyFailure(payload.code, payload.detail, token), 400);
  }

  // `results` is walked the same defensive way `requireSingleSelfieCredential`
  // walks the request: World's response carries no field this route could use
  // to prove a given result answers the one selfie entry the request was
  // required to hold (`signal_hash` is request-side only — see the note
  // above `requireBoundToChallenge`), so "exactly one" is enforced here too
  // rather than trusting `.find` to land on the right one by position.
  const selfieResults = payload.results?.filter((item) => item.identifier === SELFIE_IDENTIFIER) ?? [];
  if (selfieResults.length !== 1) {
    throw new WorldVerificationError("Proof did not include a Selfie Check credential", 400);
  }
  const [selfie] = selfieResults;
  if (!selfie.success || !selfie.nullifier) {
    throw new WorldVerificationError("Proof did not include a Selfie Check credential", 400);
  }
  return selfie.nullifier;
}

/// Used until Selfie Check is enabled on the app. Keyed on the sender rather
/// than at random, so the same address behaves like the same person and the
/// uniqueness the nullifier is there to provide is still visible.
function mockNullifier(sender: string): string {
  return keccak256(stringToBytes(`mock-selfie:${sender.toLowerCase()}`));
}

/// Lower-cased on the way out, because this value is both a ledger key and
/// the source of `identityFor`'s address: two spellings of one nullifier must
/// not read as two people, and `getAddress` rejects mixed-case hex outright
/// unless it happens to carry a valid checksum.
function toBytes32(nullifier: string): Hex {
  if (/^0x[0-9a-fA-F]{64}$/.test(nullifier)) return nullifier.toLowerCase() as Hex;
  return keccak256(stringToBytes(nullifier));
}

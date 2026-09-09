import { hashSignal } from "@worldcoin/idkit/hashing";
import { type Hex, createWalletClient, getAddress, http, keccak256, stringToBytes } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { publicClient } from "@/lib/client";
import { HUMAN_REGISTRY, chain, registryAbi } from "@/lib/contracts";
import { challengeByToken } from "@/lib/db/challenges";
import { consumeIssuedContext } from "@/lib/db/issued-contexts";
import { claimNullifier, type NullifierClaim } from "@/lib/db/nullifiers";
import { openGate } from "@/lib/gate";
import { identityMode, required } from "@/lib/env";
import { now } from "@/lib/time";

/// Matches the Selfie Check credential lifetime, so the free lane lapses when
/// the credential does rather than outliving it.
const CREDENTIAL_LIFETIME_SECONDS = 90 * 24 * 60 * 60;

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
class WorldVerificationError extends Error {
  readonly status: 400 | 502;

  constructor(message: string, status: 400 | 502) {
    super(message);
    this.status = status;
  }
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
      reason: cause instanceof Error ? cause.message : cause,
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
  try {
    if (identityMode() === "live") {
      const presented = requireWorldIdProof(proof);
      // Checked before World is called at all, so a proof pasted in from
      // another challenge costs one string comparison rather than a round trip.
      requireBoundToChallenge(presented, token);
      // Cheaper than the round trip below, and it is what stops a harvested
      // proof — or this same proof presented twice — from ever reaching World.
      await spendIssuedContext(token, presented.nonce);
      nullifier = await verifyWithWorld(presented);
    } else {
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
      reason: cause instanceof Error ? cause.message : cause,
    });
    throw cause;
  }

  const nullifierHash = toBytes32(nullifier);

  // Before the chain write and before the gate opens, so a sender who cannot be
  // written into the ledger is never handed the lane the ledger is meant to
  // account for.
  const ledgerFailure = await bindNullifierToSender(nullifierHash, challenge.sender, token);
  if (ledgerFailure !== null) return ledgerFailure;

  const identity = identityFor(nullifierHash);
  const expiresAt = now() + CREDENTIAL_LIFETIME_SECONDS;

  try {
    await recordPersonhood(identity, nullifierHash, expiresAt);
  } catch (cause) {
    // `recordPersonhood`'s first act is an RPC read against `ARC_RPC_URL`,
    // and a transport failure's `message` carries the full request URL —
    // which can hold a key if the operator points it at a keyed endpoint.
    // Logged for whoever operates this, never returned to whoever called it.
    console.error("world id attestation failed", {
      tokenRef: tokenFingerprint(token),
      identity,
      reason: cause instanceof Error ? cause.message : cause,
    });
    return Response.json({ error: "Could not record the attestation" }, { status: 502 });
  }

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
      reason: cause instanceof Error ? cause.message : cause,
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

async function recordPersonhood(identity: Hex, nullifierHash: Hex, expiresAt: number): Promise<void> {
  const alreadyFresh = await publicClient.readContract({
    address: HUMAN_REGISTRY,
    abi: registryAbi,
    functionName: "humanUntil",
    args: [identity],
  });
  // Attesting again inside the same second reverts as a non-extension, and a
  // record written moments ago says everything a new one would.
  if (Number(alreadyFresh) >= expiresAt) return;

  const attester = privateKeyToAccount(required("ATTESTER_PRIVATE_KEY") as Hex);
  const signature = await attester.signTypedData({
    domain: { name: "Postage", version: "1", chainId: chain.id, verifyingContract: HUMAN_REGISTRY },
    types: attestationTypes,
    primaryType: "Attestation",
    message: { wallet: identity, nullifierHash, expiresAt },
  });

  const relayer = privateKeyToAccount(required("RELAYER_PRIVATE_KEY") as Hex);
  const client = createWalletClient({ account: relayer, chain, transport: http() });
  const hash = await client.writeContract({
    address: HUMAN_REGISTRY,
    abi: registryAbi,
    functionName: "attest",
    args: [identity, nullifierHash, expiresAt, signature],
  });
  await publicClient.waitForTransactionReceipt({ hash });
}

interface WorldVerifyResponse {
  success?: boolean;
  detail?: string;
  code?: string;
  results?: { identifier: string; success: boolean; nullifier?: string }[];
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
async function verifyWithWorld(proof: WorldIdProof): Promise<string> {
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
      reason: cause instanceof Error ? cause.message : cause,
    });
    throw new WorldVerificationError("Could not reach World ID", 502);
  }

  let payload: WorldVerifyResponse;
  try {
    payload = (await response.json()) as WorldVerifyResponse;
  } catch (cause) {
    console.error("world id verify endpoint returned an unreadable response", {
      status: response.status,
      reason: cause instanceof Error ? cause.message : cause,
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
      status: response.status,
      detail: payload.detail,
      code: payload.code,
    });
    throw new WorldVerificationError("World ID is currently unavailable", 502);
  }

  if (!response.ok || !payload.success) {
    throw new WorldVerificationError(payload.detail ?? payload.code ?? "World ID rejected the proof", 400);
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

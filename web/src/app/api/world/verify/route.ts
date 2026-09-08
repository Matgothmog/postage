import { type Hex, createWalletClient, getAddress, http, keccak256, stringToBytes } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { publicClient } from "@/lib/client";
import { HUMAN_REGISTRY, chain, registryAbi } from "@/lib/contracts";
import {
  challengeByToken,
  claimChallenge,
  grantPass,
  hasLivePass,
  releaseChallengeClaim,
} from "@/lib/db";
import { identityMode, required } from "@/lib/env";
import { releaseHeldMessage } from "@/lib/hold";

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
    nullifier =
      identityMode() === "live" ? await verifyWithWorld(proof) : mockNullifier(challenge.sender);
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : "Verification failed";
    return Response.json({ error: detail }, { status: 400 });
  }

  const nullifierHash = toBytes32(nullifier);
  const identity = identityFor(nullifierHash);
  const expiresAt = Math.floor(Date.now() / 1000) + CREDENTIAL_LIFETIME_SECONDS;

  try {
    await recordPersonhood(identity, nullifierHash, expiresAt);
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : "Could not record the attestation";
    return Response.json({ error: detail }, { status: 502 });
  }

  // Claimed before the pass is minted, not after. Recording personhood waits on
  // a transaction receipt, so two posts of one token overlap easily, and
  // granting first would mint two windows from one proof before either lost.
  // Answered already. That is not a reason to turn someone away: recording
  // personhood waits on a transaction, so a slow reply and an impatient reload
  // are ordinary, and a hold that lapsed leaves them needing a pass to paste
  // what they wrote. The guard that matters is above — a fresh proof had to be
  // presented to get this far — so what is left is to make sure they hold what
  // that proof earned, without minting a second window on top of a live one.
  if (!(await claimChallenge(token, "human"))) {
    const settled = await challengeByToken(token);
    if (
      settled?.delivered_at == null &&
      !(await hasLivePass(challenge.handle, challenge.sender))
    ) {
      await grantPass(challenge.handle, challenge.sender, "human", null);
    }
    return Response.json({
      status: "cleared",
      reason: settled?.settled_by ?? "human",
      delivered: settled?.delivered_at != null,
    });
  }

  try {
    await grantPass(challenge.handle, challenge.sender, "human", null);
  } catch (cause) {
    // Claimed for a pass that was never minted. Left as it stands the sender
    // has proved they are a person, holds nothing, and every retry tells them
    // it is already answered.
    await releaseChallengeClaim(token).catch(() => {});
    throw cause;
  }

  return Response.json({
    status: "cleared",
    reason: "human",
    identity,
    nullifierHash,
    expiresAt,
    ...(await releaseHeldMessage(token, challenge.handle)),
  });
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

/// Forwards the IDKit result to the Developer Portal and pulls out the selfie
/// credential's nullifier. Selfie Check issues World ID 3.0 proofs, so the
/// result carries a `responses` array rather than a single flat proof.
async function verifyWithWorld(proof: unknown): Promise<string> {
  if (!proof || typeof proof !== "object") throw new Error("Missing World ID proof");

  const rpId = required("WORLD_RP_ID");
  const response = await fetch(`${VERIFY_ENDPOINT}/verify/${rpId}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(proof),
  });

  const payload = (await response.json()) as {
    success?: boolean;
    detail?: string;
    code?: string;
    results?: { identifier: string; success: boolean; nullifier?: string }[];
  };

  if (!response.ok || !payload.success) {
    throw new Error(payload.detail ?? payload.code ?? "World ID rejected the proof");
  }

  const selfie = payload.results?.find((item) => item.identifier === SELFIE_IDENTIFIER);
  if (!selfie?.success || !selfie.nullifier) {
    throw new Error("Proof did not include a Selfie Check credential");
  }
  return selfie.nullifier;
}

/// Used until Selfie Check is enabled on the app. Keyed on the sender rather
/// than at random, so the same address behaves like the same person and the
/// uniqueness the nullifier is there to provide is still visible.
function mockNullifier(sender: string): string {
  return keccak256(stringToBytes(`mock-selfie:${sender.toLowerCase()}`));
}

function toBytes32(nullifier: string): Hex {
  if (/^0x[0-9a-fA-F]{64}$/.test(nullifier)) return nullifier as Hex;
  return keccak256(stringToBytes(nullifier));
}

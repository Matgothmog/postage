import { type Hex, createWalletClient, http, isAddress, keccak256, stringToBytes } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { publicClient } from "@/lib/client";
import { HUMAN_REGISTRY, chain, registryAbi } from "@/lib/contracts";
import { identityMode, required } from "@/lib/env";

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

export async function POST(request: Request) {
  let body: unknown;
  try {
    body = await request.json();
  } catch {
    return Response.json({ error: "Body must be JSON" }, { status: 400 });
  }

  const { wallet, proof } = body as { wallet?: string; proof?: unknown };
  if (!wallet || !isAddress(wallet)) {
    return Response.json({ error: "A valid wallet address is required" }, { status: 400 });
  }

  let nullifier: string;
  try {
    nullifier =
      identityMode() === "live"
        ? await verifyWithWorld(proof)
        : mockNullifier(wallet);
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : "Verification failed";
    return Response.json({ error: detail }, { status: 400 });
  }

  const expiresAt = Math.floor(Date.now() / 1000) + CREDENTIAL_LIFETIME_SECONDS;
  const nullifierHash = toBytes32(nullifier);

  const account = privateKeyToAccount(required("ATTESTER_PRIVATE_KEY") as Hex);
  const signature = await account.signTypedData({
    domain: {
      name: "Postage",
      version: "1",
      chainId: chain.id,
      verifyingContract: HUMAN_REGISTRY,
    },
    types: attestationTypes,
    primaryType: "Attestation",
    message: { wallet, nullifierHash, expiresAt },
  });

  // The relayer posts it, funded by the vault out of postage that recipients
  // claimed from spam. The signature names the wallet, so who submits it
  // changes nothing about who it belongs to.
  let transactionHash: string;
  try {
    transactionHash = await sponsorAttestation(wallet, nullifierHash, expiresAt, signature);
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : "Could not sponsor the attestation";
    return Response.json(
      { wallet, nullifierHash, expiresAt, signature, sponsored: false, error: detail },
      { status: 502 }
    );
  }

  return Response.json({
    wallet,
    nullifierHash,
    expiresAt,
    signature,
    sponsored: true,
    transactionHash,
  });
}

async function sponsorAttestation(
  wallet: Hex,
  nullifierHash: Hex,
  expiresAt: number,
  signature: Hex
): Promise<string> {
  const relayer = privateKeyToAccount(required("RELAYER_PRIVATE_KEY") as Hex);
  const client = createWalletClient({ account: relayer, chain, transport: http() });

  const hash = await client.writeContract({
    address: HUMAN_REGISTRY,
    abi: registryAbi,
    functionName: "attest",
    args: [wallet, nullifierHash, expiresAt, signature],
  });

  await publicClient.waitForTransactionReceipt({ hash });
  return hash;
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

/// Used until Selfie Check is enabled on the app. Deterministic per wallet, so
/// it behaves like a real nullifier: the same person cannot claim two of them.
function mockNullifier(wallet: string): string {
  return keccak256(stringToBytes(`mock-selfie:${wallet.toLowerCase()}`));
}

function toBytes32(nullifier: string): Hex {
  if (/^0x[0-9a-fA-F]{64}$/.test(nullifier)) return nullifier as Hex;
  return keccak256(stringToBytes(nullifier));
}

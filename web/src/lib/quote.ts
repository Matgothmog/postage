import { type Hex, keccak256, stringToBytes } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { POSTAGE_ESCROW, chain } from "./contracts";
import { TIER_INDEX, type Tier } from "./classify";
import { required } from "./env";

/// How long a sender has to act on a price before it must be requoted. Short
/// enough that a cheap quote cannot be banked, long enough to click a link and
/// find a wallet.
const QUOTE_TTL_SECONDS = 24 * 60 * 60;

const types = {
  Quote: [
    { name: "messageId", type: "bytes32" },
    { name: "inbox", type: "address" },
    { name: "tier", type: "uint8" },
    { name: "amount", type: "uint256" },
    { name: "expiresAt", type: "uint40" },
  ],
} as const;

export interface SignedQuote {
  messageId: Hex;
  inbox: Hex;
  tier: Tier;
  amount: string;
  expiresAt: number;
  signature: Hex;
}

/// Derived from the message rather than random, so a quote is bound to the mail
/// it was issued for and cannot be spent on a different one.
export function messageIdFor(sender: string, handle: string, subject: string, receivedAt: number): Hex {
  return keccak256(stringToBytes(`${sender.toLowerCase()}|${handle.toLowerCase()}|${subject}|${receivedAt}`));
}

export async function signQuote(
  messageId: Hex,
  inbox: Hex,
  tier: Tier,
  amount: bigint
): Promise<SignedQuote> {
  const signer = privateKeyToAccount(required("CLASSIFIER_PRIVATE_KEY") as Hex);
  const expiresAt = Math.floor(Date.now() / 1000) + QUOTE_TTL_SECONDS;

  const signature = await signer.signTypedData({
    domain: { name: "Postage", version: "2", chainId: chain.id, verifyingContract: POSTAGE_ESCROW },
    types,
    primaryType: "Quote",
    message: { messageId, inbox, tier: TIER_INDEX[tier], amount, expiresAt },
  });

  return { messageId, inbox, tier, amount: amount.toString(), expiresAt, signature };
}

import { createHmac } from "node:crypto";
import type { Hex } from "viem";
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

/// Derived from the message, so a quote is bound to the mail it was issued for
/// and cannot be spent on a different one.
///
/// Keyed rather than hashed plainly, because this id is published onchain and
/// every part of the message it names is guessable: the handle is public by
/// design, the sender is a short list, the subject of paid mail is templated,
/// and the second it arrived is bounded by the block. A bare keccak of those
/// is a preimage anyone can search, which would turn the ledger into a record
/// of who writes to whom. Under HMAC it is a commitment instead.
export function messageIdFor(sender: string, handle: string, subject: string, receivedAt: number): Hex {
  const preimage = `${sender.toLowerCase()}|${handle.toLowerCase()}|${subject}|${receivedAt}`;
  const digest = createHmac("sha256", required("MESSAGE_ID_SECRET")).update(preimage).digest("hex");
  return `0x${digest}`;
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

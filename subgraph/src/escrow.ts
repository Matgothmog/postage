import { BigInt, Bytes } from "@graphprotocol/graph-ts";
import {
  PriceSet,
  StampClaimed,
  StampExpired,
  StampPosted,
  StampReleased,
} from "../generated/PostageEscrow/PostageEscrow";
import { Stamp } from "../generated/schema";
import { loadInbox, loadSender, refreshSpamRate } from "./shared";

export function handlePriceSet(event: PriceSet): void {
  const inbox = loadInbox(event.params.inbox);
  inbox.price = event.params.amount;
  inbox.save();
}

export function handleStampPosted(event: StampPosted): void {
  const sender = loadSender(event.params.sender, event.block.timestamp);
  sender.stampsPosted += 1;
  sender.totalEscrowed = sender.totalEscrowed.plus(event.params.amount);
  sender.save();

  const inbox = loadInbox(event.params.recipient);
  inbox.receivedCount += 1;
  inbox.save();

  const stamp = new Stamp(event.params.messageId);
  stamp.sender = sender.id;
  stamp.recipient = inbox.id;
  stamp.amount = event.params.amount;
  stamp.status = "Held";
  stamp.postedAt = event.block.timestamp;
  stamp.postedTx = event.transaction.hash;
  stamp.save();
}

export function handleStampReleased(event: StampReleased): void {
  const stamp = settle(event.params.messageId, "Released", event.block.timestamp, event.transaction.hash);
  if (stamp == null) return;

  const sender = loadSender(stamp.sender, event.block.timestamp);
  sender.releasedCount += 1;
  sender.settledCount += 1;
  refreshSpamRate(sender);
  sender.save();

  const inbox = loadInbox(stamp.recipient);
  inbox.releasedCount += 1;
  inbox.save();
}

export function handleStampClaimed(event: StampClaimed): void {
  const stamp = settle(event.params.messageId, "Claimed", event.block.timestamp, event.transaction.hash);
  if (stamp == null) return;

  const sender = loadSender(stamp.sender, event.block.timestamp);
  sender.claimedCount += 1;
  sender.settledCount += 1;
  refreshSpamRate(sender);
  sender.save();

  const inbox = loadInbox(stamp.recipient);
  inbox.claimedCount += 1;
  // What the recipient actually kept, not the face value of the stamp. The
  // rest went to the vault.
  inbox.claimedTotal = inbox.claimedTotal.plus(event.params.amount);
  inbox.save();
}

/// Expiry means the recipient never judged the message, so it counts against
/// nobody. The sender gets their postage back and their spam rate is untouched.
export function handleStampExpired(event: StampExpired): void {
  settle(event.params.messageId, "Expired", event.block.timestamp, event.transaction.hash);
}

function settle(messageId: Bytes, status: string, timestamp: BigInt, txHash: Bytes): Stamp | null {
  const stamp = Stamp.load(messageId);
  if (stamp == null) return null;

  stamp.status = status;
  stamp.settledAt = timestamp;
  stamp.settledTx = txHash;
  stamp.save();
  return stamp;
}

import { BigDecimal, BigInt, Bytes } from "@graphprotocol/graph-ts";
import { Inbox, Sender } from "../generated/schema";

export function loadSender(address: Bytes, timestamp: BigInt): Sender {
  let sender = Sender.load(address);
  if (sender != null) return sender;

  sender = new Sender(address);
  sender.stampsPosted = 0;
  sender.totalEscrowed = BigInt.zero();
  sender.releasedCount = 0;
  sender.claimedCount = 0;
  sender.settledCount = 0;
  sender.spamRate = BigDecimal.zero();
  sender.firstSeenAt = timestamp;
  return sender;
}

export function loadInbox(address: Bytes): Inbox {
  let inbox = Inbox.load(address);
  if (inbox != null) return inbox;

  inbox = new Inbox(address);
  inbox.price = BigInt.zero();
  inbox.receivedCount = 0;
  inbox.releasedCount = 0;
  inbox.claimedCount = 0;
  inbox.claimedTotal = BigInt.zero();
  return inbox;
}

/// Only meaningful once something has actually been settled, so an unsettled
/// sender reads as zero rather than as trustworthy.
export function refreshSpamRate(sender: Sender): void {
  if (sender.settledCount == 0) {
    sender.spamRate = BigDecimal.zero();
    return;
  }
  sender.spamRate = BigDecimal.fromString(sender.claimedCount.toString()).div(
    BigDecimal.fromString(sender.settledCount.toString())
  );
}

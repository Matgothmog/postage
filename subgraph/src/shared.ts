import { BigDecimal, BigInt, Bytes } from "@graphprotocol/graph-ts";
import { Inbox, Sender } from "../generated/schema";

export function loadSender(address: Bytes, timestamp: BigInt): Sender {
  let sender = Sender.load(address);
  if (sender != null) return sender;

  sender = new Sender(address);
  sender.paidCount = 0;
  sender.totalPaid = BigInt.zero();
  sender.spamReports = 0;
  sender.spamRate = BigDecimal.zero();
  sender.firstSeenAt = timestamp;
  return sender;
}

export function loadInbox(address: Bytes): Inbox {
  let inbox = Inbox.load(address);
  if (inbox != null) return inbox;

  inbox = new Inbox(address);
  inbox.floorPrice = BigInt.zero();
  inbox.receivedCount = 0;
  inbox.earned = BigInt.zero();
  inbox.claimed = BigInt.zero();
  return inbox;
}

/// Only meaningful once something has been paid for, so a sender with no
/// history reads as zero rather than as trustworthy.
export function refreshSpamRate(sender: Sender): void {
  if (sender.paidCount == 0) {
    sender.spamRate = BigDecimal.zero();
    return;
  }
  sender.spamRate = BigDecimal.fromString(sender.spamReports.toString()).div(
    BigDecimal.fromString(sender.paidCount.toString())
  );
}

/// Matches the Tier enum in PostageEscrow.
export function tierName(index: i32): string {
  if (index == 0) return "Human";
  if (index == 1) return "Important";
  if (index == 2) return "Commercial";
  return "Dangerous";
}

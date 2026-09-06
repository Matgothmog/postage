import {
  EarningsClaimed,
  FloorPriceSet,
  Paid,
  SpamReported,
} from "../generated/PostageEscrow/PostageEscrow";
import { Payment } from "../generated/schema";
import { loadInbox, loadSender, refreshSpamRate, tierName } from "./shared";

export function handleFloorPriceSet(event: FloorPriceSet): void {
  const inbox = loadInbox(event.params.inbox);
  inbox.floorPrice = event.params.amount;
  inbox.save();
}

export function handlePaid(event: Paid): void {
  const sender = loadSender(event.params.sender, event.block.timestamp);
  sender.paidCount += 1;
  sender.totalPaid = sender.totalPaid.plus(event.params.amount.plus(event.params.toVault));
  refreshSpamRate(sender);
  sender.save();

  const inbox = loadInbox(event.params.inbox);
  inbox.receivedCount += 1;
  // What the inbox actually keeps, not the face value. The rest went to the vault.
  inbox.earned = inbox.earned.plus(event.params.amount);
  inbox.save();

  const payment = new Payment(event.params.messageId);
  payment.sender = sender.id;
  payment.inbox = inbox.id;
  payment.tier = tierName(event.params.tier);
  payment.amount = event.params.amount;
  payment.toVault = event.params.toVault;
  payment.paidAt = event.block.timestamp;
  payment.tx = event.transaction.hash;
  payment.reportedAsSpam = false;
  payment.save();
}

/// The recipient disagreeing with the classifier after the fact. No money
/// moves; this is the signal that prices the sender worse next time.
export function handleSpamReported(event: SpamReported): void {
  const payment = Payment.load(event.params.messageId);
  // The escrow allows one report per message, and counting a replayed event
  // twice would move a price on nothing.
  if (payment == null || payment.reportedAsSpam) return;

  payment.reportedAsSpam = true;
  payment.save();

  const sender = loadSender(event.params.sender, event.block.timestamp);
  sender.spamReports += 1;
  refreshSpamRate(sender);
  sender.save();
}

export function handleEarningsClaimed(event: EarningsClaimed): void {
  const inbox = loadInbox(event.params.inbox);
  inbox.claimed = inbox.claimed.plus(event.params.amount);
  inbox.save();
}

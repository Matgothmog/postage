import { queryPostage } from "./graph";
import { now } from "./time";

export const OVERVIEW = `
  query Overview {
    vaults(first: 1) { totalFunded toTreasury toSponsorship refilledToRelayer fundingEvents }
    enclaves(first: 5) { id measurement revoked registeredAt }
    humanAttestations(first: 500, orderBy: attestedAt, orderDirection: desc) { id attestedAt }
    inboxes(first: 12, orderBy: earned, orderDirection: desc) {
      id floorPrice receivedCount earned claimed
    }
    senders(first: 12, orderBy: paidCount, orderDirection: desc) {
      id paidCount totalPaid spamReports spamRate humanUntil
    }
    payments(first: 25, orderBy: paidAt, orderDirection: desc) {
      id tier amount toVault reportedAsSpam paidAt tx sender { id } inbox { id }
    }
  }
`;

export interface Overview {
  vaults: {
    totalFunded: string;
    toTreasury: string;
    toSponsorship: string;
    refilledToRelayer: string;
    fundingEvents: number;
  }[];
  enclaves: { id: string; measurement: string; revoked: boolean; registeredAt: string }[];
  humanAttestations: { id: string; attestedAt: string }[];
  inboxes: { id: string; floorPrice: string; receivedCount: number; earned: string; claimed: string }[];
  senders: {
    id: string;
    paidCount: number;
    totalPaid: string;
    spamReports: number;
    spamRate: string;
    humanUntil: string | null;
  }[];
  payments: {
    id: string;
    tier: string;
    amount: string;
    toVault: string;
    reportedAsSpam: boolean;
    paidAt: string;
    tx: string;
    sender: { id: string };
    inbox: { id: string };
  }[];
}

/// Gas one attestation costs on Arc at 25 gwei, measured. Used to state the
/// sponsorship pool in the unit that means something: people onboarded.
export const ATTESTATION_COST = 75_395n * 25_000_000_000n;

export interface NetworkAggregates {
  sponsored: bigint;
  earned: bigint;
  delivered: number;
  signer: Overview["enclaves"][number] | undefined;
}

/// The four numbers the page's header and footer sections need beyond a
/// straight render of what the subgraph returned. Kept apart from JSX so
/// these can be checked with a test instead of a screenshot.
export function deriveAggregates(data: Overview): NetworkAggregates {
  const vault = data.vaults[0];
  const sponsored = vault ? BigInt(vault.toSponsorship) / ATTESTATION_COST : 0n;
  const earned = data.inboxes.reduce((total, inbox) => total + BigInt(inbox.earned), 0n);
  const delivered = data.inboxes.reduce((total, inbox) => total + inbox.receivedCount, 0);
  const signer = data.enclaves.find((enclave) => !enclave.revoked);
  return { sponsored, earned, delivered, signer };
}

/// PostageEscrow emits `msg.value - toVault` as Payment.amount — the inbox's
/// net share, not what the sender paid. The full amount the sender's wallet
/// left is `amount + toVault`; anywhere the page states what a sender paid,
/// it must go through this rather than reading `amount` alone.
export function paymentTotal(payment: { amount: string; toVault: string }): bigint {
  return BigInt(payment.amount) + BigInt(payment.toVault);
}

/// A credential that has run out says nothing about who is sending now, so it
/// stops counting the moment it lapses.
export function stillHuman(humanUntil: string | null): boolean {
  return humanUntil !== null && Number(humanUntil) > now();
}

/// Whole units only. A feed that says "14 minutes ago" beside "3 hours ago"
/// reads at a glance; one that says "14 minutes 6 seconds" does not.
export function since(seconds: number): string {
  const elapsed = Math.max(0, now() - seconds);
  if (elapsed < 60) return "just now";
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)}m ago`;
  if (elapsed < 86_400) return `${Math.floor(elapsed / 3600)}h ago`;
  return `${Math.floor(elapsed / 86_400)}d ago`;
}

export function fetchOverview(): Promise<Overview> {
  return queryPostage<Overview>(OVERVIEW);
}

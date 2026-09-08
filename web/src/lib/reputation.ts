import { ENS_SUBGRAPH, queryNetwork, queryPostage } from "./graph";

/// What a price is allowed to move on. Personhood is deliberately absent:
/// proving it clears the message in front of you rather than discounting the
/// next one, so a wallet does not carry it. Every field here is read by
/// `quote`; anything nothing reads is a query nobody should pay for.
export interface SenderSignals {
  paidCount: number;
  spamReports: number;
  spamRate: number;
  /// From public Subgraphs on the network.
  ensNames: number;
  oldestEnsAt: number | null;
}

const POSTAGE_HISTORY = `
  query SenderHistory($wallet: ID!) {
    sender(id: $wallet) {
      paidCount
      spamReports
      spamRate
    }
  }
`;

const ENS_OWNED = `
  query NamesOwned($wallet: String!) {
    domains(first: 5, where: { owner: $wallet }) {
      createdAt
    }
  }
`;

interface PostageResult {
  sender: {
    paidCount: number;
    spamReports: number;
    spamRate: string;
  } | null;
}

interface EnsResult {
  domains: { createdAt: string }[];
}

/// Both lookups are independent, and a failure in either should soften the
/// price rather than block the message, so they settle rather than throw.
export async function gatherSignals(wallet: string): Promise<SenderSignals> {
  const id = wallet.toLowerCase();

  const [history, ens] = await Promise.allSettled([
    queryPostage<PostageResult>(POSTAGE_HISTORY, { wallet: id }),
    queryNetwork<EnsResult>(ENS_SUBGRAPH, ENS_OWNED, { wallet: id }),
  ]);

  const sender = history.status === "fulfilled" ? history.value.sender : null;
  const domains = ens.status === "fulfilled" ? ens.value.domains : [];

  return {
    paidCount: sender?.paidCount ?? 0,
    spamReports: sender?.spamReports ?? 0,
    spamRate: sender ? Number(sender.spamRate) : 0,
    ensNames: domains.length,
    oldestEnsAt: domains.length
      ? Math.min(...domains.map((domain) => Number(domain.createdAt)))
      : null,
  };
}

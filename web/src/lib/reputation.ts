import { ENS_SUBGRAPH, queryNetwork, queryPostage } from "./graph";

export interface SenderSignals {
  wallet: string;
  /// From our Subgraph, mirrored off the onchain World ID attestation.
  isHuman: boolean;
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
      humanUntil
    }
  }
`;

const ENS_OWNED = `
  query NamesOwned($wallet: String!) {
    domains(first: 5, where: { owner: $wallet }) {
      name
      createdAt
    }
  }
`;

interface PostageResult {
  sender: {
    paidCount: number;
    spamReports: number;
    spamRate: string;
    humanUntil: string | null;
  } | null;
}

interface EnsResult {
  domains: { name: string; createdAt: string }[];
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

  const humanUntil = sender?.humanUntil ? Number(sender.humanUntil) : 0;

  return {
    wallet: id,
    isHuman: humanUntil > Math.floor(Date.now() / 1000),
    paidCount: sender?.paidCount ?? 0,
    spamReports: sender?.spamReports ?? 0,
    spamRate: sender ? Number(sender.spamRate) : 0,
    ensNames: domains.length,
    oldestEnsAt: domains.length
      ? Math.min(...domains.map((domain) => Number(domain.createdAt)))
      : null,
  };
}

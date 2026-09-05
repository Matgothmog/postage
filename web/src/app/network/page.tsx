import { queryPostage } from "@/lib/graph";
import { formatUsdc, shortAddress } from "@/lib/format";

export const dynamic = "force-dynamic";

const OVERVIEW = `
  query Overview {
    vaults(first: 1) {
      totalFunded
      toTreasury
      toSponsorship
      refilledToRelayer
      fundingEvents
    }
    humanAttestations(first: 50, orderBy: attestedAt, orderDirection: desc) {
      id
      expiresAt
      attestedAt
    }
    senders(first: 25, orderBy: stampsPosted, orderDirection: desc) {
      id
      stampsPosted
      totalEscrowed
      releasedCount
      claimedCount
      settledCount
      spamRate
      humanUntil
    }
    stamps(first: 15, orderBy: postedAt, orderDirection: desc) {
      id
      amount
      status
      postedAt
      sender { id }
    }
    inboxes(first: 10) {
      id
      price
      receivedCount
      claimedCount
    }
  }
`;

interface Overview {
  vaults: {
    totalFunded: string;
    toTreasury: string;
    toSponsorship: string;
    refilledToRelayer: string;
    fundingEvents: number;
  }[];
  humanAttestations: { id: string; expiresAt: string; attestedAt: string }[];
  senders: {
    id: string;
    stampsPosted: number;
    totalEscrowed: string;
    releasedCount: number;
    claimedCount: number;
    settledCount: number;
    spamRate: string;
    humanUntil: string | null;
  }[];
  stamps: {
    id: string;
    amount: string;
    status: string;
    postedAt: string;
    sender: { id: string };
  }[];
  inboxes: { id: string; price: string; receivedCount: number; claimedCount: number }[];
}

/// Gas a single attestation costs on Arc, measured at 25 gwei. Used to express
/// the vault balance in the unit that actually matters: people onboarded.
const ATTESTATION_COST = 75_395n * 25_000_000_000n;

export default async function NetworkPage() {
  let data: Overview | null = null;
  let failure: string | null = null;

  try {
    data = await queryPostage<Overview>(OVERVIEW);
  } catch (cause) {
    failure = cause instanceof Error ? cause.message : String(cause);
  }

  if (!data) {
    return (
      <Shell>
        <p className="rounded-xl border border-red-200 bg-red-50 p-4 text-sm text-red-700">
          Could not reach the subgraph. {failure}
        </p>
      </Shell>
    );
  }

  const vault = data.vaults[0];
  const sponsored = vault ? BigInt(vault.toSponsorship) / ATTESTATION_COST : 0n;
  const verified = data.humanAttestations.length;

  return (
    <Shell>
      <header>
        <h1 className="text-xl font-semibold tracking-tight">The network</h1>
        <p className="mt-1 text-sm text-neutral-600">
          Every stamp, settlement and verification, read from the subgraph indexing Arc.
        </p>
      </header>

      <div className="mt-6 grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Stat label="People verified" value={String(verified)} note="free to send" />
        <Stat
          label="Spam collected"
          value={vault ? formatUsdc(BigInt(vault.totalFunded)) : "0"}
          note={vault ? `over ${vault.fundingEvents} claims` : "no claims yet"}
        />
        <Stat
          label="Funds verification"
          value={vault ? formatUsdc(BigInt(vault.toSponsorship)) : "0"}
          note={`pays for ~${sponsored} more`}
        />
        <Stat
          label="Kept for the protocol"
          value={vault ? formatUsdc(BigInt(vault.toTreasury)) : "0"}
          note="30% of the vault"
        />
      </div>

      <Section title="Senders" hint="Reputation is what decides the price a stranger pays.">
        <table className="w-full text-sm">
          <thead className="text-left text-xs uppercase tracking-wide text-neutral-500">
            <tr>
              <th className="pb-2 font-medium">Address</th>
              <th className="pb-2 font-medium">Sent</th>
              <th className="pb-2 font-medium">Refunded</th>
              <th className="pb-2 font-medium">Spam</th>
              <th className="pb-2 text-right font-medium">Standing</th>
            </tr>
          </thead>
          <tbody>
            {data.senders.map((sender) => {
              const human =
                sender.humanUntil !== null &&
                Number(sender.humanUntil) > Math.floor(Date.now() / 1000);
              const rate = Number(sender.spamRate);
              return (
                <tr key={sender.id} className="border-t border-neutral-100">
                  <td className="py-2 font-mono">{shortAddress(sender.id)}</td>
                  <td className="py-2">{sender.stampsPosted}</td>
                  <td className="py-2">{sender.releasedCount}</td>
                  <td className="py-2">{sender.claimedCount}</td>
                  <td className="py-2 text-right">
                    {human ? (
                      <Tag tone="good">verified person</Tag>
                    ) : sender.settledCount === 0 ? (
                      <Tag tone="quiet">no history</Tag>
                    ) : rate >= 0.5 ? (
                      <Tag tone="bad">{Math.round(rate * 100)}% spam</Tag>
                    ) : (
                      <Tag tone="quiet">{Math.round(rate * 100)}% spam</Tag>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </Section>

      <Section title="Recent stamps">
        <ul className="space-y-1 text-sm">
          {data.stamps.map((stamp) => (
            <li key={stamp.id} className="flex justify-between border-t border-neutral-100 py-2">
              <span className="font-mono text-neutral-600">{shortAddress(stamp.sender.id)}</span>
              <span className="font-mono">{formatUsdc(BigInt(stamp.amount))}</span>
              <span className="w-20 text-right text-neutral-500">{stamp.status}</span>
            </li>
          ))}
        </ul>
      </Section>

      <p className="mt-10 text-xs text-neutral-500">
        Served by a subgraph indexing PostageEscrow, HumanRegistry and PostageVault on Arc
        testnet. The same data prices every message.
      </p>
    </Shell>
  );
}

function Stat({ label, value, note }: { label: string; value: string; note: string }) {
  return (
    <div className="rounded-xl border border-neutral-200 bg-white p-4">
      <p className="text-xs uppercase tracking-wide text-neutral-500">{label}</p>
      <p className="mt-1 text-xl font-semibold tabular-nums">{value}</p>
      <p className="mt-0.5 text-xs text-neutral-500">{note}</p>
    </div>
  );
}

function Section({
  title,
  hint,
  children,
}: {
  title: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="mt-10">
      <h2 className="text-sm font-medium">{title}</h2>
      {hint && <p className="mt-0.5 text-sm text-neutral-500">{hint}</p>}
      <div className="mt-3 rounded-xl border border-neutral-200 bg-white p-4">{children}</div>
    </section>
  );
}

function Tag({ tone, children }: { tone: "good" | "bad" | "quiet"; children: React.ReactNode }) {
  const tones = {
    good: "bg-green-50 text-green-700",
    bad: "bg-red-50 text-red-700",
    quiet: "bg-neutral-100 text-neutral-600",
  };
  return <span className={`rounded-full px-2 py-0.5 text-xs ${tones[tone]}`}>{children}</span>;
}

function Shell({ children }: { children: React.ReactNode }) {
  return <main className="mx-auto w-full max-w-3xl px-6 py-12">{children}</main>;
}

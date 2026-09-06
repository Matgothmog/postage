import { queryPostage } from "@/lib/graph";
import { formatUsdc, shortAddress } from "@/lib/format";

export const dynamic = "force-dynamic";

const OVERVIEW = `
  query Overview {
    vaults(first: 1) { totalFunded toSponsorship refilledToRelayer fundingEvents }
    enclaves(first: 5) { id measurement revoked registeredAt }
    humanAttestations(first: 100) { id }
    inboxes(first: 10, orderBy: earned, orderDirection: desc) {
      id floorPrice receivedCount earned claimed
    }
    senders(first: 25, orderBy: paidCount, orderDirection: desc) {
      id paidCount totalPaid spamReports spamRate humanUntil
    }
    payments(first: 15, orderBy: paidAt, orderDirection: desc) {
      id tier amount reportedAsSpam sender { id }
    }
  }
`;

interface Overview {
  vaults: { totalFunded: string; toSponsorship: string; refilledToRelayer: string; fundingEvents: number }[];
  enclaves: { id: string; measurement: string; revoked: boolean; registeredAt: string }[];
  humanAttestations: { id: string }[];
  inboxes: { id: string; floorPrice: string; receivedCount: number; earned: string; claimed: string }[];
  senders: {
    id: string;
    paidCount: number;
    totalPaid: string;
    spamReports: number;
    spamRate: string;
    humanUntil: string | null;
  }[];
  payments: { id: string; tier: string; amount: string; reportedAsSpam: boolean; sender: { id: string } }[];
}

/// Gas one attestation costs on Arc at 25 gwei, measured. Used to state the
/// sponsorship pool in the unit that means something: people onboarded.
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
  const sponsors = vault ? BigInt(vault.toSponsorship) / ATTESTATION_COST : 0n;
  const earned = data.inboxes.reduce((total, inbox) => total + BigInt(inbox.earned), 0n);

  return (
    <Shell>
      <header>
        <h1 className="text-xl font-semibold tracking-tight">The network</h1>
        <p className="mt-1 text-sm text-neutral-600">
          Every price paid, who paid it, and which code decided it. Read from the subgraph
          indexing Arc.
        </p>
      </header>

      <div className="mt-6 grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Stat label="People verified" value={String(data.humanAttestations.length)} note="send free" />
        <Stat label="Earned by inboxes" value={formatUsdc(earned)} note="paid by senders" />
        <Stat
          label="Funds verification"
          value={vault ? formatUsdc(BigInt(vault.toSponsorship)) : "0"}
          note={`pays for ~${sponsors} more`}
        />
        <Stat
          label="Priced by"
          value={data.enclaves.filter((e) => !e.revoked).length ? "attested code" : "none"}
          note={data.enclaves.length ? shortAddress(data.enclaves[0].id) : "no signer yet"}
        />
      </div>

      {data.enclaves.length > 0 && (
        <Section
          title="Who is allowed to set a price"
          hint="The escrow rejects any price not signed by one of these keys."
        >
          <ul className="space-y-2 text-sm">
            {data.enclaves.map((enclave) => (
              <li key={enclave.id} className="flex items-baseline justify-between gap-3">
                <span className="font-mono">{shortAddress(enclave.id)}</span>
                <span className="truncate font-mono text-xs text-neutral-500">
                  {enclave.measurement.slice(0, 18)}…
                </span>
                <span className={enclave.revoked ? "text-neutral-400" : "text-green-700"}>
                  {enclave.revoked ? "revoked" : "active"}
                </span>
              </li>
            ))}
          </ul>
        </Section>
      )}

      <Section title="Senders" hint="What a sender pays next time depends on this.">
        {data.senders.length === 0 ? (
          <p className="text-sm text-neutral-500">Nobody has paid yet.</p>
        ) : (
          <table className="w-full text-sm">
            <thead className="text-left text-xs uppercase tracking-wide text-neutral-500">
              <tr>
                <th className="pb-2 font-medium">Wallet</th>
                <th className="pb-2 font-medium">Paid</th>
                <th className="pb-2 font-medium">Reported</th>
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
                    <td className="py-2">{sender.paidCount}</td>
                    <td className="py-2">{sender.spamReports}</td>
                    <td className="py-2 text-right">
                      {human ? (
                        <Tag tone="good">verified person</Tag>
                      ) : rate >= 0.5 ? (
                        <Tag tone="bad">{Math.round(rate * 100)}% reported</Tag>
                      ) : (
                        <Tag tone="quiet">{Math.round(rate * 100)}% reported</Tag>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </Section>

      <Section title="Recent mail that paid">
        {data.payments.length === 0 ? (
          <p className="text-sm text-neutral-500">Nothing yet.</p>
        ) : (
          <ul className="space-y-1 text-sm">
            {data.payments.map((payment) => (
              <li
                key={payment.id}
                className="flex justify-between border-t border-neutral-100 py-2"
              >
                <span className="font-mono text-neutral-600">
                  {shortAddress(payment.sender.id)}
                </span>
                <span className="text-neutral-500">{payment.tier.toLowerCase()}</span>
                <span className="font-mono">{formatUsdc(BigInt(payment.amount))}</span>
              </li>
            ))}
          </ul>
        )}
      </Section>

      <p className="mt-10 text-xs text-neutral-500">
        Verified people and anything urgent are delivered free and never appear here, because
        nothing is charged and nothing is written to a chain.
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

function Section({ title, hint, children }: { title: string; hint?: string; children: React.ReactNode }) {
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

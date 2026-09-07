import Link from "next/link";
import { SiteFooter, SiteHeader, quietButton } from "@/components/chrome";
import {
  ENCLAVE_REGISTRY,
  HUMAN_REGISTRY,
  POSTAGE_ESCROW,
  POSTAGE_VAULT,
} from "@/lib/contracts";
import { formatUsdc, shortAddress } from "@/lib/format";
import { queryPostage } from "@/lib/graph";
import { AutoRefresh } from "./AutoRefresh";

export const dynamic = "force-dynamic";

const EXPLORER = "https://testnet.arcscan.app";

const OVERVIEW = `
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

interface Overview {
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
        <div className="mx-auto w-full max-w-5xl px-6 py-20">
          <h1 className="text-2xl font-semibold tracking-[-0.02em] text-ink">The network</h1>
          <p className="mt-4 rounded-2xl border border-stamp/30 bg-stamp-soft p-5 text-sm text-stamp">
            Could not reach the subgraph. {failure}
          </p>
        </div>
      </Shell>
    );
  }

  const vault = data.vaults[0];
  const sponsored = vault ? BigInt(vault.toSponsorship) / ATTESTATION_COST : 0n;
  const earned = data.inboxes.reduce((total, inbox) => total + BigInt(inbox.earned), 0n);
  const delivered = data.inboxes.reduce((total, inbox) => total + inbox.receivedCount, 0);
  const signer = data.enclaves.find((enclave) => !enclave.revoked);

  return (
    <Shell>
      <main className="mx-auto w-full max-w-5xl px-6 py-14">
        <header className="flex flex-wrap items-end justify-between gap-4">
          <div>
            <p className="text-[11px] font-semibold uppercase tracking-[0.2em] text-stamp">
              Public ledger
            </p>
            <h1 className="mt-4 text-[2.5rem] leading-[1.02] font-semibold tracking-[-0.035em] text-ink">
              Every cent anyone paid
              <br />
              to reach someone.
            </h1>
            <p className="mt-4 max-w-xl text-[15px] leading-relaxed text-ink-soft">
              Read from our subgraph indexing Arc. Nobody signs in to see this — the price a sender
              paid, the code that was allowed to set it, and who was reported afterwards are all on
              chain, so the pricing can be checked rather than believed.
            </p>
          </div>
          <AutoRefresh seconds={20} />
        </header>

        <div className="mt-12 grid grid-cols-2 gap-px overflow-hidden rounded-2xl border border-rule bg-rule lg:grid-cols-4">
          <Stat label="Paid to inboxes" value={formatUsdc(earned)} note="earned by recipients" />
          <Stat label="Messages charged for" value={String(delivered)} note="held, then paid" />
          <Stat
            label="People verified"
            value={String(data.humanAttestations.length)}
            note="sent free, no wallet"
          />
          <Stat
            label="Verification funded"
            value={vault ? formatUsdc(BigInt(vault.toSponsorship)) : "0"}
            note={`pays for ~${sponsored} more`}
          />
        </div>

        <Section
          title="As it settles"
          hint="Recent payments, newest first. Free mail is not here, because nothing is charged and nothing is written to a chain."
        >
          {data.payments.length === 0 ? (
            <Empty>Nobody has paid yet.</Empty>
          ) : (
            <ul className="divide-y divide-rule">
              {data.payments.map((payment) => (
                <li key={payment.id} className="flex items-center gap-4 px-5 py-3.5 text-sm">
                  <Tier tier={payment.tier} />
                  <a
                    href={`${EXPLORER}/address/${payment.sender.id}`}
                    className="font-mono text-ink-soft hover:text-ink"
                  >
                    {shortAddress(payment.sender.id)}
                  </a>
                  <span className="hidden text-ink-faint sm:inline">paid</span>
                  <a
                    href={`${EXPLORER}/address/${payment.inbox.id}`}
                    className="hidden font-mono text-ink-soft hover:text-ink sm:inline"
                  >
                    {shortAddress(payment.inbox.id)}
                  </a>
                  <span className="ml-auto shrink-0 text-xs text-ink-faint tabular-nums">
                    {since(Number(payment.paidAt))}
                  </span>
                  <a
                    href={`${EXPLORER}/tx/${payment.tx}`}
                    className="w-16 shrink-0 text-right font-mono tabular-nums text-ink hover:text-stamp"
                  >
                    {formatUsdc(BigInt(payment.amount))}
                  </a>
                </li>
              ))}
            </ul>
          )}
        </Section>

        <div className="mt-4 grid gap-4 lg:grid-cols-2">
          <Section title="Senders" hint="What one pays next time is decided here.">
            {data.senders.length === 0 ? (
              <Empty>No sender has a history yet.</Empty>
            ) : (
              <table className="w-full text-sm">
                <thead className="text-left text-[11px] uppercase tracking-[0.12em] text-ink-faint">
                  <tr className="border-b border-rule">
                    <th className="px-5 pb-2.5 pt-1 font-medium">Wallet</th>
                    <th className="pb-2.5 pt-1 font-medium">Paid</th>
                    <th className="px-5 pb-2.5 pt-1 text-right font-medium">Standing</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-rule">
                  {data.senders.map((sender) => {
                    const rate = Number(sender.spamRate);
                    return (
                      <tr key={sender.id}>
                        <td className="px-5 py-2.5 font-mono text-ink-soft">
                          {shortAddress(sender.id)}
                        </td>
                        <td className="py-2.5 tabular-nums text-ink">{sender.paidCount}</td>
                        <td className="px-5 py-2.5 text-right">
                          {stillHuman(sender.humanUntil) ? (
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

          <Section title="Inboxes" hint="What each charges, and what it has taken.">
            {data.inboxes.length === 0 ? (
              <Empty>No inbox has been paid yet.</Empty>
            ) : (
              <ul className="divide-y divide-rule">
                {data.inboxes.map((inbox) => (
                  <li key={inbox.id} className="flex items-baseline gap-3 px-5 py-2.5 text-sm">
                    <span className="font-mono text-ink-soft">{shortAddress(inbox.id)}</span>
                    <span className="text-xs text-ink-faint">
                      {formatUsdc(BigInt(inbox.floorPrice))} floor
                    </span>
                    <span className="ml-auto font-mono tabular-nums text-ink">
                      {formatUsdc(BigInt(inbox.earned))}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </Section>
        </div>

        <Section
          title="Spam pays for the free lane"
          hint="A share of every payment goes to the vault, and the vault pays the gas that lets a wallet holding nothing prove it is a person."
        >
          <div className="grid gap-px bg-rule sm:grid-cols-3">
            <Flow label="Into the vault" value={vault ? formatUsdc(BigInt(vault.totalFunded)) : "0"}>
              from {vault?.fundingEvents ?? 0} payments
            </Flow>
            <Flow
              label="Set aside to sponsor"
              value={vault ? formatUsdc(BigInt(vault.toSponsorship)) : "0"}
            >
              70% of it, spendable only on gas
            </Flow>
            <Flow
              label="Already spent on gas"
              value={vault ? formatUsdc(BigInt(vault.refilledToRelayer)) : "0"}
            >
              verifications nobody was charged for
            </Flow>
          </div>
        </Section>

        <Section
          title="Who is allowed to set a price"
          hint="The escrow recovers the signer of every quote and reverts on anyone not listed here, so a price cannot exist unless code with a published identity produced it."
        >
          {data.enclaves.length === 0 ? (
            <Empty>No signing key is registered.</Empty>
          ) : (
            <ul className="divide-y divide-rule">
              {data.enclaves.map((enclave) => (
                <li key={enclave.id} className="px-5 py-3.5">
                  <div className="flex items-center gap-3 text-sm">
                    <a
                      href={`${EXPLORER}/address/${enclave.id}`}
                      className="font-mono text-ink hover:text-stamp"
                    >
                      {shortAddress(enclave.id)}
                    </a>
                    {enclave.revoked ? (
                      <Tag tone="quiet">revoked</Tag>
                    ) : (
                      <Tag tone="good">active</Tag>
                    )}
                  </div>
                  <p className="mt-1.5 truncate font-mono text-xs text-ink-faint">
                    measurement {enclave.measurement.slice(0, 26)}…
                  </p>
                </li>
              ))}
            </ul>
          )}
          <p className="border-t border-rule px-5 py-3.5 text-xs leading-relaxed text-ink-faint">
            {signer
              ? "The measurement recorded against the live key says it belongs to an ordinary server process rather than an attested enclave. Claiming otherwise would be false, so it names itself."
              : "Nothing can be priced until a key is registered."}
          </p>
        </Section>

        <Section title="The contracts" hint="Read them yourself.">
          <ul className="divide-y divide-rule text-sm">
            {[
              ["PostageEscrow", POSTAGE_ESCROW, "takes payment against a signed quote"],
              ["HumanRegistry", HUMAN_REGISTRY, "records that someone proved they are a person"],
              ["PostageVault", POSTAGE_VAULT, "turns paid mail into free verification"],
              ["EnclaveRegistry", ENCLAVE_REGISTRY, "which keys may set a price"],
            ].map(([name, address, role]) => (
              <li key={address} className="flex flex-wrap items-baseline gap-x-3 gap-y-1 px-5 py-3">
                <span className="w-36 shrink-0 text-ink">{name}</span>
                <a
                  href={`${EXPLORER}/address/${address}`}
                  className="font-mono text-xs text-ink-soft hover:text-stamp"
                >
                  {address}
                </a>
                <span className="text-xs text-ink-faint">{role}</span>
              </li>
            ))}
          </ul>
        </Section>
      </main>
    </Shell>
  );
}

/// A credential that has run out says nothing about who is sending now, so it
/// stops counting the moment it lapses.
function stillHuman(humanUntil: string | null): boolean {
  return humanUntil !== null && Number(humanUntil) > Math.floor(Date.now() / 1000);
}

/// Whole units only. A feed that says "14 minutes ago" beside "3 hours ago"
/// reads at a glance; one that says "14 minutes 6 seconds" does not.
function since(seconds: number): string {
  const elapsed = Math.max(0, Math.floor(Date.now() / 1000) - seconds);
  if (elapsed < 60) return "just now";
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)}m ago`;
  if (elapsed < 86_400) return `${Math.floor(elapsed / 3600)}h ago`;
  return `${Math.floor(elapsed / 86_400)}d ago`;
}

function Stat({ label, value, note }: { label: string; value: string; note: string }) {
  return (
    <div className="bg-card p-5">
      <p className="text-[11px] uppercase tracking-[0.14em] text-ink-faint">{label}</p>
      <p className="mt-2 font-mono text-2xl tabular-nums text-ink">{value}</p>
      <p className="mt-1 text-xs text-ink-faint">{note}</p>
    </div>
  );
}

function Flow({
  label,
  value,
  children,
}: {
  label: string;
  value: string;
  children: React.ReactNode;
}) {
  return (
    <div className="bg-card p-5">
      <p className="text-[11px] uppercase tracking-[0.14em] text-ink-faint">{label}</p>
      <p className="mt-2 font-mono text-xl tabular-nums text-ink">{value}</p>
      <p className="mt-1 text-xs text-ink-faint">{children}</p>
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
    <section className="mt-12">
      <h2 className="text-[15px] font-medium text-ink">{title}</h2>
      {hint && <p className="mt-1 max-w-2xl text-sm leading-relaxed text-ink-soft">{hint}</p>}
      <div className="mt-4 overflow-hidden rounded-2xl border border-rule bg-card">{children}</div>
    </section>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return <p className="px-5 py-8 text-center text-sm text-ink-faint">{children}</p>;
}

const TIER_TONE: Record<string, string> = {
  human: "bg-ink",
  important: "bg-good",
  commercial: "bg-warn",
  dangerous: "bg-stamp",
};

function Tier({ tier }: { tier: string }) {
  const name = tier.toLowerCase();
  return (
    <span className="flex shrink-0 items-center gap-2">
      <span className={`h-2 w-2 rounded-full ${TIER_TONE[name] ?? "bg-rule-strong"}`} />
      <span className="w-20 text-xs text-ink-faint">{name}</span>
    </span>
  );
}

function Tag({ tone, children }: { tone: "good" | "bad" | "quiet"; children: React.ReactNode }) {
  const tones = {
    good: "bg-good-soft text-good",
    bad: "bg-stamp-soft text-stamp",
    quiet: "bg-paper text-ink-faint",
  };
  return <span className={`rounded-full px-2 py-0.5 text-[11px] ${tones[tone]}`}>{children}</span>;
}

function Shell({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex min-h-full flex-col">
      <SiteHeader
        actions={
          <Link href="/" className={quietButton}>
            Get an address
          </Link>
        }
      />
      <div className="flex-1">{children}</div>
      <SiteFooter />
    </div>
  );
}

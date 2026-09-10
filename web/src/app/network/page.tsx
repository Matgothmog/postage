import Link from "next/link";
import { Callout, Shell, quietButton } from "@/components/chrome";
import {
  ENCLAVE_REGISTRY,
  HUMAN_REGISTRY,
  POSTAGE_ESCROW,
  POSTAGE_VAULT,
} from "@/lib/contracts";
import { causeMessage } from "@/lib/errors";
import { formatUsdc, shortAddress } from "@/lib/format";
import {
  deriveAggregates,
  fetchOverview,
  paymentTotal,
  since,
  stillHuman,
  type Overview,
} from "@/lib/network";
import { AutoRefresh } from "./AutoRefresh";
import { Empty, Flow, Section, Stat, Tag, Tier } from "./components";

export const dynamic = "force-dynamic";

const EXPLORER = "https://testnet.arcscan.app";

/// A payment and a verification are both on-chain proof, so the feed reads
/// as one timeline instead of two lists — merged and re-sorted by when each
/// happened rather than split by type.
type FeedItem =
  | { kind: "payment"; at: number; payment: Overview["payments"][number] }
  | { kind: "verification"; at: number; wallet: string };

function buildFeed(data: Overview): FeedItem[] {
  const payments: FeedItem[] = data.payments.map((payment) => ({
    kind: "payment",
    at: Number(payment.paidAt),
    payment,
  }));
  const verifications: FeedItem[] = data.humanAttestations.map((attestation) => ({
    kind: "verification",
    at: Number(attestation.attestedAt),
    wallet: attestation.id,
  }));
  return [...payments, ...verifications].sort((a, b) => b.at - a.at).slice(0, 25);
}

function GetAddressLink() {
  return (
    <Link href="/" className={quietButton}>
      Get an address
    </Link>
  );
}

export default async function NetworkPage() {
  let data: Overview | null = null;
  let failure: string | null = null;

  try {
    data = await fetchOverview();
  } catch (cause) {
    failure = causeMessage(cause);
  }

  if (!data) {
    return (
      <Shell actions={<GetAddressLink />}>
        <div className="mx-auto w-full max-w-5xl px-6 py-20">
          <h1 className="text-2xl font-semibold tracking-[-0.02em] text-fg">Ledger</h1>
          <div className="mt-4">
            <Callout tone="bad" title="Can't reach the chain.">
              {failure}
            </Callout>
          </div>
        </div>
      </Shell>
    );
  }

  const vault = data.vaults[0];
  const { sponsored, earned, delivered, signer } = deriveAggregates(data);
  const feed = buildFeed(data);

  return (
    <Shell actions={<GetAddressLink />}>
      <main className="mx-auto w-full max-w-5xl px-6 py-14">
        <header className="flex flex-wrap items-end justify-between gap-4">
          <div>
            <p className="text-[11px] font-semibold uppercase tracking-[0.2em] text-accent">
              Ledger
            </p>
            <h1 className="mt-4 text-[2.5rem] leading-[1.02] font-semibold tracking-[-0.035em] text-fg">
              Every cent,
              <br />
              public.
            </h1>
            <p className="mt-4 max-w-xl text-[15px] leading-relaxed text-muted">
              No login. No trust required — check it yourself, on chain.
            </p>
          </div>
          <AutoRefresh seconds={20} />
        </header>

        <div className="mt-12 grid grid-cols-2 gap-px overflow-hidden rounded-2xl border border-line bg-line lg:grid-cols-4">
          <Stat label="Paid out" value={formatUsdc(earned)} note="earned by inboxes" />
          <Stat
            label="Verified free"
            value={String(data.humanAttestations.length)}
            note="World ID, no wallet"
          />
          <Stat label="Held, then paid" value={String(delivered)} note="messages" />
          <Stat
            label="Sponsor pool"
            value={vault ? formatUsdc(BigInt(vault.toSponsorship)) : "$0.00"}
            note={`funds ~${sponsored} more`}
          />
        </div>

        <Section title="Live" hint="Every payment and verification, newest first.">
          {feed.length === 0 ? (
            <Empty>Nothing yet.</Empty>
          ) : (
            <ul className="divide-y divide-line">
              {feed.map((item) =>
                item.kind === "payment" ? (
                  <li
                    key={`payment-${item.payment.id}`}
                    className="flex items-center gap-4 px-5 py-3.5 text-sm"
                  >
                    <Tier tier={item.payment.tier} />
                    <a
                      href={`${EXPLORER}/address/${item.payment.sender.id}`}
                      className="font-mono text-muted hover:text-fg"
                    >
                      {shortAddress(item.payment.sender.id)}
                    </a>
                    <span className="hidden text-faint sm:inline">paid</span>
                    <a
                      href={`${EXPLORER}/address/${item.payment.inbox.id}`}
                      className="hidden font-mono text-muted hover:text-fg sm:inline"
                    >
                      {shortAddress(item.payment.inbox.id)}
                    </a>
                    <span className="ml-auto shrink-0 text-xs text-faint tabular-nums">
                      {since(item.at)}
                    </span>
                    <a
                      href={`${EXPLORER}/tx/${item.payment.tx}`}
                      className="w-16 shrink-0 text-right font-mono tabular-nums text-fg hover:text-accent"
                    >
                      {formatUsdc(paymentTotal(item.payment))}
                    </a>
                  </li>
                ) : (
                  <li
                    key={`verification-${item.wallet}-${item.at}`}
                    className="flex items-center gap-4 px-5 py-3.5 text-sm"
                  >
                    <span className="flex shrink-0 items-center gap-2">
                      <span className="h-2 w-2 rounded-full bg-accent" />
                      <span className="w-20 text-xs text-faint">verified</span>
                    </span>
                    <a
                      href={`${EXPLORER}/address/${item.wallet}`}
                      className="font-mono text-muted hover:text-fg"
                    >
                      {shortAddress(item.wallet)}
                    </a>
                    <span className="hidden text-faint sm:inline">proved human</span>
                    <span className="ml-auto shrink-0 text-xs text-faint tabular-nums">
                      {since(item.at)}
                    </span>
                    <span className="w-16 shrink-0 text-right font-mono tabular-nums text-good">
                      {formatUsdc(0n)}
                    </span>
                  </li>
                ),
              )}
            </ul>
          )}
        </Section>

        <details className="group mt-12 rounded-2xl border border-line bg-surface">
          <summary className="flex cursor-pointer list-none items-center justify-between gap-4 px-5 py-4 text-[15px] font-medium text-fg [&::-webkit-details-marker]:hidden">
            <span>Proof</span>
            <span className="flex items-center gap-2 text-xs font-normal text-faint">
              <span className="hidden sm:inline">Vault, signer, contracts, standings</span>
              <span aria-hidden className="transition-transform group-open:rotate-180">
                ⌄
              </span>
            </span>
          </summary>

          <div className="space-y-10 border-t border-line px-5 py-6">
            <div className="grid gap-6 lg:grid-cols-2">
              <div>
                <h3 className="text-sm font-medium text-fg">Senders</h3>
                <p className="mt-1 text-xs text-muted">What one pays next time is decided here.</p>
                {data.senders.length === 0 ? (
                  <Empty>No sender has a history yet.</Empty>
                ) : (
                  <div className="mt-3 overflow-x-auto rounded-xl border border-line">
                    <table className="w-full text-sm">
                      <thead className="text-left text-[11px] uppercase tracking-[0.12em] text-faint">
                        <tr className="border-b border-line">
                          <th className="px-4 pb-2.5 pt-2 font-medium">Wallet</th>
                          <th className="pb-2.5 pt-2 font-medium">Paid</th>
                          <th className="px-4 pb-2.5 pt-2 text-right font-medium">Standing</th>
                        </tr>
                      </thead>
                      <tbody className="divide-y divide-line">
                        {data.senders.map((sender) => {
                          const rate = Number(sender.spamRate);
                          return (
                            <tr key={sender.id}>
                              <td className="px-4 py-2.5 font-mono text-muted">
                                {shortAddress(sender.id)}
                              </td>
                              <td className="py-2.5 tabular-nums text-fg">{sender.paidCount}</td>
                              <td className="px-4 py-2.5 text-right">
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
                  </div>
                )}
              </div>

              <div>
                <h3 className="text-sm font-medium text-fg">Inboxes</h3>
                <p className="mt-1 text-xs text-muted">What each charges, and what it has taken.</p>
                {data.inboxes.length === 0 ? (
                  <Empty>No inbox has been paid yet.</Empty>
                ) : (
                  <ul className="mt-3 divide-y divide-line rounded-xl border border-line">
                    {data.inboxes.map((inbox) => (
                      <li key={inbox.id} className="flex items-baseline gap-3 px-4 py-2.5 text-sm">
                        <span className="font-mono text-muted">{shortAddress(inbox.id)}</span>
                        <span className="text-xs text-faint">
                          {formatUsdc(BigInt(inbox.floorPrice))} floor
                        </span>
                        <span className="ml-auto font-mono tabular-nums text-fg">
                          {formatUsdc(BigInt(inbox.earned))}
                        </span>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            </div>

            <div>
              <h3 className="text-sm font-medium text-fg">The vault</h3>
              <p className="mt-1 max-w-2xl text-xs text-muted">
                A share of every payment funds free verification for wallets holding nothing.
              </p>
              <div className="mt-3 grid gap-px overflow-hidden rounded-xl bg-line sm:grid-cols-3">
                <Flow
                  label="Into the vault"
                  value={vault ? formatUsdc(BigInt(vault.totalFunded)) : "$0.00"}
                >
                  from {vault?.fundingEvents ?? 0} payments
                </Flow>
                <Flow
                  label="Set aside to sponsor"
                  value={vault ? formatUsdc(BigInt(vault.toSponsorship)) : "$0.00"}
                >
                  70% of it, gas only
                </Flow>
                <Flow
                  label="Spent on gas"
                  value={vault ? formatUsdc(BigInt(vault.refilledToRelayer)) : "$0.00"}
                >
                  verifications nobody paid for
                </Flow>
              </div>
            </div>

            <div>
              <h3 className="text-sm font-medium text-fg">Who can set a price</h3>
              <p className="mt-1 max-w-2xl text-xs text-muted">
                The escrow reverts on any signer not listed here.
              </p>
              {data.enclaves.length === 0 ? (
                <Empty>No signing key is registered.</Empty>
              ) : (
                <ul className="mt-3 divide-y divide-line rounded-xl border border-line">
                  {data.enclaves.map((enclave) => (
                    <li key={enclave.id} className="px-4 py-3">
                      <div className="flex items-center gap-3 text-sm">
                        <a
                          href={`${EXPLORER}/address/${enclave.id}`}
                          className="font-mono text-fg hover:text-accent"
                        >
                          {shortAddress(enclave.id)}
                        </a>
                        {enclave.revoked ? (
                          <Tag tone="quiet">revoked</Tag>
                        ) : (
                          <Tag tone="good">active</Tag>
                        )}
                      </div>
                      <p className="mt-1.5 truncate font-mono text-xs text-faint">
                        measurement {enclave.measurement.slice(0, 26)}…
                      </p>
                    </li>
                  ))}
                </ul>
              )}
              <p className="mt-2 text-xs leading-relaxed text-faint">
                {signer
                  ? "The live key's measurement matches an ordinary server, not an attested enclave. Said plainly, not hidden."
                  : "Nothing can be priced until a key is registered."}
              </p>
            </div>

            <div>
              <h3 className="text-sm font-medium text-fg">The contracts</h3>
              <p className="mt-1 text-xs text-muted">Read them yourself.</p>
              <ul className="mt-3 divide-y divide-line rounded-xl border border-line text-sm">
                {[
                  ["PostageEscrow", POSTAGE_ESCROW, "takes payment against a signed quote"],
                  ["HumanRegistry", HUMAN_REGISTRY, "records that someone proved they are a person"],
                  ["PostageVault", POSTAGE_VAULT, "turns paid mail into free verification"],
                  ["EnclaveRegistry", ENCLAVE_REGISTRY, "which keys may set a price"],
                ].map(([name, address, role]) => (
                  <li key={address} className="flex flex-wrap items-baseline gap-x-3 gap-y-1 px-4 py-3">
                    <span className="w-36 shrink-0 text-fg">{name}</span>
                    <a
                      href={`${EXPLORER}/address/${address}`}
                      className="font-mono text-xs text-muted hover:text-accent"
                    >
                      {address}
                    </a>
                    <span className="text-xs text-faint">{role}</span>
                  </li>
                ))}
              </ul>
            </div>
          </div>
        </details>
      </main>
    </Shell>
  );
}

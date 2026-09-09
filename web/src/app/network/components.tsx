import Link from "next/link";
import { SiteFooter, SiteHeader, quietButton } from "@/components/chrome";

export function Stat({ label, value, note }: { label: string; value: string; note: string }) {
  return (
    <div className="bg-card p-5">
      <p className="text-[11px] uppercase tracking-[0.14em] text-ink-faint">{label}</p>
      <p className="mt-2 font-mono text-2xl tabular-nums text-ink">{value}</p>
      <p className="mt-1 text-xs text-ink-faint">{note}</p>
    </div>
  );
}

export function Flow({
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

export function Section({
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

export function Empty({ children }: { children: React.ReactNode }) {
  return <p className="px-5 py-8 text-center text-sm text-ink-faint">{children}</p>;
}

const TIER_TONE: Record<string, string> = {
  human: "bg-ink",
  important: "bg-good",
  commercial: "bg-warn",
  dangerous: "bg-stamp",
};

export function Tier({ tier }: { tier: string }) {
  const name = tier.toLowerCase();
  return (
    <span className="flex shrink-0 items-center gap-2">
      <span className={`h-2 w-2 rounded-full ${TIER_TONE[name] ?? "bg-rule-strong"}`} />
      <span className="w-20 text-xs text-ink-faint">{name}</span>
    </span>
  );
}

export function Tag({ tone, children }: { tone: "good" | "bad" | "quiet"; children: React.ReactNode }) {
  const tones = {
    good: "bg-good-soft text-good",
    bad: "bg-stamp-soft text-stamp",
    quiet: "bg-paper text-ink-faint",
  };
  return <span className={`rounded-full px-2 py-0.5 text-[11px] ${tones[tone]}`}>{children}</span>;
}

export function Shell({ children }: { children: React.ReactNode }) {
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

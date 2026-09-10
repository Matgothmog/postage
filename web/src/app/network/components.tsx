import type { ReactNode } from "react";

export function Stat({ label, value, note }: { label: string; value: string; note: string }) {
  return (
    <div className="bg-surface p-5">
      <p className="text-[11px] uppercase tracking-[0.14em] text-faint">{label}</p>
      <p className="mt-2 font-mono text-2xl tabular-nums text-fg">{value}</p>
      <p className="mt-1 text-xs text-faint">{note}</p>
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
  children: ReactNode;
}) {
  return (
    <div className="bg-surface p-5">
      <p className="text-[11px] uppercase tracking-[0.14em] text-faint">{label}</p>
      <p className="mt-2 font-mono text-xl tabular-nums text-fg">{value}</p>
      <p className="mt-1 text-xs text-faint">{children}</p>
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
  children: ReactNode;
}) {
  return (
    <section className="mt-12">
      <h2 className="text-[15px] font-medium text-fg">{title}</h2>
      {hint && <p className="mt-1 max-w-2xl text-sm leading-relaxed text-muted">{hint}</p>}
      <div className="mt-4 overflow-hidden rounded-2xl border border-line bg-surface">{children}</div>
    </section>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return <p className="px-5 py-8 text-center text-sm text-faint">{children}</p>;
}

const TIER_TONE: Record<string, string> = {
  human: "bg-fg",
  important: "bg-good",
  commercial: "bg-warn",
  dangerous: "bg-bad",
};

export function Tier({ tier }: { tier: string }) {
  const name = tier.toLowerCase();
  return (
    <span className="flex shrink-0 items-center gap-2">
      <span className={`h-2 w-2 rounded-full ${TIER_TONE[name] ?? "bg-line-strong"}`} />
      <span className="w-20 text-xs text-faint">{name}</span>
    </span>
  );
}

export function Tag({ tone, children }: { tone: "good" | "bad" | "quiet"; children: ReactNode }) {
  const tones = {
    good: "bg-good-soft text-good",
    bad: "bg-bad-soft text-bad",
    quiet: "bg-bg text-faint",
  };
  return <span className={`rounded-full px-2 py-0.5 text-[11px] ${tones[tone]}`}>{children}</span>;
}

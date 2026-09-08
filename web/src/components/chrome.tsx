import Link from "next/link";
import type { ReactNode } from "react";

export const primaryButton =
  "inline-flex items-center justify-center rounded-xl bg-ink px-5 py-3 text-sm font-medium text-paper transition hover:opacity-85 disabled:opacity-40";

export const secondaryButton =
  "inline-flex items-center justify-center rounded-xl border border-rule-strong bg-card px-5 py-3 text-sm font-medium text-ink transition hover:border-ink disabled:opacity-40";

export const quietButton =
  "inline-flex items-center justify-center rounded-lg px-3 py-2 text-sm text-ink-soft transition hover:text-ink";

export const field =
  "w-full rounded-xl border border-rule bg-card px-4 py-3 text-[15px] text-ink outline-none transition placeholder:text-ink-faint focus:border-ink";

function Wordmark({ href = "/" }: { href?: string }) {
  return (
    <Link href={href} className="group inline-flex items-center gap-2.5">
      <span className="grid h-6 w-6 place-items-center rounded-[5px] bg-stamp text-[11px] font-bold text-white">
        P
      </span>
      <span className="text-[13px] font-semibold uppercase tracking-[0.22em] text-ink">Postage</span>
    </Link>
  );
}

export function SiteHeader({ actions }: { actions?: ReactNode }) {
  return (
    <header className="border-b border-rule">
      <div className="mx-auto flex w-full max-w-5xl items-center justify-between px-6 py-5">
        <Wordmark />
        <nav className="flex items-center gap-1 text-sm">{actions}</nav>
      </div>
    </header>
  );
}

export function SiteFooter() {
  return (
    <footer className="mt-24 border-t border-rule">
      <div className="mx-auto flex w-full max-w-5xl flex-col gap-3 px-6 py-8 text-xs text-ink-faint sm:flex-row sm:items-center sm:justify-between">
        <p>Postage — a price on the mail that wastes your time.</p>
        <p className="font-mono">Arc testnet · settled in USDC</p>
      </div>
    </footer>
  );
}

/// The product, drawn as the thing it is named after. Perforation is masked out
/// of the card rather than drawn on it, so the edge is genuinely torn.
export function StampCard({
  handle,
  price,
  caption,
}: {
  handle: string;
  price: string;
  caption: string;
}) {
  return (
    <div className="perforated guilloche w-full max-w-xs bg-card px-7 py-8 shadow-[0_18px_44px_-28px_rgba(0,0,0,0.5)]">
      <div className="flex items-start justify-between">
        <span className="text-[10px] font-semibold uppercase tracking-[0.2em] text-stamp">
          Postage
        </span>
        <span className="rounded border border-stamp px-1.5 py-0.5 font-mono text-[10px] text-stamp">
          {price}
        </span>
      </div>
      <p className="mt-7 font-mono text-[15px] leading-snug break-all text-ink">{handle}</p>
      <p className="mt-1.5 text-xs text-ink-soft">{caption}</p>
      <div className="mt-7 flex gap-1" aria-hidden>
        {Array.from({ length: 18 }).map((_, index) => (
          <span
            key={index}
            className="h-3 w-px bg-rule-strong"
            style={{ height: `${6 + ((index * 7) % 11)}px` }}
          />
        ))}
      </div>
    </div>
  );
}

export function Callout({
  tone,
  title,
  children,
}: {
  tone: "good" | "bad" | "quiet";
  title: string;
  children?: ReactNode;
}) {
  const tones = {
    good: "border-good/30 bg-good-soft text-good",
    bad: "border-stamp/30 bg-stamp-soft text-stamp",
    quiet: "border-rule bg-card text-ink-soft",
  };
  return (
    <div className={`rounded-xl border p-4 text-sm ${tones[tone]}`}>
      <p className="font-medium">{title}</p>
      {children && <div className="mt-1 opacity-90">{children}</div>}
    </div>
  );
}

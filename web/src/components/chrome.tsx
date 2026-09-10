import Link from "next/link";
import type { ReactNode } from "react";

export const primaryButton =
  "inline-flex items-center justify-center rounded-xl bg-accent px-5 py-3 text-sm font-medium text-on-accent transition hover:bg-accent-hi focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-hi focus-visible:ring-offset-2 focus-visible:ring-offset-bg disabled:opacity-40";

export const secondaryButton =
  "inline-flex items-center justify-center rounded-xl border border-line-strong bg-surface-2 px-5 py-3 text-sm font-medium text-fg transition hover:border-accent-hi focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-hi focus-visible:ring-offset-2 focus-visible:ring-offset-bg disabled:opacity-40";

export const quietButton =
  "inline-flex items-center justify-center rounded-lg px-3 py-2 text-sm text-muted transition hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-hi focus-visible:ring-offset-2 focus-visible:ring-offset-bg";

export const field =
  "w-full rounded-xl border border-line-strong bg-surface px-4 py-3 text-[15px] text-fg outline-none transition placeholder:text-faint focus:border-accent focus:ring-2 focus:ring-accent/30";

function Wordmark({ href = "/" }: { href?: string }) {
  return (
    <Link href={href} className="group inline-flex items-center gap-2.5">
      <span className="grid h-6 w-6 place-items-center rounded-[5px] bg-accent text-[11px] font-bold text-on-accent">
        P
      </span>
      <span className="text-[13px] font-semibold uppercase tracking-[0.22em] text-fg">Postage</span>
    </Link>
  );
}

export function SiteHeader({ actions }: { actions?: ReactNode }) {
  return (
    <header className="border-b border-line">
      <div className="mx-auto flex w-full max-w-5xl items-center justify-between px-6 py-5">
        <Wordmark />
        <nav className="flex items-center gap-1 text-sm">{actions}</nav>
      </div>
    </header>
  );
}

export function SiteFooter() {
  return (
    <footer className="mt-24 border-t border-line">
      <div className="mx-auto flex w-full max-w-5xl flex-col gap-3 px-6 py-8 text-xs text-faint sm:flex-row sm:items-center sm:justify-between">
        <p>Postage</p>
        <p className="font-mono">USDC on Arc</p>
      </div>
    </footer>
  );
}

/// Shared page chrome: header, the flex-grown content slot, footer. Extracted
/// from three near-identical local copies (Account.tsx, network's
/// components.tsx, c/[token]/page.tsx) so the layout lives in one place.
/// `actions` is the header nav slot each of those three filled differently
/// (a sign-in control, a static "get an address" link, a static "what is
/// this?" link) - it is optional here because callers own that decision, not
/// this component. Callers that need a padded/max-width content wrapper
/// (only c/[token]/page.tsx's Shell did) supply it themselves as `children`,
/// the same way Account.tsx's and network/page.tsx's own content already do.
export function Shell({ actions, children }: { actions?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex min-h-dvh flex-col">
      <SiteHeader actions={actions} />
      <div className="flex-1">{children}</div>
      <SiteFooter />
    </div>
  );
}

/// The product, drawn as the thing it is named after: a flat surface with
/// `.glow`'s radial accent wash standing in for what used to be physical
/// texture on the retired paper-and-stamp `StampCard`. Same prop shape as
/// that component had, kept for the two call sites (Landing.tsx,
/// InboxPanel.tsx).
export function AddressCard({
  handle,
  price,
  caption,
}: {
  handle: string;
  price: string;
  caption: string;
}) {
  return (
    <div className="glow w-full max-w-xs rounded-2xl border border-line-strong bg-surface px-7 py-8">
      <div className="flex items-start justify-between">
        <span className="text-[10px] font-semibold uppercase tracking-[0.2em] text-accent">
          Postage
        </span>
        <span className="rounded-full border border-accent px-2 py-0.5 font-mono text-[10px] text-accent-hi">
          {price}
        </span>
      </div>
      <p className="mt-7 font-mono text-[15px] leading-snug break-all text-fg">{handle}</p>
      <p className="mt-1.5 text-xs text-muted">{caption}</p>
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
    bad: "border-bad/30 bg-bad-soft text-bad",
    quiet: "border-line bg-surface text-muted",
  };
  return (
    <div className={`rounded-xl border p-4 text-sm ${tones[tone]}`}>
      <p className="font-medium">{title}</p>
      {children && <div className="mt-1 opacity-90">{children}</div>}
    </div>
  );
}

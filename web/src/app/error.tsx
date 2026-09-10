"use client";

import Link from "next/link";
import { useEffect } from "react";
import { Shell, primaryButton, secondaryButton } from "@/components/chrome";

export default function Error({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    console.error(error);
  }, [error]);

  return (
    <Shell>
      <main className="mx-auto flex w-full max-w-5xl flex-col items-center gap-6 px-6 py-24 text-center sm:py-32">
        <p className="text-[11px] font-semibold uppercase tracking-[0.2em] text-bad">Error</p>
        <h1 className="text-[2.25rem] font-semibold tracking-[-0.03em] text-fg sm:text-[2.75rem]">
          Lost in transit.
        </h1>
        <p className="text-sm text-muted">Not your fault. Try again.</p>
        <div className="flex flex-wrap items-center justify-center gap-3">
          <button type="button" onClick={reset} className={primaryButton}>
            Try again
          </button>
          <Link href="/" className={secondaryButton}>
            Go home
          </Link>
        </div>
      </main>
    </Shell>
  );
}

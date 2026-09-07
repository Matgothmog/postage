"use client";

import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";

/// The page is a view of a chain that keeps moving, so it re-reads itself rather
/// than showing whatever was true when it was opened.
export function AutoRefresh({ seconds }: { seconds: number }) {
  const router = useRouter();
  const [left, setLeft] = useState(seconds);

  useEffect(() => {
    const tick = setInterval(() => {
      setLeft((remaining) => {
        if (remaining > 1) return remaining - 1;
        router.refresh();
        return seconds;
      });
    }, 1000);
    return () => clearInterval(tick);
  }, [router, seconds]);

  return (
    <span className="inline-flex items-center gap-2 text-xs text-ink-faint">
      <span className="relative flex h-1.5 w-1.5">
        <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-good opacity-70" />
        <span className="relative inline-flex h-1.5 w-1.5 rounded-full bg-good" />
      </span>
      re-reading in {left}s
    </span>
  );
}

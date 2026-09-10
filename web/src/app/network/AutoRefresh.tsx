"use client";

import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";

/// The page is a view of a chain that keeps moving, so it re-reads itself rather
/// than showing whatever was true when it was opened.
///
/// The countdown is read off a deadline rather than decremented, so what it says
/// and what it does cannot drift apart, and `router.refresh()` happens in a
/// timeout rather than inside a state updater — React may call an updater more
/// than once for one tick, and a refresh in there fires as often as it does.
export function AutoRefresh({ seconds }: { seconds: number }) {
  const router = useRouter();
  const [left, setLeft] = useState(seconds);
  /// Bumped by each refresh, which is what re-arms the effect: refreshing
  /// re-renders the server component above this one and leaves this one mounted,
  /// so nothing else would ever set the next deadline.
  const [round, setRound] = useState(0);

  useEffect(() => {
    const deadline = Date.now() + seconds * 1000;

    const tick = setInterval(() => {
      setLeft(Math.max(0, Math.ceil((deadline - Date.now()) / 1000)));
    }, 250);

    const refresh = setTimeout(() => {
      router.refresh();
      setRound((n) => n + 1);
    }, seconds * 1000);

    return () => {
      clearInterval(tick);
      clearTimeout(refresh);
    };
  }, [router, seconds, round]);

  return (
    <span className="inline-flex items-center gap-2 text-xs text-faint">
      <span className="relative flex h-1.5 w-1.5">
        <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-good opacity-70" />
        <span className="relative inline-flex h-1.5 w-1.5 rounded-full bg-good" />
      </span>
      re-reading in {left}s
    </span>
  );
}

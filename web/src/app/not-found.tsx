import Link from "next/link";
import { Shell, primaryButton } from "@/components/chrome";

export default function NotFound() {
  return (
    <Shell>
      <main className="mx-auto flex w-full max-w-5xl flex-col items-center gap-6 px-6 py-24 text-center sm:py-32">
        <p className="text-[11px] font-semibold uppercase tracking-[0.2em] text-accent">404</p>
        <h1 className="text-[2.25rem] font-semibold tracking-[-0.03em] text-fg sm:text-[2.75rem]">
          Nothing here.
        </h1>
        <p className="text-sm text-muted">Dead link. The rest of your mail is fine.</p>
        <Link href="/" className={primaryButton}>
          Go home
        </Link>
      </main>
    </Shell>
  );
}

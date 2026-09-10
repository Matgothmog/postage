import Link from "next/link";
import type { ReactNode } from "react";
import { AddressCard } from "@/components/chrome";
import { postageAddress } from "@/lib/handle";

/// The whole product, as four outcomes. Every one of them is what actually
/// happens to a piece of mail, so the page makes one claim and then answers the
/// only question a reader has about it.
const WHO_PAYS = [
  { who: "Codes and receipts", outcome: "free, always", tone: "text-good" },
  { who: "Real people", outcome: "free, once proven", tone: "text-good" },
  { who: "Newsletters and bots", outcome: "they pay you", tone: "text-accent-hi" },
  { who: "Phishing", outcome: "never arrives", tone: "text-bad" },
];

/// Server rendered, so the page a stranger opens is readable before any script
/// runs. The one piece that needs a session — the claim form, which signs in
/// and claims on the same click — is handed in rather than pulled in, which is
/// what keeps the rest of this off the client.
export function Landing({ claim }: { claim: ReactNode }) {
  return (
    <main>
      <section className="glow">
        <div className="mx-auto grid w-full max-w-5xl gap-14 px-6 pt-20 pb-24 lg:grid-cols-[1.15fr_auto] lg:items-center lg:gap-20">
          <div className="rise">
            <h1 className="text-[3.25rem] leading-[0.95] font-semibold tracking-[-0.04em] text-fg sm:text-[4.25rem]">
              Make spam pay.
            </h1>
            <p className="mt-6 max-w-xl text-[19px] leading-relaxed text-muted">
              One address. Humans get through free. Machines pay you in USDC.
            </p>

            {claim}

            <div className="mt-5 flex flex-wrap items-center gap-x-5 gap-y-2">
              <p className="text-xs text-faint">Free. No card. Keep your inbox.</p>
              <Link
                href="/network"
                className="text-xs text-accent-hi transition hover:text-accent"
              >
                See the money →
              </Link>
            </div>
          </div>

          <div className="flex justify-center lg:justify-end">
            <AddressCard
              handle={postageAddress("you")}
              price="$0.01"
              caption="What a stranger pays to reach you"
            />
          </div>
        </div>
      </section>

      <section className="border-t border-line">
        <div className="mx-auto w-full max-w-5xl px-6 py-16">
          <h2 className="text-2xl font-semibold tracking-[-0.02em] text-fg">Who pays.</h2>
          <div className="mt-6 divide-y divide-line border-y border-line">
            {WHO_PAYS.map((row) => (
              <p key={row.who} className="flex flex-wrap items-baseline gap-x-2 py-4 text-[15px]">
                <span className="text-fg">{row.who}</span>
                <span className="text-faint" aria-hidden>
                  —
                </span>
                <span className={row.tone}>{row.outcome}</span>
              </p>
            ))}
          </div>
        </div>
      </section>
    </main>
  );
}

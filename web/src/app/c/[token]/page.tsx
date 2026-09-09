import Link from "next/link";
import { notFound } from "next/navigation";
import { SiteFooter, SiteHeader, quietButton } from "@/components/chrome";
import { challengeByToken } from "@/lib/db/challenges";
import { formatUsdc } from "@/lib/format";
import { postageAddress } from "@/lib/handle";
import { parseStoredQuote } from "@/lib/quote-types";
import { ChallengeActions } from "./ChallengeActions";

export default async function ChallengePage({ params, searchParams }: PageProps<"/c/[token]">) {
  const { token } = await params;
  const { as } = await searchParams;

  const challenge = await challengeByToken(token);
  if (!challenge) notFound();

  const dangerous = challenge.tier === "dangerous";
  const held = challenge.held_until !== null;

  if (challenge.resolved_at) {
    return (
      <Shell>
        <h1 className="text-2xl font-semibold tracking-[-0.02em] text-ink">Already answered</h1>
        <p className="mt-3 text-[15px] leading-relaxed text-ink-soft">
          Whatever you sent to{" "}
          <span className="font-mono text-ink">{postageAddress(challenge.handle)}</span> has been
          dealt with. A pass lasts fifteen minutes, so writing again later means answering again.
        </p>
        <Advert />
      </Shell>
    );
  }

  const quote = parseStoredQuote(challenge.quote_json);
  if (!quote) {
    return (
      <Shell>
        <h1 className="text-2xl font-semibold tracking-[-0.02em] text-ink">This link is broken</h1>
        <p className="mt-3 text-[15px] leading-relaxed text-ink-soft">
          We could not read what this challenge was for, so there is nothing safe to show you here.
          Nothing has been decided either way — write to{" "}
          <span className="font-mono text-ink">{postageAddress(challenge.handle)}</span> again and
          you will get a fresh link.
        </p>
        <Advert />
      </Shell>
    );
  }

  return (
    <Shell>
      <p className="text-[11px] font-semibold uppercase tracking-[0.2em] text-stamp">
        {held ? "Held, not lost" : "Not delivered"}
      </p>
      <h1 className="mt-4 text-3xl leading-[1.1] font-semibold tracking-[-0.03em] text-ink">
        {dangerous ? "This will not be delivered." : "Did a person write this?"}
      </h1>
      <p className="mt-3 text-[15px] leading-relaxed text-ink-soft">
        {dangerous ? (
          <>
            It read as an attempt to deceive whoever you wrote to, and that is not something being a
            person excuses.
          </>
        ) : held ? (
          <>
            Your message to{" "}
            <span className="font-mono text-ink">{postageAddress(challenge.handle)}</span> is still
            here, exactly as you sent it. Answer this and we deliver it — you do not write it twice.
          </>
        ) : (
          <>
            Your message to{" "}
            <span className="font-mono text-ink">{postageAddress(challenge.handle)}</span> was
            refused. Answer this and the next one goes straight through.
          </>
        )}
      </p>

      <div className="mt-8 rounded-2xl border border-rule bg-card p-5">
        <p className="text-[11px] font-semibold uppercase tracking-[0.14em] text-ink-faint">
          What the filter decided
        </p>
        <ul className="mt-3 space-y-2">
          {quote.reasons.map((reason) => (
            <li key={reason} className="flex gap-2.5 text-sm text-ink-soft">
              <span className="mt-2 h-1 w-1 shrink-0 rounded-full bg-stamp" />
              {reason}
            </li>
          ))}
        </ul>
      </div>

      {dangerous ? (
        <p className="mt-5 rounded-2xl border border-stamp/30 bg-stamp-soft p-5 text-sm leading-relaxed text-stamp">
          It will not arrive whatever happens next. Paying is a penalty rather than a price, and
          proving you are a person does not clear it. If the verdict is wrong, this page is where to
          say so.
        </p>
      ) : (
        <p className="mt-5 text-sm leading-relaxed text-ink-soft">
          A person goes through for nothing. Otherwise we take you for a machine, and this inbox
          charges <span className="font-mono text-ink">{formatUsdc(BigInt(quote.amount))}</span> to
          let it through — paid to the person you wrote to, not to us.
        </p>
      )}

      <ChallengeActions
        token={token}
        quote={quote}
        dangerous={dangerous}
        handle={challenge.handle}
        lane={as === "bot" ? "paying" : "choosing"}
      />

      <Advert />
    </Shell>
  );
}

/// The pitch goes here and nowhere near a forwarded message. Whoever is reading
/// this is on the wrong side of exactly the problem Postage sells.
function Advert() {
  return (
    <aside className="mt-14 rounded-2xl border border-stamp/25 bg-stamp-soft p-6">
      <p className="text-[11px] font-semibold uppercase tracking-[0.14em] text-stamp">
        Your inbox could be earning
      </p>
      <p className="mt-3 text-[17px] leading-snug font-medium text-ink">
        On the other side of this, someone is being paid.
      </p>
      <p className="mt-2 text-sm leading-relaxed text-ink-soft">
        Hand out a Postage address instead of your own. Real people and anything urgent reach you
        free; everything else pays you for the interruption. Keep the inbox you already have.
      </p>
      <Link
        href="/"
        className="mt-5 inline-flex items-center justify-center rounded-xl bg-stamp px-5 py-3 text-sm font-medium text-white transition hover:opacity-90"
      >
        Create an account and start earning
      </Link>
    </aside>
  );
}

function Shell({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex min-h-full flex-col">
      <SiteHeader
        actions={
          <Link href="/" className={quietButton}>
            What is this?
          </Link>
        }
      />
      <main className="mx-auto w-full max-w-xl flex-1 px-6 py-14">{children}</main>
      <SiteFooter />
    </div>
  );
}

import Link from "next/link";
import { notFound } from "next/navigation";
import { Shell, primaryButton, quietButton } from "@/components/chrome";
import { challengeByToken } from "@/lib/db/challenges";
import { identityMode } from "@/lib/env";
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
      <Shell
        actions={
          <Link href="/" className={quietButton}>
            What is this?
          </Link>
        }
      >
        <main className="mx-auto w-full max-w-xl px-6 py-14">
          <h1 className="text-2xl font-semibold tracking-[-0.02em] text-fg">Done.</h1>
          <p className="mt-3 text-[15px] leading-relaxed text-muted">
            {"That one's dealt with. A pass lasts 15 minutes."}
          </p>
          <Advert />
        </main>
      </Shell>
    );
  }

  const quote = parseStoredQuote(challenge.quote_json);
  if (!quote) {
    return (
      <Shell
        actions={
          <Link href="/" className={quietButton}>
            What is this?
          </Link>
        }
      >
        <main className="mx-auto w-full max-w-xl px-6 py-14">
          <h1 className="text-2xl font-semibold tracking-[-0.02em] text-fg">Dead link.</h1>
          <p className="mt-3 text-[15px] leading-relaxed text-muted">Write again for a fresh one.</p>
          <Advert />
        </main>
      </Shell>
    );
  }

  return (
    <Shell
      actions={
        <Link href="/" className={quietButton}>
          What is this?
        </Link>
      }
    >
      <main className="mx-auto w-full max-w-xl px-6 py-14">
        <p className="text-[11px] font-semibold uppercase tracking-[0.2em] text-accent">
          {held ? "HELD" : "BOUNCED"}
        </p>
        <h1 className="mt-4 text-3xl leading-[1.1] font-semibold tracking-[-0.03em] text-fg">
          {dangerous ? "Blocked. For good." : "Held at the door."}
        </h1>
        <p className="mt-3 text-[15px] leading-relaxed text-muted">
          {dangerous ? (
            "Reads as an attempt to deceive. Money won't fix that."
          ) : held ? (
            <>
              Your mail to{" "}
              <span className="font-mono text-fg">{postageAddress(challenge.handle)}</span> is safe.
              One click sends it.
            </>
          ) : (
            "That one bounced. Clear this and the next goes straight through."
          )}
        </p>

        <div className="mt-8 rounded-2xl border border-line bg-surface p-5">
          <p className="text-[11px] font-semibold uppercase tracking-[0.14em] text-faint">Why</p>
          <ul className="mt-3 space-y-2">
            {quote.reasons.map((reason) => (
              <li key={reason} className="flex gap-2.5 text-sm text-muted">
                <span className="mt-2 h-1 w-1 shrink-0 rounded-full bg-line-strong" />
                {reason}
              </li>
            ))}
          </ul>
        </div>

        {!dangerous && (
          <p className="mt-5 text-sm leading-relaxed text-muted">
            Humans go free. Machines pay{" "}
            <span className="font-mono text-fg">{formatUsdc(BigInt(quote.amount))}</span> — to them,
            not us.
          </p>
        )}

        <ChallengeActions
          token={token}
          quote={quote}
          dangerous={dangerous}
          handle={challenge.handle}
          lane={as === "bot" ? "paying" : as === "human" ? "human" : "choosing"}
          identityMode={identityMode()}
        />

        <Advert />
      </main>
    </Shell>
  );
}

/// The pitch goes here and nowhere near a forwarded message. Whoever is reading
/// this is on the wrong side of exactly the problem Postage sells.
function Advert() {
  return (
    <aside className="mt-14 rounded-2xl border border-accent/25 bg-accent-soft p-6">
      <p className="text-[11px] font-semibold uppercase tracking-[0.14em] text-accent">POSTAGE</p>
      <p className="mt-3 text-[17px] leading-snug font-medium text-fg">
        Someone just got paid for this.
      </p>
      <p className="mt-2 text-sm leading-relaxed text-muted">
        Charge strangers for your inbox. Keep the one you have.
      </p>
      <Link href="/" className={`mt-5 ${primaryButton}`}>
        Get paid too
      </Link>
    </aside>
  );
}

import { notFound } from "next/navigation";
import { challengeByToken } from "@/lib/db";
import { formatUsdc } from "@/lib/format";
import { ChallengeActions } from "./ChallengeActions";

interface StoredQuote {
  messageId: string;
  inbox: string;
  tier: string;
  amount: string;
  expiresAt: number;
  signature: string;
  reasons: string[];
}

export default async function ChallengePage({ params }: PageProps<"/c/[token]">) {
  const { token } = await params;

  const challenge = await challengeByToken(token);
  if (!challenge) notFound();

  const quote = JSON.parse(challenge.quote_json) as StoredQuote;
  const dangerous = challenge.tier === "dangerous";

  if (challenge.resolved_at) {
    return (
      <Shell>
        <h1 className="text-xl font-semibold">Already cleared</h1>
        <p className="mt-2 text-neutral-600">
          You can write to {challenge.handle}@usepostage.com and it will go straight through.
        </p>
      </Shell>
    );
  }

  return (
    <Shell>
      <p className="text-sm text-neutral-500">Your message was held</p>
      <h1 className="mt-1 text-xl font-semibold">
        {challenge.handle}@usepostage.com did not receive it
      </h1>

      <div className="mt-4 rounded-xl border border-neutral-200 bg-white p-4 text-sm">
        <p className="text-neutral-600">Our filter read it and decided:</p>
        <ul className="mt-2 space-y-1">
          {quote.reasons.map((reason) => (
            <li key={reason} className="text-neutral-800">
              {reason}
            </li>
          ))}
        </ul>
      </div>

      {dangerous ? (
        <p className="mt-6 rounded-xl border border-red-200 bg-red-50 p-4 text-sm text-red-800">
          This looked like an attempt to deceive the recipient, so it will not be delivered
          whatever happens next. If that is wrong, proving you are a person is the way to say so.
        </p>
      ) : (
        <p className="mt-6 text-sm text-neutral-600">
          Prove there is a person behind it and it goes through for nothing, permanently. Otherwise
          this inbox charges {formatUsdc(BigInt(quote.amount))} for automated mail.
        </p>
      )}

      <ChallengeActions token={token} quote={quote} dangerous={dangerous} />

      <p className="mt-10 border-t border-neutral-200 pt-6 text-sm text-neutral-500">
        Tired of the same problem? Hand out a Postage address instead of your own and get paid by
        whoever fills it. <a href="/" className="underline">Take a look</a>.
      </p>
    </Shell>
  );
}

function Shell({ children }: { children: React.ReactNode }) {
  return <main className="mx-auto w-full max-w-lg px-6 py-16">{children}</main>;
}

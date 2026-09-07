import Link from "next/link";
import type { ReactNode } from "react";
import { StampCard, secondaryButton } from "@/components/chrome";

const TIERS = [
  {
    tone: "text-good",
    what: "Something you are waiting for",
    detail: "A login code, a receipt, a delivery update.",
    outcome: "Straight through, free. Never held.",
  },
  {
    tone: "text-ink",
    what: "Written by a person",
    detail: "A reply, an introduction, a real message.",
    outcome: "Held until they prove it. Proving it costs nothing.",
  },
  {
    tone: "text-warn",
    what: "Ordinary automated mail",
    detail: "Newsletters, marketing, announcements.",
    outcome: "Held. Nobody proved a person is behind it, so it pays you.",
  },
  {
    tone: "text-stamp",
    what: "Trying to deceive you",
    detail: "Phishing, impersonation, payment redirection.",
    outcome: "Never delivered. Being a person does not clear it.",
  },
];

const STEPS = [
  {
    title: "Claim a handle",
    body: "Sign in with your email and pick a name. That address is yours, and the one you already read is where its mail lands.",
  },
  {
    title: "Hand it out",
    body: "Put it on forms, in signatures, wherever your real address used to go. You do not change email provider or learn a new inbox.",
  },
  {
    title: "Get paid for the rest",
    body: "Strangers are asked one question: did a person write this? People go free. Machines pay you a cent, in USDC, on Arc.",
  },
];

/// Server rendered, so the page a stranger opens is readable before any script
/// runs. The single button that needs a session is handed in rather than pulled
/// in, which is what keeps the rest of this off the client.
export function Landing({ cta }: { cta: ReactNode }) {
  return (
    <main>
      <section className="mx-auto grid w-full max-w-5xl gap-14 px-6 pt-16 pb-20 sm:pt-24 lg:grid-cols-[1.15fr_auto] lg:items-center lg:gap-20">
        <div className="rise">
          <p className="text-[11px] font-semibold uppercase tracking-[0.2em] text-stamp">
            Email, with a toll gate
          </p>
          <h1 className="mt-5 text-[2.75rem] leading-[0.98] font-semibold tracking-[-0.035em] text-ink sm:text-[3.75rem]">
            Your attention
            <br />
            already has a price.
            <br />
            <span className="relative inline-block">
              Collect it.
              <span className="absolute inset-x-0 -bottom-1 h-[3px] bg-stamp" aria-hidden />
            </span>
          </h1>
          <p className="mt-7 max-w-xl text-[17px] leading-relaxed text-ink-soft">
            Give out <span className="font-mono text-ink">you@usepostage.com</span> instead of your
            own address. Everything sent there is read, judged, and forwarded to the inbox you
            already use — untouched. Mail from a person arrives free. Mail from a machine pays you
            first.
          </p>

          <div className="mt-9 flex flex-wrap items-center gap-3">
            {cta}
            <Link href="/network" className={secondaryButton}>
              Watch the network
            </Link>
          </div>
          <p className="mt-4 text-xs text-ink-faint">
            One sign-in, one handle. No wallet to install, nothing to configure, no card.
          </p>
        </div>

        <div className="flex justify-center lg:justify-end">
          <StampCard
            handle="you@usepostage.com"
            price="$0.01"
            caption="What a stranger pays to reach you"
          />
        </div>
      </section>

      <section className="border-y border-rule bg-card">
        <div className="mx-auto w-full max-w-5xl px-6 py-16">
          <h2 className="text-2xl font-semibold tracking-[-0.02em] text-ink">
            Four things arrive. Only one of them is free by accident.
          </h2>
          <p className="mt-2 max-w-2xl text-[15px] text-ink-soft">
            Every stranger is held. The filter does not decide whether to hold you — it decides who
            pays to get through.
          </p>

          <div className="mt-10 grid gap-px overflow-hidden rounded-2xl border border-rule bg-rule sm:grid-cols-2">
            {TIERS.map((tier) => (
              <div key={tier.what} className="bg-card p-6">
                <p className={`text-[15px] font-medium ${tier.tone}`}>{tier.what}</p>
                <p className="mt-1 text-sm text-ink-faint">{tier.detail}</p>
                <p className="mt-4 border-t border-rule pt-4 text-sm text-ink-soft">
                  {tier.outcome}
                </p>
              </div>
            ))}
          </div>
        </div>
      </section>

      <section className="mx-auto w-full max-w-5xl px-6 py-20">
        <h2 className="text-2xl font-semibold tracking-[-0.02em] text-ink">
          Three things happen, and you do one of them.
        </h2>
        <ol className="mt-10 grid gap-10 sm:grid-cols-3">
          {STEPS.map((step, index) => (
            <li key={step.title}>
              <span className="font-mono text-xs text-stamp">0{index + 1}</span>
              <h3 className="mt-3 text-[17px] font-medium text-ink">{step.title}</h3>
              <p className="mt-2 text-sm leading-relaxed text-ink-soft">{step.body}</p>
            </li>
          ))}
        </ol>

        <div className="mt-16 rounded-2xl border border-rule bg-card p-8 sm:p-10">
          <h2 className="max-w-2xl text-xl font-semibold tracking-[-0.02em] text-ink">
            Spam is cheap to send and expensive to receive. Every filter ever built tried to fix
            that by guessing better.
          </h2>
          <p className="mt-3 max-w-2xl text-[15px] leading-relaxed text-ink-soft">
            Postage does something else: it makes the sender carry the cost. A price that cannot be
            invented — every quote is signed by code whose identity is published, and the escrow
            refuses anything else. The money is yours, and you can watch every cent of it settle.
          </p>
          <div className="mt-7 flex flex-wrap gap-3">
            {cta}
            <Link href="/network" className={secondaryButton}>
              See what it has earned
            </Link>
          </div>
        </div>
      </section>
    </main>
  );
}

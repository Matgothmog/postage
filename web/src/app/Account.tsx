"use client";

import Link from "next/link";
import { useIdentityToken, usePrivy, useSignMessage, useWallets } from "@privy-io/react-auth";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import { Callout, SiteFooter, SiteHeader, quietButton } from "@/components/chrome";
import { readStatement } from "@/lib/wallet-proof";
import { walletProof } from "./claim-inbox-helpers";
import { ClaimInbox } from "./ClaimInbox";
import { type Inbox, InboxPanel } from "./InboxPanel";

/// Decides which of the two things this address is: the page a stranger reads,
/// or the inbox its owner manages. The first arrives already rendered and is
/// simply passed through, so nobody waits on a wallet library to read a pitch.
export function Account({ landing }: { landing: ReactNode }) {
  const { ready, authenticated, login, logout, user } = usePrivy();
  const { identityToken } = useIdentityToken();
  const { signMessage } = useSignMessage();
  const { wallets } = useWallets();
  const wallet = wallets[0];

  const [inbox, setInbox] = useState<Inbox | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<InboxErrorKind | null>(null);

  // The row holds the address the user actually reads, so the server will not
  // hand it over on the strength of a wallet address alone - those are public.
  // Privy's identity token is that proof, already signed and already in hand;
  // signing a statement is the same proof made the long way, for a session that
  // has no identity token to offer.
  //
  // A non-ok response is never treated as "no inbox" - that would show someone
  // who already claimed one the claim flow again, inviting them to fight
  // themselves for their own handle. A refused proof needs a sign-in prompt;
  // anything else means we could not ask at all, which needs different words
  // and a retry.
  //
  // Nothing here clears a previous error up front, because a fetch that has not
  // answered yet says nothing about the last one. It is cleared on the answer
  // instead, and it has to be: a retry that succeeds for someone with no inbox
  // sets `inbox` to the null it already held, so leaving the error standing
  // would pin them on the error screen and keep the claim flow out of reach for
  // the rest of the session.
  //
  // A thrown fetch or signature error is left to propagate rather than caught
  // here, so every caller decides for itself what a failure means to it - the
  // mount effect below records it as "network", the same as a bad response.
  const refresh = useCallback(async () => {
    const address = wallet?.address;
    if (!address) return;
    try {
      const response = await fetch("/api/inbox", {
        headers: await walletProof(identityToken, signMessage, address, (at) =>
          readStatement(address, at)
        ),
      });
      if (response.ok) {
        setInbox(((await response.json()) as { inbox: Inbox | null }).inbox);
        setError(null);
      } else {
        setError(refusedTheProof(response.status) ? "unauthorized" : "network");
      }
    } finally {
      setLoaded(true);
    }
  }, [identityToken, wallet?.address, signMessage]);

  useEffect(() => {
    refresh().catch(() => setError("network"));
  }, [refresh]);

  const retry = useCallback(() => {
    refresh().catch(() => setError("network"));
  }, [refresh]);

  if (!ready || !authenticated) {
    return <Shell actions={<SignedOutActions login={login} disabled={!ready} />}>{landing}</Shell>;
  }

  return (
    <Shell
      actions={
        <>
          <Link href="/network" className={quietButton}>
            Network
          </Link>
          <button onClick={logout} className={quietButton}>
            Sign out
          </button>
        </>
      }
    >
      {!wallet ? (
        <Waiting>Setting up your wallet</Waiting>
      ) : inbox ? (
        <InboxPanel inbox={inbox} wallet={wallet.address} />
      ) : loaded && error ? (
        <InboxError kind={error} onRetry={retry} />
      ) : loaded ? (
        <ClaimInbox
          wallet={wallet.address}
          email={user?.email?.address?.toLowerCase() ?? null}
          onLive={refresh}
        />
      ) : (
        <Waiting>Reading your inbox</Waiting>
      )}
    </Shell>
  );
}

function SignedOutActions({ login, disabled }: { login: () => void; disabled: boolean }) {
  return (
    <>
      <Link href="/network" className={quietButton}>
        Network
      </Link>
      <button onClick={login} disabled={disabled} className={quietButton}>
        Sign in
      </button>
    </>
  );
}

function Shell({ actions, children }: { actions: ReactNode; children: ReactNode }) {
  return (
    <div className="flex min-h-full flex-col">
      <SiteHeader actions={actions} />
      <div className="flex-1">{children}</div>
      <SiteFooter />
    </div>
  );
}

function Waiting({ children }: { children: ReactNode }) {
  return <main className="m-auto px-6 py-32 text-center text-sm text-ink-faint">{children}</main>;
}

type InboxErrorKind = "unauthorized" | "network";

/// Whether the route turned down the proof rather than failing to read it.
/// 401 is the signature that did not verify; 400 is the proof that named no
/// wallet at all, which is what a session offering an expired identity token
/// and nothing else sends. Both are permanent until the user signs in again,
/// so neither may be dressed up as a transient fault with a retry that cannot
/// work.
function refusedTheProof(status: number): boolean {
  return status === 400 || status === 401;
}

const INBOX_ERROR_COPY: Record<InboxErrorKind, { title: string; body: string }> = {
  unauthorized: {
    title: "We could not verify that is your wallet",
    body: "Your sign-in did not check out with the server, so we are not guessing and showing you someone else's inbox. Sign out and back in, then try again.",
  },
  network: {
    title: "We could not reach the server",
    body: "This is not about whether you have an inbox - we just could not ask. Try again in a moment.",
  },
};

function InboxError({ kind, onRetry }: { kind: InboxErrorKind; onRetry: () => void }) {
  const { title, body } = INBOX_ERROR_COPY[kind];
  return (
    <section className="rise mx-auto w-full max-w-xl px-6 py-16">
      <Callout tone="bad" title={title}>
        <p>{body}</p>
        <button onClick={onRetry} className={`${quietButton} -ml-3 mt-1 text-stamp`}>
          Try again
        </button>
      </Callout>
    </section>
  );
}

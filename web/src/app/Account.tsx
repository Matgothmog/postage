"use client";

import Link from "next/link";
import { useIdentityToken, usePrivy, useSignMessage, useWallets } from "@privy-io/react-auth";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import { SiteFooter, SiteHeader, quietButton } from "@/components/chrome";
import { readStatement } from "@/lib/statements";
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

  // The row holds the address the user actually reads, so the server will not
  // hand it over on the strength of a wallet address alone - those are public.
  // Privy's identity token is that proof, already signed and already in hand;
  // signing a statement is the same proof made the long way, for a session that
  // has no identity token to offer.
  const refresh = useCallback(async () => {
    const address = wallet?.address;
    if (!address) return;
    try {
      const response = await fetch("/api/inbox", {
        headers: identityToken
          ? { "privy-id-token": identityToken }
          : await signedHeaders(address, signMessage),
      });
      if (response.ok) setInbox(((await response.json()) as { inbox: Inbox | null }).inbox);
    } finally {
      setLoaded(true);
    }
  }, [identityToken, wallet?.address, signMessage]);

  useEffect(() => {
    refresh().catch(() => setLoaded(true));
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

type SignMessage = ReturnType<typeof useSignMessage>["signMessage"];

async function signedHeaders(address: string, signMessage: SignMessage): Promise<HeadersInit> {
  const issuedAt = Math.floor(Date.now() / 1000);
  const { signature } = await signMessage(
    { message: readStatement(address, issuedAt) },
    { address, uiOptions: { showWalletUIs: false } }
  );
  return {
    "x-postage-wallet": address,
    "x-postage-issued": String(issuedAt),
    "x-postage-signature": signature,
  };
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

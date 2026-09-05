"use client";

import { PrivyProvider } from "@privy-io/react-auth";
import type { ReactNode } from "react";
import { chain } from "@/lib/contracts";

export function Providers({ children }: { children: ReactNode }) {
  const appId = process.env.NEXT_PUBLIC_PRIVY_APP_ID;

  if (!appId) {
    return (
      <main className="m-auto max-w-md p-8 text-sm">
        <p className="font-medium">NEXT_PUBLIC_PRIVY_APP_ID is not set.</p>
        <p className="mt-2 text-neutral-600">
          Copy <code>.env.local.example</code> to <code>.env.local</code> and add your Privy app
          id.
        </p>
      </main>
    );
  }

  return (
    <PrivyProvider
      appId={appId}
      config={{
        loginMethods: ["email", "passkey"],
        embeddedWallets: { ethereum: { createOnLogin: "users-without-wallets" } },
        defaultChain: chain,
        supportedChains: [chain],
      }}
    >
      {children}
    </PrivyProvider>
  );
}

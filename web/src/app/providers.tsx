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
        <p className="mt-2 text-muted">
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
        embeddedWallets: {
          ethereum: { createOnLogin: "users-without-wallets" },
          priceDisplay: {
            primary: "native-token",
            secondary: null,
          },
        },
        defaultChain: chain,
        supportedChains: [chain],
        appearance: {
          theme: "#0A0E1A",
          accentColor: "#3B82F6",
          logo: (
            <svg
              viewBox="0 0 32 32"
              width="40"
              height="40"
              xmlns="http://www.w3.org/2000/svg"
              fill="none"
            >
              <circle cx="16" cy="16" r="14" stroke="#3B82F6" strokeWidth="1.5" />
              <text
                x="16"
                y="20"
                fontSize="18"
                fontWeight="bold"
                fill="#3B82F6"
                textAnchor="middle"
              >
                P
              </text>
            </svg>
          ),
          landingHeader: "Claim your address",
          loginMessage: "One address. Spam pays you.",
          emailDomain: "usepostage.com",
          showWalletLoginFirst: false,
        },
      }}
    >
      {children}
    </PrivyProvider>
  );
}

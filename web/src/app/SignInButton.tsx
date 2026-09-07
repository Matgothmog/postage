"use client";

import { usePrivy } from "@privy-io/react-auth";
import { primaryButton } from "@/components/chrome";

/// The only part of the landing page that needs a session, kept to itself so
/// the rest of it renders on the server.
export function SignInButton({ label = "Claim your address" }: { label?: string }) {
  const { ready, login } = usePrivy();
  return (
    <button onClick={login} disabled={!ready} className={primaryButton}>
      {label}
    </button>
  );
}

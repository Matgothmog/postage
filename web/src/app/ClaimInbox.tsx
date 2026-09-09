"use client";

import { useState } from "react";
import type { ClaimProgress } from "./claim-inbox-helpers";
import { FinishClaim } from "./FinishClaim";
import { PickHandle } from "./PickHandle";

export type { ClaimProgress };

export function ClaimInbox({
  wallet,
  email,
  onLive,
}: {
  wallet: string;
  email: string | null;
  onLive: () => void;
}) {
  const [claim, setClaim] = useState<ClaimProgress | null>(null);

  if (!claim) {
    return <PickHandle wallet={wallet} email={email} onStarted={setClaim} onLive={onLive} />;
  }
  return (
    <FinishClaim
      claim={claim}
      wallet={wallet}
      onClaim={setClaim}
      onLive={onLive}
      onRestart={() => setClaim(null)}
    />
  );
}

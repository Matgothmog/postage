import assert from "node:assert/strict";
import { test } from "node:test";

import {
  ATTESTATION_COST,
  deriveAggregates,
  paymentTotal,
  since,
  stillHuman,
  type Overview,
} from "./network";
import { now } from "./time";

/// Every field defaults to "nothing on chain" so a test only has to name the
/// slice it cares about, and a reader only has to look at those overrides.
function overview(overrides: Partial<Overview> = {}): Overview {
  return {
    vaults: [],
    enclaves: [],
    humanAttestations: [],
    inboxes: [],
    senders: [],
    payments: [],
    ...overrides,
  };
}

test("deriveAggregates sums what recipients kept, not what was charged", () => {
  const { earned } = deriveAggregates(
    overview({
      inboxes: [
        { id: "0xa", floorPrice: "0", receivedCount: 1, earned: "800", claimed: "0" },
        { id: "0xb", floorPrice: "0", receivedCount: 2, earned: "1200", claimed: "0" },
      ],
    })
  );

  assert.equal(earned, 2000n);
});

test("deriveAggregates counts every message an inbox received", () => {
  const { delivered } = deriveAggregates(
    overview({
      inboxes: [
        { id: "0xa", floorPrice: "0", receivedCount: 3, earned: "0", claimed: "0" },
        { id: "0xb", floorPrice: "0", receivedCount: 5, earned: "0", claimed: "0" },
      ],
    })
  );

  assert.equal(delivered, 8);
});

test("deriveAggregates prices the sponsorship pool in whole attestations, rounded down", () => {
  const { sponsored } = deriveAggregates(
    overview({
      vaults: [
        {
          totalFunded: "0",
          toTreasury: "0",
          toSponsorship: String(ATTESTATION_COST * 3n + 1n),
          refilledToRelayer: "0",
          fundingEvents: 0,
        },
      ],
    })
  );

  assert.equal(sponsored, 3n);
});

test("deriveAggregates prices sponsorship as zero when no vault has been indexed yet", () => {
  const { sponsored } = deriveAggregates(overview());

  assert.equal(sponsored, 0n);
});

test("deriveAggregates picks the first enclave that has not been revoked", () => {
  const { signer } = deriveAggregates(
    overview({
      enclaves: [
        { id: "0xrevoked", measurement: "0x00", revoked: true, registeredAt: "0" },
        { id: "0xlive", measurement: "0x01", revoked: false, registeredAt: "1" },
      ],
    })
  );

  assert.equal(signer?.id, "0xlive");
});

test("deriveAggregates has no signer when every registered key is revoked", () => {
  const { signer } = deriveAggregates(
    overview({
      enclaves: [{ id: "0xrevoked", measurement: "0x00", revoked: true, registeredAt: "0" }],
    })
  );

  assert.equal(signer, undefined);
});

/// The bug this split exists to fix: PostageEscrow emits `msg.value - toVault`
/// as Payment.amount, so amount alone understates what the sender's wallet
/// actually left by the vault's cut. Pinned with an amount and a toVault that
/// are both known and different, so a regression back to `amount` alone would
/// fail this rather than pass by coincidence.
test("paymentTotal reconstructs the full amount the sender paid, not the inbox's net share", () => {
  const total = paymentTotal({ amount: "800000000000000000", toVault: "200000000000000000" });

  assert.equal(total, 1_000_000_000_000_000_000n);
});

test("stillHuman is false once a credential has lapsed", () => {
  assert.equal(stillHuman(null), false);
  assert.equal(stillHuman(String(now() - 1)), false);
});

test("stillHuman is true while a credential has time left", () => {
  assert.equal(stillHuman(String(now() + 60)), true);
});

test("since reads recent payments as just now", () => {
  assert.equal(since(now()), "just now");
  assert.equal(since(now() - 59), "just now");
});

test("since rounds down to whole minutes, then hours, then days", () => {
  assert.equal(since(now() - 120), "2m ago");
  assert.equal(since(now() - 2 * 3600), "2h ago");
  assert.equal(since(now() - 2 * 86_400), "2d ago");
});

test("since clamps a timestamp from the future to zero elapsed rather than going negative", () => {
  assert.equal(since(now() + 3600), "just now");
});

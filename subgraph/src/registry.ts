import { HumanAttested } from "../generated/HumanRegistry/HumanRegistry";
import { HumanAttestation } from "../generated/schema";
import { loadSender } from "./shared";

export function handleHumanAttested(event: HumanAttested): void {
  let attestation = HumanAttestation.load(event.params.wallet);
  if (attestation == null) {
    attestation = new HumanAttestation(event.params.wallet);
    attestation.wallet = event.params.wallet;
    attestation.renewals = 0;
  } else {
    attestation.renewals += 1;
  }

  attestation.nullifierHash = event.params.nullifierHash;
  attestation.expiresAt = event.params.expiresAt;
  attestation.attestedAt = event.block.timestamp;
  attestation.save();

  // Mirrored onto the sender so a reputation lookup is one query, not two.
  const sender = loadSender(event.params.wallet, event.block.timestamp);
  sender.humanUntil = event.params.expiresAt;
  sender.save();
}

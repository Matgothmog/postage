import { EnclaveRegistered, EnclaveRevoked } from "../generated/EnclaveRegistry/EnclaveRegistry";
import { Enclave } from "../generated/schema";

export function handleEnclaveRegistered(event: EnclaveRegistered): void {
  const enclave = new Enclave(event.params.signer);
  enclave.measurement = event.params.measurement;
  enclave.registeredAt = event.block.timestamp;
  enclave.revoked = false;
  enclave.save();
}

export function handleEnclaveRevoked(event: EnclaveRevoked): void {
  const enclave = Enclave.load(event.params.signer);
  if (enclave == null) return;
  enclave.revoked = true;
  enclave.save();
}

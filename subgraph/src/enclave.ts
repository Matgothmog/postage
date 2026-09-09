import { Bytes } from "@graphprotocol/graph-ts";
import {
  EnclaveRegistered,
  EnclaveRevoked,
  MeasurementSet,
} from "../generated/EnclaveRegistry/EnclaveRegistry";
import { Enclave, ExpectedMeasurement } from "../generated/schema";

const EXPECTED_MEASUREMENT_ID = "current";

/// Anyone can recompute this from a clean checkout and compare — same claim
/// EnclaveRegistry.setMeasurement makes onchain.
export function handleMeasurementSet(event: MeasurementSet): void {
  const id = Bytes.fromUTF8(EXPECTED_MEASUREMENT_ID);
  let expected = ExpectedMeasurement.load(id);
  if (expected == null) expected = new ExpectedMeasurement(id);

  expected.measurement = event.params.measurement;
  expected.setAt = event.block.timestamp;
  expected.save();
}

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

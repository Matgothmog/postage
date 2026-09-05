import { BigInt, Bytes } from "@graphprotocol/graph-ts";
import {
  Funded,
  RelayerRefilled,
  TreasuryWithdrawn,
} from "../generated/PostageVault/PostageVault";
import { Vault } from "../generated/schema";

const VAULT_ID = "vault";

function loadVault(): Vault {
  const id = Bytes.fromUTF8(VAULT_ID);
  let vault = Vault.load(id);
  if (vault != null) return vault;

  vault = new Vault(id);
  vault.totalFunded = BigInt.zero();
  vault.toTreasury = BigInt.zero();
  vault.toSponsorship = BigInt.zero();
  vault.refilledToRelayer = BigInt.zero();
  vault.withdrawnByTreasury = BigInt.zero();
  vault.fundingEvents = 0;
  return vault;
}

export function handleFunded(event: Funded): void {
  const vault = loadVault();
  vault.totalFunded = vault.totalFunded.plus(event.params.amount);
  vault.toTreasury = vault.toTreasury.plus(event.params.toTreasury);
  vault.toSponsorship = vault.toSponsorship.plus(event.params.toSponsorship);
  vault.fundingEvents += 1;
  vault.save();
}

export function handleRelayerRefilled(event: RelayerRefilled): void {
  const vault = loadVault();
  vault.refilledToRelayer = vault.refilledToRelayer.plus(event.params.amount);
  vault.save();
}

export function handleTreasuryWithdrawn(event: TreasuryWithdrawn): void {
  const vault = loadVault();
  vault.withdrawnByTreasury = vault.withdrawnByTreasury.plus(event.params.amount);
  vault.save();
}

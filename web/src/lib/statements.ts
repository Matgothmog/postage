/// What a wallet signs to prove a request is really from it. Both ends build the
/// same string, so this pulls in nothing but the domain constant.
import { postageAddress } from "./handle";

export function claimStatement(handle: string, destination: string, wallet: string, issuedAt: number): string {
  return [
    "Postage: claim an address",
    `Handle: ${postageAddress(handle)}`,
    `Forward to: ${destination.toLowerCase()}`,
    `Wallet: ${wallet.toLowerCase()}`,
    `Issued: ${issuedAt}`,
  ].join("\n");
}

export function readStatement(wallet: string, issuedAt: number): string {
  return ["Postage: read my inbox", `Wallet: ${wallet.toLowerCase()}`, `Issued: ${issuedAt}`].join("\n");
}

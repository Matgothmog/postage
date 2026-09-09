/// What a wallet signs to prove a request is really from it. Both ends build the
/// same string, so this pulls in nothing but the domain constant.
import { MAIL_DOMAIN, postageAddress } from "./handle";

export function claimStatement(handle: string, destination: string, wallet: string, issuedAt: number): string {
  return [
    "Postage: claim an address",
    `Handle: ${postageAddress(handle)}`,
    `Forward to: ${destination.toLowerCase()}`,
    `Wallet: ${wallet.toLowerCase()}`,
    `Issued: ${issuedAt}`,
  ].join("\n");
}

/// Names the deployment as well as the wallet. The other two carry it already,
/// inside the `postageAddress(handle)` they each state; this one has no handle
/// to carry it, and named only a wallet and a time until now — so a signature
/// over that text, collected on a staging copy or by any other site that asked
/// the same wallet for one, opened this inbox too for as long as the timestamp
/// stayed fresh. Reading an inbox discloses the address the handle forwards to.
export function readStatement(wallet: string, issuedAt: number): string {
  return [
    "Postage: read my inbox",
    `Domain: ${MAIL_DOMAIN}`,
    `Wallet: ${wallet.toLowerCase()}`,
    `Issued: ${issuedAt}`,
  ].join("\n");
}

/// Names the handle as well as the wallet, so a signature collected for one
/// claim cannot confirm another.
export function confirmStatement(handle: string, wallet: string, issuedAt: number): string {
  return [
    "Postage: confirm my code",
    `Handle: ${postageAddress(handle)}`,
    `Wallet: ${wallet.toLowerCase()}`,
    `Issued: ${issuedAt}`,
  ].join("\n");
}

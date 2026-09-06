/// What a wallet signs to prove a request is really from it. Kept free of
/// imports so the browser can build the same string the server will verify.

export function claimStatement(handle: string, destination: string, wallet: string, issuedAt: number): string {
  return [
    "Postage: claim an address",
    `Handle: ${handle.toLowerCase()}@usepostage.com`,
    `Forward to: ${destination.toLowerCase()}`,
    `Wallet: ${wallet.toLowerCase()}`,
    `Issued: ${issuedAt}`,
  ].join("\n");
}

export function readStatement(wallet: string, issuedAt: number): string {
  return ["Postage: read my inbox", `Wallet: ${wallet.toLowerCase()}`, `Issued: ${issuedAt}`].join("\n");
}

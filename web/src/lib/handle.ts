/// The domain the gateway receives mail for, and the two things anyone needs to
/// do with it. Written out in five files before this, which is how a route came
/// to forward to an address on our own domain without noticing.
export const MAIL_DOMAIN = "usepostage.com";

export function postageAddress(handle: string): string {
  return `${handle.toLowerCase()}@${MAIL_DOMAIN}`;
}

/// The handle an address points at, or "" if it names no local part.
export function handleOf(address: string): string {
  return address.split("@")[0]?.toLowerCase() ?? "";
}

/// Whether this address is one of ours. A destination that is means mail
/// forwarded to it comes straight back, and every lap spends a classify call, a
/// chain read and a challenge row.
export function isOurs(address: string): boolean {
  return address.toLowerCase().endsWith(`@${MAIL_DOMAIN}`);
}

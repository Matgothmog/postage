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

/// The rules a handle must satisfy, stated once. The signup route enforces
/// them server-side; the claim form's live input filter and its
/// suggested-handle normaliser both need the same charset and length
/// client-side, and each used to restate them by hand.
export const HANDLE_MIN_LENGTH = 2;
export const HANDLE_MAX_LENGTH = 31;

/// The shape a handle must have once lowercased: starts and ends on a letter
/// or digit, with only letters, digits, dot, underscore and dash allowed
/// between. Length and the "no two dots running together" rule are checked
/// separately, since the server reports each as its own error message.
export const HANDLE_SHAPE = /^[a-z0-9]([a-z0-9._-]*[a-z0-9])?$/;

export function hasRepeatedDot(handle: string): boolean {
  return handle.includes("..");
}

/// The single predicate a handle must pass, all rules together. Nothing in
/// this codebase currently needs anything less than the full check, but the
/// server reports shape/length and the repeated-dot rule as two different
/// messages, so it composes `HANDLE_SHAPE` and `hasRepeatedDot` itself rather
/// than calling this.
export function isValidHandle(handle: string): boolean {
  return (
    handle.length >= HANDLE_MIN_LENGTH &&
    handle.length <= HANDLE_MAX_LENGTH &&
    HANDLE_SHAPE.test(handle) &&
    !hasRepeatedDot(handle)
  );
}

/// Strips a raw string down to the characters a handle may contain, leaving
/// case untouched. What the claim form's live input filter needs — it keeps
/// whatever case the user typed and only lowercases at submission.
export function keepHandleChars(raw: string): string {
  return raw.replace(/[^A-Za-z0-9._-]/g, "");
}

/// Turns free text (typically an email's local part) into a handle that
/// already satisfies `isValidHandle`: lowercases, drops characters outside the
/// charset, collapses runs of dots, and trims punctuation off both ends —
/// including after the length cap, since truncating alone can land the cut on
/// a dot or dash and hand back something `HANDLE_SHAPE` would then reject.
export function normalizeHandle(raw: string): string {
  const trimPunctuation = (value: string) => value.replace(/^[._-]+|[._-]+$/g, "");
  const cleaned = trimPunctuation(keepHandleChars(raw.toLowerCase()).replace(/\.{2,}/g, "."));
  const truncated = trimPunctuation(cleaned.slice(0, HANDLE_MAX_LENGTH));
  return truncated.length >= HANDLE_MIN_LENGTH ? truncated : "";
}

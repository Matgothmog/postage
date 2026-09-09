/// Fails loudly at the call site rather than letting `undefined` reach an API.
export function required(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set`);
  return value;
}

/// Unset (or empty) is the documented local/test default, and stays silent.
/// Anything else must be exactly "live" or "mock" — a near-miss (wrong case,
/// stray whitespace, a typo) is refused rather than trimmed or lowercased,
/// because normalizing it would hide the very misconfiguration this guards
/// against: an operator who meant "live" and silently got mock instead.
export function identityMode(): "live" | "mock" {
  const value = process.env.IDENTITY_MODE;
  if (!value) return "mock";
  if (value === "live" || value === "mock") return value;
  throw new Error(`IDENTITY_MODE is set to an unrecognised value: "${value}". Expected "live" or "mock".`);
}

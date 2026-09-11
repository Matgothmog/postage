/// Fails loudly at the call site rather than letting `undefined` reach an API.
export function required(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set`);
  return value;
}

/// Unset (or empty) defaults to live — the safer failure mode for an
/// identity-verification gate is to require a real proof, not to skip it.
/// Anything else must be exactly "live" or "mock": a near-miss (wrong case,
/// stray whitespace, a typo) throws rather than being coerced to whichever
/// value it resembles. An operator who typed "Mock " meant mock — silently
/// resolving that to "live", the new default, would be the dangerous
/// outcome, so a near-miss fails loudly instead of guessing.
export function identityMode(): "live" | "mock" {
  const value = process.env.IDENTITY_MODE;
  if (!value) return "live";
  if (value === "live" || value === "mock") return value;
  throw new Error(`IDENTITY_MODE is set to an unrecognised value: "${value}". Expected "live" or "mock".`);
}

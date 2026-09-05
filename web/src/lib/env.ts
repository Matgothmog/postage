/// Fails loudly at the call site rather than letting `undefined` reach an API.
export function required(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set`);
  return value;
}

export function identityMode(): "live" | "mock" {
  return process.env.IDENTITY_MODE === "live" ? "live" : "mock";
}

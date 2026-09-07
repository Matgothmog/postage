import { createPublicKey, verify } from "node:crypto";
import { required } from "./env";

/// The accounts Privy has already confirmed for whoever is holding this token.
export interface PrivyIdentity {
  userId: string;
  /// Null for someone who signed in with a passkey and never linked an address.
  email: string | null;
  wallets: string[];
}

const ISSUER = "privy.io";
const ALGORITHM = "ES256";

interface Jwk {
  kid?: string;
  kty: string;
  crv: string;
  x: string;
  y: string;
}

interface LinkedAccount {
  type: string;
  address?: string;
}

let cachedKeys: Promise<Jwk[]> | null = null;

function appId(): string {
  return required("NEXT_PUBLIC_PRIVY_APP_ID");
}

async function signingKeys(): Promise<Jwk[]> {
  if (cachedKeys) return cachedKeys;
  cachedKeys = (async () => {
    const response = await fetch(`https://auth.privy.io/api/v1/apps/${appId()}/jwks.json`, {
      cache: "no-store",
    });
    if (!response.ok) throw new Error(`Privy JWKS returned ${response.status}`);
    return ((await response.json()) as { keys?: Jwk[] }).keys ?? [];
  })();

  try {
    return await cachedKeys;
  } catch (cause) {
    // A failed fetch must not be remembered, or one bad minute breaks signup
    // until the process is replaced.
    cachedKeys = null;
    throw cause;
  }
}

function decode(segment: string): unknown {
  return JSON.parse(Buffer.from(segment, "base64url").toString("utf8"));
}

/// Reads the identity token Privy issues alongside a session.
///
/// Its claims carry the accounts Privy verified - the address someone proved
/// they could read when they signed in, and the wallet minted for them - signed
/// by a key only Privy holds. That is the same fact our own emailed code
/// establishes, already established, which is why the short signup does not ask
/// for one.
///
/// Returns null rather than throwing on anything malformed, because an absent or
/// stale token is an ordinary state that falls back to the longer path.
export async function readIdentity(token: string | null): Promise<PrivyIdentity | null> {
  if (!token) return null;

  const [head, payload, signature] = token.split(".");
  if (!head || !payload || !signature) return null;

  try {
    const header = decode(head) as { alg?: string; kid?: string };
    // The token names its own algorithm, so it is checked against the one we
    // accept rather than trusted - "none" is a signature nobody has to forge.
    if (header.alg !== ALGORITHM) return null;

    const keys = await signingKeys();
    const jwk = keys.find((key) => key.kid === header.kid) ?? (keys.length === 1 ? keys[0] : null);
    if (!jwk) return null;

    const signed = verify(
      "sha256",
      Buffer.from(`${head}.${payload}`),
      { key: createPublicKey({ key: jwk as never, format: "jwk" }), dsaEncoding: "ieee-p1363" },
      Buffer.from(signature, "base64url")
    );
    if (!signed) return null;

    return claims(decode(payload) as Record<string, unknown>);
  } catch {
    return null;
  }
}

function claims(payload: Record<string, unknown>): PrivyIdentity | null {
  const audience = payload.aud;
  const forThisApp = Array.isArray(audience) ? audience.includes(appId()) : audience === appId();
  if (payload.iss !== ISSUER || !forThisApp) return null;
  if (typeof payload.exp !== "number" || payload.exp <= Math.floor(Date.now() / 1000)) return null;
  if (typeof payload.sub !== "string") return null;

  const accounts = linkedAccounts(payload.linked_accounts);
  const email = accounts.find((account) => account.type === "email")?.address ?? null;

  return {
    userId: payload.sub,
    email: email ? email.toLowerCase() : null,
    wallets: accounts
      .filter((account) => account.type === "wallet" && account.address?.startsWith("0x"))
      .map((account) => account.address!.toLowerCase()),
  };
}

/// Privy stringifies this claim rather than nesting it, so it arrives as JSON
/// inside JSON.
function linkedAccounts(claim: unknown): LinkedAccount[] {
  const parsed = typeof claim === "string" ? JSON.parse(claim) : claim;
  return Array.isArray(parsed) ? (parsed as LinkedAccount[]) : [];
}

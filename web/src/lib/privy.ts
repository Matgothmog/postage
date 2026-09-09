import { createPublicKey, verify } from "node:crypto";
import { required } from "./env";
import { now } from "./time";

/// The accounts Privy has already confirmed for whoever is holding this token.
export interface PrivyIdentity {
  userId: string;
  /// Null for someone who signed in with a passkey and never linked an address.
  email: string | null;
  wallets: string[];
}

const ISSUER = "privy.io";
const ALGORITHM = "ES256";

/// Bounds how often the JWKS is fetched again, for either of the two reasons
/// it ever is: a `kid` the cached set does not name, and a fetch that failed.
/// Privy rotates rarely, so one refetch per window finds a real rotation
/// within a minute of the first token that names the new key - and without a
/// bound, a stream of tokens naming a kid nobody ever published (stale or
/// simply forged) would turn into a stream of outbound requests to Privy,
/// one per request we're trying to answer.
const REFRESH_GUARD_SECONDS = 60;

/// How many failed fetches in a row open the cooldown.
///
/// Not one, because a failure is deliberately not remembered as a verdict: a
/// connection that drops once usually does not drop twice, and one unreachable
/// moment must not push every signed-in user onto the long signup path while a
/// working JWKS sits behind it. Two, because a second failure a moment after
/// the first is a dependency that is down rather than a request that stumbled,
/// and asking a service that is down once per inbound request is how it is
/// held down - our traffic becomes its traffic, and we pay the egress for it.
const FAILURES_BEFORE_COOLDOWN = 2;

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
let lastRefreshAttempt = 0;
/// Consecutive failed fetches, and when the last of them was. Kept apart from
/// `lastRefreshAttempt` because they bound different things: that one bounds
/// refetching over a cache that is present and healthy, this one bounds
/// fetching at all when there is no cache to fall back on.
let failedFetches = 0;
let lastFailedFetch = 0;

function appId(): string {
  return required("NEXT_PUBLIC_PRIVY_APP_ID");
}

async function fetchJwks(): Promise<Jwk[]> {
  const response = await fetch(`https://auth.privy.io/api/v1/apps/${appId()}/jwks.json`, {
    cache: "no-store",
  });
  if (!response.ok) throw new Error(`Privy JWKS returned ${response.status}`);
  return ((await response.json()) as { keys?: Jwk[] }).keys ?? [];
}

/// Waits on a fetch already stored in `cachedKeys`, forgetting it on failure
/// and counting it. A failed fetch must not be remembered as an answer, or one
/// bad minute breaks signup until the process is replaced; what is remembered
/// instead is that it failed, which is what `coolingDown` reads.
async function settle(pending: Promise<Jwk[]>): Promise<Jwk[]> {
  try {
    const keys = await pending;
    failedFetches = 0;
    return keys;
  } catch (cause) {
    cachedKeys = null;
    failedFetches += 1;
    lastFailedFetch = now();
    throw cause;
  }
}

/// True while a JWKS that has failed twice running is to be left alone.
///
/// A cooldown rather than a lockout: it lasts one window from the last failed
/// attempt, so the next lookup after that goes and finds out whether Privy
/// recovered, and the first success clears it.
///
/// What a caller gets meanwhile is the null identity an unreachable JWKS
/// already gave them, and it is handed to them as an empty key set rather than
/// a rejection: no key signs the token, which is both true and the answer, and
/// it spares every reader an exception for a state that is expected. So the
/// cooldown costs the longer signup path and nothing else.
function coolingDown(): boolean {
  if (failedFetches < FAILURES_BEFORE_COOLDOWN) return false;
  return now() - lastFailedFetch < REFRESH_GUARD_SECONDS;
}

async function signingKeys(): Promise<Jwk[]> {
  if (cachedKeys) return cachedKeys;
  if (coolingDown()) return [];
  cachedKeys = fetchJwks();
  return settle(cachedKeys);
}

/// Forces one refetch of the JWKS, at most once per `REFRESH_GUARD_SECONDS`
/// regardless of how many lookups miss in the meantime. Called only after
/// the cached set has already been checked and failed to name the `kid` -
/// see `readIdentity`.
async function refreshSigningKeys(): Promise<Jwk[]> {
  if (cachedKeys && now() - lastRefreshAttempt < REFRESH_GUARD_SECONDS) return cachedKeys;
  if (coolingDown()) return cachedKeys ?? [];
  lastRefreshAttempt = now();
  cachedKeys = fetchJwks();
  return settle(cachedKeys);
}

/// The key naming `kid`, or the only key published when the header names one
/// the set doesn't have - Privy publishes exactly one key outside a rotation
/// window, so that's the only case with one honest answer.
function matchKey(keys: Jwk[], kid: string | undefined): Jwk | null {
  return keys.find((key) => key.kid === kid) ?? (keys.length === 1 ? keys[0] : null);
}

/// Resolves the key `header.kid` names, refetching the JWKS once (guarded)
/// when the cached set doesn't have it - a rotation might have landed since
/// the cache was built, and the only way to tell is to go look. A header
/// naming no kid never benefits from a refetch, since nothing about a fresh
/// fetch resolves which of several keys an absent kid means, so that case
/// goes straight to the single-key fallback on the set already cached.
async function resolveSigningKey(kid: string | undefined): Promise<Jwk | null> {
  const keys = await signingKeys();
  if (!kid) return matchKey(keys, kid);

  const named = keys.find((key) => key.kid === kid);
  if (named) return named;

  return matchKey(await refreshSigningKeys(), kid);
}

function decode(segment: string): unknown {
  return JSON.parse(Buffer.from(segment, "base64url").toString("utf8"));
}

/// Reads the identity token Privy issues alongside a session.
///
/// Its claims carry the accounts Privy verified - the address someone proved
/// they could read when they signed in, and the wallet minted for them - signed
/// by a key only Privy holds. That is the fact our emailed code exists to
/// establish, already established, which is why the short signup skips it.
///
/// Returns null rather than throwing on anything malformed, because an absent or
/// stale token is an ordinary state that falls back to the longer path.
export async function readIdentity(token: string | null): Promise<PrivyIdentity | null> {
  if (!token) return null;

  // A JWT has exactly three segments. Destructuring more than that silently
  // drops everything past the third, which used to let `<valid token>.junk`
  // verify on the three segments that were actually signed.
  const segments = token.split(".");
  if (segments.length !== 3) return null;
  const [head, payload, signature] = segments;
  if (!head || !payload || !signature) return null;

  try {
    const header = decode(head) as { alg?: string; kid?: string };
    // The token names its own algorithm, so it is checked against the one we
    // accept rather than trusted - "none" is a signature nobody has to forge.
    if (header.alg !== ALGORITHM) return null;

    const jwk = await resolveSigningKey(header.kid);
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
  if (typeof payload.exp !== "number" || payload.exp <= now()) return null;
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

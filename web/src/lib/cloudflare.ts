import { required } from "./env";

const API = "https://api.cloudflare.com/client/v4";

/// A destination address on the Cloudflare account. `verifiedAt` is null until
/// the owner clicks the link Cloudflare mails them.
export interface Destination {
  id: string;
  verifiedAt: number | null;
}

interface Address {
  id: string;
  email: string;
  verified: string | null;
}

interface Envelope<T> {
  success: boolean;
  errors: { code: number; message: string }[];
  result: T;
  /// Present on list responses only.
  result_info?: { page: number; per_page: number; total_count: number };
}

async function call<T>(path: string, init?: RequestInit): Promise<Envelope<T>> {
  const response = await fetch(`${API}/accounts/${required("CLOUDFLARE_ACCOUNT_ID")}${path}`, {
    ...init,
    headers: {
      Authorization: `Bearer ${required("CLOUDFLARE_API_TOKEN")}`,
      "Content-Type": "application/json",
      ...init?.headers,
    },
    cache: "no-store",
  });

  const body = await response.text();
  let payload: Envelope<T>;
  try {
    payload = JSON.parse(body) as Envelope<T>;
  } catch {
    // An edge 5xx or a WAF challenge answers with HTML, not JSON.
    throw new Error(`Cloudflare returned ${response.status}`);
  }

  // A refusal that names an address as already registered is an answer we act
  // on. Anything else — a revoked token, a rate limit, an outage — is a
  // failure, and returning it as data would read as "not verified yet" forever.
  if (!response.ok && !isDuplicate(payload)) {
    throw new Error(payload.errors?.[0]?.message ?? `Cloudflare returned ${response.status}`);
  }
  return payload;
}

/// Cloudflare sends a zero value rather than null for an address it has not
/// verified, and an unparseable date would otherwise become NaN — which passes
/// every null check and then fails the database bind.
function toDestination(address: Address): Destination {
  const parsed = address.verified ? Date.parse(address.verified) : Number.NaN;
  const seconds = Math.floor(parsed / 1000);
  const verifiedAt = Number.isFinite(seconds) && seconds > 0 ? seconds : null;
  return { id: address.id, verifiedAt };
}

/// The one refusal that carries information rather than a fault: this address
/// is already on the account.
function isDuplicate(payload: Envelope<unknown>): boolean {
  return (payload.errors ?? []).some((error) => /exist|already/i.test(error.message ?? ""));
}

/// Registers the address so `message.forward()` will accept it, which also
/// makes Cloudflare send its verification mail. Addresses are shared across the
/// whole account, so one that another user already registered comes back as it
/// stands rather than as an error.
export async function ensureDestination(email: string): Promise<Destination> {
  const created = await call<Address>("/email/routing/addresses", {
    method: "POST",
    body: JSON.stringify({ email: email.toLowerCase() }),
  });
  if (created.success) return toDestination(created.result);

  const existing = await findDestination(email);
  if (existing) return existing;

  throw new Error(created.errors?.[0]?.message ?? "Cloudflare refused the destination address");
}

const PER_PAGE = 50;

/// Every page, not the first one. One destination is created per inbox and none
/// are ever deleted, so a single page stopped answering for older addresses at
/// roughly the fiftieth signup — and the caller reads a miss as "Cloudflare
/// refused the address", which is a signup that fails for good.
async function findDestination(email: string): Promise<Destination | null> {
  const wanted = email.toLowerCase();

  for (let page = 1; ; page += 1) {
    const listed = await call<Address[]>(
      `/email/routing/addresses?per_page=${PER_PAGE}&page=${page}&direction=desc`
    );
    const addresses = listed.result ?? [];

    const match = addresses.find((address) => address.email.toLowerCase() === wanted);
    if (match) return toDestination(match);

    // Stops on a short page as well as on the count, so a response without
    // `result_info` cannot turn this into an unbounded loop.
    const seen = (listed.result_info?.page ?? page) * PER_PAGE;
    if (addresses.length < PER_PAGE || seen >= (listed.result_info?.total_count ?? 0)) return null;
  }
}

export async function destinationStatus(id: string): Promise<Destination | null> {
  const found = await call<Address>(`/email/routing/addresses/${id}`);
  return found.success ? toDestination(found.result) : null;
}

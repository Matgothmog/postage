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

  const payload = (await response.json()) as Envelope<T>;
  if (!response.ok && !payload.errors?.length) {
    throw new Error(`Cloudflare returned ${response.status}`);
  }
  return payload;
}

function toDestination(address: Address): Destination {
  return { id: address.id, verifiedAt: address.verified ? Math.floor(Date.parse(address.verified) / 1000) : null };
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

export async function findDestination(email: string): Promise<Destination | null> {
  const wanted = email.toLowerCase();
  const listed = await call<Address[]>(`/email/routing/addresses?per_page=50&direction=desc`);
  const match = listed.result?.find((address) => address.email.toLowerCase() === wanted);
  return match ? toDestination(match) : null;
}

export async function destinationStatus(id: string): Promise<Destination | null> {
  const found = await call<Address>(`/email/routing/addresses/${id}`);
  return found.success ? toDestination(found.result) : null;
}

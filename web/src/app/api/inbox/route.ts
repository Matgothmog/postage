import { isAddress } from "viem";
import { createInbox, inboxByHandle, inboxByWallet } from "@/lib/db";

const HANDLE = /^[a-z0-9][a-z0-9._-]{1,30}$/;
const EMAIL = /^[^@\s]+@[^@\s]+\.[^@\s]+$/;

/// Where a signed-in user sees their inbox, or nothing if they have not made
/// one yet.
export async function GET(request: Request) {
  const wallet = new URL(request.url).searchParams.get("wallet");
  if (!wallet || !isAddress(wallet)) {
    return Response.json({ error: "A valid wallet is required" }, { status: 400 });
  }
  return Response.json({ inbox: await inboxByWallet(wallet) });
}

/// Claims handle@usepostage.com and points it at an address the user already
/// reads. Cloudflare verifies that destination separately before any mail is
/// forwarded to it, which is also what proves they control it.
export async function POST(request: Request) {
  const { handle, destination, wallet } = (await request.json()) as {
    handle?: string;
    destination?: string;
    wallet?: string;
  };

  if (!handle || !HANDLE.test(handle.toLowerCase())) {
    return Response.json({ error: "Pick 2-31 characters: letters, digits, dot, dash" }, { status: 400 });
  }
  if (!destination || !EMAIL.test(destination)) {
    return Response.json({ error: "A valid destination address is required" }, { status: 400 });
  }
  if (!wallet || !isAddress(wallet)) {
    return Response.json({ error: "A valid wallet is required" }, { status: 400 });
  }

  const existing = await inboxByHandle(handle);
  if (existing && existing.wallet !== wallet.toLowerCase()) {
    return Response.json({ error: "That handle is taken" }, { status: 409 });
  }

  await createInbox(handle, destination, wallet);
  return Response.json({ handle: handle.toLowerCase(), destination, address: `${handle.toLowerCase()}@usepostage.com` });
}

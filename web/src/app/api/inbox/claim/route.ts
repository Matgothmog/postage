import { isAddress } from "viem";
import { claimInbox, inboxMessages, walletForInbox } from "@/lib/db";

const LOCAL_PART = /^[a-z0-9][a-z0-9._-]{1,30}$/;

export async function POST(request: Request) {
  const { localPart, wallet } = (await request.json()) as {
    localPart?: string;
    wallet?: string;
  };

  if (!localPart || !LOCAL_PART.test(localPart.toLowerCase())) {
    return Response.json({ error: "Pick a name of 2-31 letters, digits, dot, dash" }, { status: 400 });
  }
  if (!wallet || !isAddress(wallet)) {
    return Response.json({ error: "A valid wallet is required" }, { status: 400 });
  }

  const existing = await walletForInbox(localPart);
  if (existing && existing !== wallet.toLowerCase()) {
    return Response.json({ error: "That name is taken" }, { status: 409 });
  }

  await claimInbox(localPart, wallet);
  return Response.json({ localPart: localPart.toLowerCase(), wallet: wallet.toLowerCase() });
}

export async function GET(request: Request) {
  const localPart = new URL(request.url).searchParams.get("localPart");
  if (!localPart) return Response.json({ error: "localPart required" }, { status: 400 });
  return Response.json({ messages: await inboxMessages(localPart) });
}

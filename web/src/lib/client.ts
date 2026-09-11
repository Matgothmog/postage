import { createPublicClient, http } from "viem";
import { chain } from "./contracts";

/// The JSON-RPC endpoint every chain interaction *this server* makes goes
/// through — its reads and the attestation write alike.
///
/// Not the app's only route to the chain, despite being the only one named in
/// this file. `providers.tsx:34-35` hands Privy `defaultChain`/`supportedChains`
/// straight from the chain definition, so a sender paying from their browser
/// wallet signs and broadcasts over whatever public RPC `arcTestnet` ships
/// with and never touches this transport at all. Pointing `ARC_RPC_URL`
/// somewhere bounds what the server does, not what the product does.
///
/// `http()` with no argument uses whatever public RPC the chain definition ships
/// with, shared by everyone using it. Every inbound message reads
/// `effectiveFloor`, so a busy gateway is one rate limit away from being unable
/// to price anything — and the tests, which are the same code, were the first to
/// hit it. Unset, the default still applies, so nothing has to be configured for
/// this to run.
///
/// Exported so `recordPersonhood` (`api/world/verify/route.ts`) can build its
/// wallet client on this exact transport instead of a second, unconfigured one.
/// A relayer that signs and sends through a different provider than the one
/// `publicClient` reads and awaits receipts through can broadcast a
/// transaction the receipt wait never sees — two providers with two views of
/// the same chain, one of them blind to the other's traffic.
export const rpcTransport = http(process.env.ARC_RPC_URL);

export const publicClient = createPublicClient({
  chain,
  transport: rpcTransport,
});

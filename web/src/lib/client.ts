import { createPublicClient, http } from "viem";
import { chain } from "./contracts";

/// Reads the chain, through an endpoint the operator chooses.
///
/// `http()` with no argument uses whatever public RPC the chain definition ships
/// with, shared by everyone using it. Every inbound message reads
/// `effectiveFloor`, so a busy gateway is one rate limit away from being unable
/// to price anything — and the tests, which are the same code, were the first to
/// hit it. Unset, the default still applies, so nothing has to be configured for
/// this to run.
export const publicClient = createPublicClient({
  chain,
  transport: http(process.env.ARC_RPC_URL),
});

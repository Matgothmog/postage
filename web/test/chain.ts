import { createServer } from "node:http";
import type { AddressInfo } from "node:net";

/// A stand-in for the chain, listening on loopback.
///
/// Every held message reads `effectiveFloor` off the chain. Answered here rather
/// than by the public RPC in the chain definition, which is shared with everyone
/// else using it and rate limits accordingly — these tests are about what the
/// gateway decides, and a verdict should not depend on somebody else's traffic.
export interface StubChain {
  /// What `effectiveFloor` answers with, so a test can say where a price came
  /// from rather than restating the arithmetic that produced it.
  readonly floor: bigint;
  close(): Promise<void>;
}

/// Starts the stub and points the chain client at it for the rest of this
/// process. Await it before importing anything that reaches the chain: the
/// client reads its endpoint once, when it is first imported.
export async function startStubChain(floor: bigint = 10n ** 16n): Promise<StubChain> {
  const server = createServer((request, response) => {
    let body = "";
    request.on("data", (chunk) => (body += chunk));
    request.on("end", () => {
      const { id } = JSON.parse(body) as { id: number };
      response.writeHead(200, { "Content-Type": "application/json" });
      response.end(
        JSON.stringify({ jsonrpc: "2.0", id, result: `0x${floor.toString(16).padStart(64, "0")}` })
      );
    });
  });

  await new Promise<void>((ready) => server.listen(0, "127.0.0.1", ready));
  process.env.ARC_RPC_URL = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;

  return {
    floor,
    close: () =>
      new Promise<void>((closed) => {
        // The client keeps its sockets alive, so a plain close would wait for a
        // pool that is not going to disconnect itself and hang the suite.
        server.closeAllConnections();
        server.close(() => closed());
      }),
  };
}

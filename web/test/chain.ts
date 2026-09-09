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
  /// Set to make every call refuse instead of answering, which is the other
  /// thing a chain does to a gateway carrying real mail. The refusal is a
  /// JSON-RPC error rather than a closed port or an HTTP 5xx: viem retries
  /// both of those with backoff and does not retry this one, so a test that
  /// means "the chain said no" costs milliseconds rather than seconds.
  failing: boolean;
  close(): Promise<void>;
}

/// Starts the stub and points the chain client at it for the rest of this
/// process. Await it before importing anything that reaches the chain: the
/// client reads its endpoint once, when it is first imported.
export async function startStubChain(floor: bigint = 10n ** 16n): Promise<StubChain> {
  let failing = false;

  const server = createServer((request, response) => {
    let body = "";
    request.on("data", (chunk) => (body += chunk));
    request.on("end", () => {
      const { id } = JSON.parse(body) as { id: number };
      response.writeHead(200, { "Content-Type": "application/json" });
      const answer = failing
        ? { error: { code: -32000, message: "the node is not accepting calls" } }
        : { result: `0x${floor.toString(16).padStart(64, "0")}` };
      response.end(JSON.stringify({ jsonrpc: "2.0", id, ...answer }));
    });
  });

  await new Promise<void>((ready) => server.listen(0, "127.0.0.1", ready));
  process.env.ARC_RPC_URL = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;

  return {
    floor,
    get failing() {
      return failing;
    },
    set failing(refusing: boolean) {
      failing = refusing;
    },
    close: () =>
      new Promise<void>((closed) => {
        // The client keeps its sockets alive, so a plain close would wait for a
        // pool that is not going to disconnect itself and hang the suite.
        server.closeAllConnections();
        server.close(() => closed());
      }),
  };
}

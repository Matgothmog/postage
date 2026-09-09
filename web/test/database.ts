import { createServer } from "node:http";
import type { AddressInfo } from "node:net";

/// A database that accepts the connection and then refuses the query.
///
/// This is the shape the live outage had: Turso was reachable and would not
/// serve, and the gateway answered the mail worker with an empty 500 that told
/// nobody why. A closed port would be a different failure - the client words
/// that one itself, as a transport error - so the refusal is served rather than
/// the port left shut, and served as an HTTP error because the libsql client
/// does not retry one.
export interface BrokenDatabase {
  /// What `DATABASE_URL` was pointed at, so a test can say the response does
  /// not quote the connection back to whoever called it.
  readonly url: string;
  /// Likewise for the credential that travels with it.
  readonly authToken: string;
  close(): Promise<void>;
}

/// Long enough to be mistaken for a real one, which is the point: a test that
/// asserts it never appears anywhere is asserting something about a value the
/// redaction rules would actually have to catch.
const AUTH_TOKEN = "not-a-real-turso-token-but-long-enough-to-look-like-one";

const REFUSAL = "SQLITE_UNKNOWN: the server is not accepting queries";

/// Starts the stub and points the database client at it for the rest of this
/// process. Await it before importing anything that queries: the client reads
/// `DATABASE_URL` when it first opens a connection, and keeps what it opened.
export async function startBrokenDatabase(): Promise<BrokenDatabase> {
  const server = createServer((request, response) => {
    request.resume();
    response.writeHead(500, { "Content-Type": "text/plain" }).end(REFUSAL);
  });

  await new Promise<void>((ready) => server.listen(0, "127.0.0.1", ready));
  const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;

  process.env.DATABASE_URL = url;
  process.env.DATABASE_AUTH_TOKEN = AUTH_TOKEN;

  return {
    url,
    authToken: AUTH_TOKEN,
    close: () =>
      new Promise<void>((closed) => {
        // The client keeps its sockets alive, so a plain close would wait for a
        // pool that is not going to disconnect itself and hang the suite.
        server.closeAllConnections();
        server.close(() => closed());
      }),
  };
}

import { createServer } from "node:http";
import type { AddressInfo } from "node:net";

/// A stand-in for the classifier's model, listening on loopback.
///
/// `classify()` reaches Anthropic through the SDK, and the SDK finds credentials
/// wherever it can: `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, a profile the
/// developer logged into on this machine. So a test that means "the model is not
/// reachable, and every verdict here is the header fallback" cannot say that by
/// leaving a variable unset — that is a statement about the machine, not about
/// the test. On a laptop with credentials the same file quietly starts making
/// live calls, stops testing what its names claim, and goes on passing.
///
/// Pointing the SDK at a server this process owns settles both halves: what the
/// model says is part of the test, and nothing can leave the machine either way.
export interface StubModel {
  /// How many times the SDK actually asked. A test that means to reach the model
  /// and one that means not to can then both say so rather than assume it.
  readonly calls: number;
  close(): Promise<void>;
}

/// Starts the stub and redirects the SDK at it for the rest of this process.
///
/// `answer` is returned as the model's verdict; `null` refuses every request,
/// which is what `classify()` degrades on and the only deliberate way to reach
/// the header fallback. The refusal is a 400 rather than a dead port because the
/// SDK retries a connection failure with backoff and does not retry a 400 —
/// several hundred classify calls per suite make that the difference between
/// milliseconds and minutes.
export async function startStubModel(answer: object | null = null): Promise<StubModel> {
  let calls = 0;

  const server = createServer((request, response) => {
    calls += 1;
    request.resume();
    if (!answer) {
      response.writeHead(400, JSON_HEADERS).end(JSON.stringify(REFUSAL));
      return;
    }
    response.writeHead(200, JSON_HEADERS).end(JSON.stringify(messageSaying(answer)));
  });

  await new Promise<void>((ready) => server.listen(0, "127.0.0.1", ready));
  const { port } = server.address() as AddressInfo;

  // A key of our own as well as the URL. Without one the SDK falls through to
  // the machine's own credential chain, which can supply both a token and a base
  // URL of its own — and then the redirect below is advice rather than a rule.
  process.env.ANTHROPIC_BASE_URL = `http://127.0.0.1:${port}`;
  process.env.ANTHROPIC_API_KEY = "sk-ant-nothing-real-answers-this";

  return {
    get calls() {
      return calls;
    },
    close: () =>
      new Promise<void>((closed) => {
        // The SDK keeps its sockets alive, so a plain close would wait for a
        // pool that is not going to disconnect itself and hang the suite.
        server.closeAllConnections();
        server.close(() => closed());
      }),
  };
}

const JSON_HEADERS = { "Content-Type": "application/json" };

const REFUSAL = {
  type: "error",
  error: { type: "invalid_request_error", message: "no model is reachable from a test" },
};

/// The shape `messages.parse` reads: it takes the first text block and hands it
/// to the output format's parser, so the verdict travels as JSON inside it.
function messageSaying(answer: object) {
  return {
    id: "msg_stub",
    type: "message",
    role: "assistant",
    model: "claude-opus-5",
    content: [{ type: "text", text: JSON.stringify(answer) }],
    stop_reason: "end_turn",
    stop_sequence: null,
    usage: { input_tokens: 0, output_tokens: 0 },
  };
}

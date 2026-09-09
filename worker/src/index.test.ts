import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import worker, { authResults } from "./index.ts";

const CLEAN =
  "mx.cloudflare.net; dkim=pass header.d=example.com header.i=@example.com; " +
  "spf=pass smtp.mailfrom=sender@example.com; dmarc=pass header.from=example.com";

test("a single header from the receiving MTA is read straight through", () => {
  assert.deepEqual(authResults(CLEAN), { spf: "pass", dkim: "pass", dmarc: "pass" });
});

test("results are read case-insensitively and reported lowercase", () => {
  assert.deepEqual(authResults("mx; SPF=Pass; DKIM=FAIL; DMARC=None"), {
    spf: "pass",
    dkim: "fail",
    dmarc: "none",
  });
});

/// The regression this file was written for. `policy.dmarc` is a registered
/// RFC 8601 property, so an MTA stating the domain's published policy beside
/// its verdict is writing ordinary header syntax - and read with a word
/// boundary, the `dmarc` inside it looks like a second, contradicting result.
/// A DMARC-passing message then reports unknown, which the gateway treats as
/// unauthenticated: honest mail, downgraded by its own authentication header.
test("a policy property does not contradict the result it belongs to", () => {
  assert.equal(authResults("mx; dmarc=pass policy.dmarc=none; spf=pass").dmarc, "pass");
});

/// A header in the shape one actually arrives in: comments inside a result,
/// properties either side of it, and the published policy stated beside the
/// verdict. Every one of these was a way to lose a result to a word boundary.
test("a header as a real MTA writes it reads all three methods", () => {
  const header =
    "mx.google.com; dkim=pass header.i=@example.com header.b=\"AbC\"; " +
    "spf=pass (google.com: domain of sender@example.com designates 1.2.3.4) " +
    "smtp.mailfrom=sender@example.com; " +
    "dmarc=pass (p=NONE sp=QUARANTINE dis=NONE) policy.dmarc=none header.from=example.com";

  assert.deepEqual(authResults(header), { spf: "pass", dkim: "pass", dmarc: "pass" });
});

test("a vendor's own x- method does not contradict the standard one", () => {
  assert.equal(authResults("mx; dkim=pass header.d=example.com; x-dkim=fail").dkim, "pass");
});

test("a property whose name ends in a method name is not read as a result", () => {
  assert.deepEqual(authResults("mx; spf=pass smtp.mailfrom=a@b.com policy.spf=none"), {
    spf: "pass",
    dkim: null,
    dmarc: null,
  });
});

/// `Headers.get()` joins every copy of the header into one string, and the
/// sender may have written some of them. The rule that survives that - believe
/// a method only when every copy agrees - is settled; these two pin it.
test("copies that agree are believed", () => {
  assert.equal(authResults("mx1; dmarc=pass, mx2; dmarc=pass").dmarc, "pass");
});

test("a copy contradicting another leaves the method unknown", () => {
  assert.deepEqual(authResults("mx1; dmarc=pass; spf=pass, mx2; dmarc=fail"), {
    spf: "pass",
    dkim: null,
    dmarc: null,
  });
});

/// The gap the function's own comment describes, in executable form: with no
/// result from the receiving MTA there is nothing for a forged claim to
/// disagree with. Recorded so that closing it is a visible change here rather
/// than a silent one.
test("a lone claim stands when no other copy speaks to it", () => {
  assert.equal(authResults("mx; dmarc=pass").dmarc, "pass");
});

test("a message with no authentication header authenticates nothing", () => {
  assert.deepEqual(authResults(null), { spf: null, dkim: null, dmarc: null });
});

test("a header naming no method authenticates nothing", () => {
  assert.deepEqual(authResults("mx.cloudflare.net; none"), {
    spf: null,
    dkim: null,
    dmarc: null,
  });
});

test("a truncated method with no result is not a result", () => {
  assert.deepEqual(authResults("mx; dmarc= ; spf="), { spf: null, dkim: null, dmarc: null });
});

test("an empty header authenticates nothing", () => {
  assert.deepEqual(authResults(""), { spf: null, dkim: null, dmarc: null });
});

// --- the inbound handler ------------------------------------------------
//
// `email()` is driven here through the two boundaries it actually has: the
// gateway it asks for a verdict, and the message and KV namespace Cloudflare
// hands it. Both are replaced with recorders, so what is asserted is which
// branch ran and what it did, never what a stand-in was told to say.

const RETRY = "Postage is temporarily unavailable, please retry";
const SENDER = "sender@example.com";
const RECIPIENT = "demo@usepostage.com";
const DAY_SECONDS = 24 * 60 * 60;

const RAW_MESSAGE = [
  `From: ${SENDER}`,
  `To: ${RECIPIENT}`,
  "Subject: A question",
  "",
  "Is this thing on?",
].join("\r\n");

interface Recorder {
  rejects: string[];
  forwards: string[];
  replies: string[];
}

interface Held {
  key: string;
  value: ArrayBuffer;
  expiration?: number;
}

interface GatewayCall {
  url: string;
  secret: string | null;
  payload: Record<string, unknown>;
}

const realFetch = globalThis.fetch;
const realConsoleError = console.error;

afterEach(() => {
  globalThis.fetch = realFetch;
  console.error = realConsoleError;
});

/// Kept rather than printed: a handler that logs on its way to a refusal is
/// half of what these tests check, and letting it through would bury the runner
/// output in errors from tests that passed.
function collectedErrors(): string[] {
  const labels: string[] = [];
  console.error = (...args: unknown[]) => {
    labels.push(String(args[0]));
  };
  return labels;
}

function gatewayAnswers(status: number, body: unknown): GatewayCall[] {
  const calls: GatewayCall[] = [];
  globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push({
      url: String(input),
      secret: new Headers(init?.headers).get("x-postage-secret"),
      payload: JSON.parse(String(init?.body)) as Record<string, unknown>,
    });
    return new Response(JSON.stringify(body), { status });
  };
  return calls;
}

function gatewayIsUnreachable(): void {
  globalThis.fetch = async () => {
    throw new Error("connect ECONNREFUSED");
  };
}

/// The handler calls one method of a namespace that declares dozens. Widening
/// the stub to the whole interface would mean inventing return values no test
/// reads back, so it is narrowed here and cast once.
function heldStore(failure?: string): { puts: Held[]; namespace: KVNamespace } {
  const puts: Held[] = [];
  const namespace = {
    async put(key: string, value: ArrayBuffer, options?: { expiration?: number }) {
      if (failure) throw new Error(failure);
      puts.push({ key, value, expiration: options?.expiration });
    },
  } as unknown as KVNamespace;
  return { puts, namespace };
}

/// Taken off the handler rather than restated: the worker's `Env` is private to
/// its module, and a second copy of it here would drift from the real one
/// without either side noticing.
type Environment = Parameters<typeof worker.email>[1];

function environment(namespace: KVNamespace): Environment {
  return {
    POSTAGE_API_URL: "https://gateway.test",
    POSTAGE_SECRET: "shh",
    HELD: namespace,
    MAILGUN_API_BASE: "https://api.mailgun.test",
    MAILGUN_DOMAIN: "usepostage.com",
    MAILGUN_API_KEY: "key",
  };
}

interface InboundOptions {
  /// `undefined` for a clean header, `null` for a message that arrived without
  /// one at all - the two the handler has to tell apart.
  authentication?: string | null;
  forwardFails?: boolean;
  replyFails?: boolean;
}

function inbound(options: InboundOptions = {}): { message: ForwardableEmailMessage; recorder: Recorder } {
  const bytes = new TextEncoder().encode(RAW_MESSAGE);
  const recorder: Recorder = { rejects: [], forwards: [], replies: [] };
  const headers = new Headers();
  const authentication = options.authentication === undefined ? CLEAN : options.authentication;
  if (authentication !== null) headers.set("authentication-results", authentication);

  const message: ForwardableEmailMessage = {
    from: SENDER,
    to: RECIPIENT,
    rawSize: bytes.byteLength,
    headers,
    raw: new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(bytes);
        controller.close();
      },
    }),
    setReject(reason: string) {
      recorder.rejects.push(reason);
    },
    async forward(rcptTo: string) {
      if (options.forwardFails) throw new Error("not a verified destination");
      recorder.forwards.push(rcptTo);
      return { messageId: "forwarded" };
    },
    // Cloudflare overloads `reply()` and the handler only ever calls the builder
    // form, but a stand-in for the method has to accept both.
    async reply(sent: EmailMessage | EmailReplyMessageBuilder) {
      // Refused for a sender Cloudflare could not authenticate, which is the
      // case the caller has to survive rather than the exception.
      if (options.replyFails) throw new Error("cannot reply to an unauthenticated sender");
      recorder.replies.push("subject" in sent ? sent.subject : "");
      return { messageId: "replied" };
    },
  };

  return { message, recorder };
}

const notice = { subject: "Held for release", text: "Release it here", html: "<p>Release it here</p>" };
const nowSeconds = () => Math.floor(Date.now() / 1000);

/// The collision fix, end to end: a header the gateway would have been told
/// nothing about now reaches it as the pass it is. Everything downstream of
/// this payload - whether the sender clears the gate, whether we write back at
/// all - hangs on that field.
test("the gateway is told what the receiving MTA concluded", async () => {
  const calls = gatewayAnswers(200, { action: "reject", bounce: "no" });
  const store = heldStore();
  const { message } = inbound({ authentication: "mx; dmarc=pass policy.dmarc=none; spf=pass" });

  await worker.email(message, environment(store.namespace));

  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, "https://gateway.test/api/mail/inbound");
  assert.equal(calls[0].secret, "shh");
  assert.equal(calls[0].payload.dmarc, "pass");
  assert.equal(calls[0].payload.spf, "pass");
  assert.equal(calls[0].payload.dkim, null);
  assert.equal(calls[0].payload.from, SENDER);
  assert.equal(calls[0].payload.to, RECIPIENT);
  assert.equal(calls[0].payload.subject, "A question");
  assert.match(String(calls[0].payload.body), /Is this thing on\?/);
});

/// The other end of the same field: a message the receiving MTA said nothing
/// about is reported as unknown rather than as anything, and the gateway - not
/// the worker - is what decides that an unauthenticated sender is not written
/// back to.
test("a message that arrived with no authentication header is reported as unknown", async () => {
  const calls = gatewayAnswers(200, { action: "reject" });
  const store = heldStore();
  const { message } = inbound({ authentication: null });

  await worker.email(message, environment(store.namespace));

  assert.equal(calls[0].payload.spf, null);
  assert.equal(calls[0].payload.dkim, null);
  assert.equal(calls[0].payload.dmarc, null);
});

test("a gateway that cannot be reached refuses the session for a retry", async () => {
  gatewayIsUnreachable();
  const errors = collectedErrors();
  const store = heldStore();
  const { message, recorder } = inbound();

  await worker.email(message, environment(store.namespace));

  assert.deepEqual(recorder.rejects, [RETRY]);
  assert.deepEqual(recorder.forwards, []);
  assert.equal(store.puts.length, 0);
  assert.deepEqual(errors, ["classify failed"]);
});

test("a gateway that answers with an error refuses the session for a retry", async () => {
  gatewayAnswers(500, { error: "boom" });
  collectedErrors();
  const store = heldStore();
  const { message, recorder } = inbound();

  await worker.email(message, environment(store.namespace));

  assert.deepEqual(recorder.rejects, [RETRY]);
});

test("an address that does not exist is refused permanently, not retried", async () => {
  gatewayAnswers(404, {});
  const store = heldStore();
  const { message, recorder } = inbound();

  await worker.email(message, environment(store.namespace));

  assert.deepEqual(recorder.rejects, ["No such address at this domain"]);
});

test("a forward verdict delivers to the address the gateway verified", async () => {
  gatewayAnswers(200, { action: "forward", to: "owner@personal.example" });
  const store = heldStore();
  const { message, recorder } = inbound();

  await worker.email(message, environment(store.namespace));

  assert.deepEqual(recorder.forwards, ["owner@personal.example"]);
  assert.deepEqual(recorder.rejects, []);
  assert.equal(store.puts.length, 0);
});

test("a destination Cloudflare will not accept refuses the session for a retry", async () => {
  gatewayAnswers(200, { action: "forward", to: "owner@personal.example" });
  const store = heldStore();
  const { message, recorder } = inbound({ forwardFails: true });

  await worker.email(message, environment(store.namespace));

  assert.deepEqual(recorder.rejects, ["Postage could not deliver to that inbox, please retry"]);
});

/// A verdict that asks for a forward and names nowhere is our bug, and the
/// message is one the gateway meant to deliver. Refused as temporary so it
/// waits at the sending MTA instead of being written off.
test("a forward with no destination is refused as ours to fix, not the sender's", async () => {
  gatewayAnswers(200, { action: "forward" });
  const errors = collectedErrors();
  const store = heldStore();
  const { message, recorder } = inbound();

  await worker.email(message, environment(store.namespace));

  assert.deepEqual(recorder.rejects, [RETRY]);
  assert.deepEqual(recorder.forwards, []);
  assert.deepEqual(errors, ["incoherent verdict"]);
});

test("a hold stores the bytes that arrived, under the gateway's token", async () => {
  const heldUntil = nowSeconds() + 900;
  gatewayAnswers(200, { action: "hold", token: "tok-1", held_until: heldUntil, bounce: "Held." });
  const store = heldStore();
  const { message, recorder } = inbound();

  await worker.email(message, environment(store.namespace));

  assert.equal(store.puts.length, 1);
  assert.equal(store.puts[0].key, "tok-1");
  assert.equal(store.puts[0].expiration, heldUntil);
  assert.equal(new TextDecoder().decode(store.puts[0].value), RAW_MESSAGE);
  assert.deepEqual(recorder.rejects, ["Held."]);
});

/// The floor under a gateway that forgot to set a deadline. Without it the
/// value would be written with no expiry at all and kept forever.
test("a hold with no deadline is stored under the fallback one", async () => {
  gatewayAnswers(200, { action: "hold", token: "tok-2" });
  const store = heldStore();
  const { message } = inbound();

  await worker.email(message, environment(store.namespace));

  const expiration = store.puts[0].expiration ?? 0;
  assert.ok(
    Math.abs(expiration - (nowSeconds() + DAY_SECONDS)) <= 2,
    `expected roughly a day out, got ${expiration}`
  );
});

test("a sender who was sent the release notice is not also refused", async () => {
  gatewayAnswers(200, { action: "hold", token: "tok-3", notice });
  const store = heldStore();
  const { message, recorder } = inbound();

  await worker.email(message, environment(store.namespace));

  assert.deepEqual(recorder.replies, ["Held for release"]);
  assert.deepEqual(recorder.rejects, []);
});

/// The forged-sender case: Cloudflare will not let us write to someone whose
/// name may not be theirs, so the link has to travel inside the SMTP refusal.
test("a sender who cannot be written to is told inside the session instead", async () => {
  gatewayAnswers(200, { action: "hold", token: "tok-4", notice, bounce: "Held, see the link" });
  const store = heldStore();
  const { message, recorder } = inbound({ replyFails: true });

  await worker.email(message, environment(store.namespace));

  assert.deepEqual(recorder.replies, []);
  assert.deepEqual(recorder.rejects, ["Held, see the link"]);
  assert.equal(store.puts.length, 1);
});

test("a hold the gateway sent no notice for still refuses the session", async () => {
  gatewayAnswers(200, { action: "hold", token: "tok-5", notice: null });
  const store = heldStore();
  const { message, recorder } = inbound();

  await worker.email(message, environment(store.namespace));

  assert.deepEqual(recorder.rejects, ["Held. See the link in this message to release it"]);
});

/// KV holds the only copy. A put that did not land and a sender told the
/// message is waiting for them is the one combination that loses mail without
/// anyone noticing, so the session is refused and the bytes stay with the
/// sending MTA.
test("a hold that could not be stored refuses the session rather than promising to keep it", async () => {
  gatewayAnswers(200, { action: "hold", token: "tok-6", notice });
  const errors = collectedErrors();
  const store = heldStore("KV put failed");
  const { message, recorder } = inbound();

  await worker.email(message, environment(store.namespace));

  assert.deepEqual(recorder.rejects, [RETRY]);
  assert.deepEqual(recorder.replies, []);
  assert.deepEqual(errors, ["hold failed"]);
});

test("a hold with no token is refused as ours to fix, not the sender's", async () => {
  gatewayAnswers(200, { action: "hold", notice });
  const errors = collectedErrors();
  const store = heldStore();
  const { message, recorder } = inbound();

  await worker.email(message, environment(store.namespace));

  assert.deepEqual(recorder.rejects, [RETRY]);
  assert.deepEqual(recorder.replies, []);
  assert.equal(store.puts.length, 0);
  assert.deepEqual(errors, ["incoherent verdict"]);
});

test("a rejection carries the gateway's own words into the session", async () => {
  gatewayAnswers(200, { action: "reject", reason: "dangerous", bounce: "This looks like phishing" });
  const store = heldStore();
  const { message, recorder } = inbound();

  await worker.email(message, environment(store.namespace));

  assert.deepEqual(recorder.rejects, ["This looks like phishing"]);
});

test("a rejection with nothing to say still refuses the session", async () => {
  gatewayAnswers(200, { action: "reject" });
  const store = heldStore();
  const { message, recorder } = inbound();

  await worker.email(message, environment(store.namespace));

  assert.deepEqual(recorder.rejects, ["Not delivered."]);
});

test("an inbox the gateway answers 200 for but does not know is refused permanently", async () => {
  gatewayAnswers(200, { action: "reject", reason: "unknown_inbox" });
  const store = heldStore();
  const { message, recorder } = inbound();

  await worker.email(message, environment(store.namespace));

  assert.deepEqual(recorder.rejects, ["No such address at this domain"]);
});

// --- the release handler ---------------------------------------------------
//
// `fetch()` is the other side of the hold `email()` writes: a capability
// token and a shared secret, exchanged for the exact bytes that were kept.
// Driven here the same way as `email()` - real Request objects in, KV and
// Mailgun replaced with recorders - so what is asserted is which branch ran
// and what it did, never what a stand-in was told to say. No test body ever
// carries a real token, only stand-ins invented here, so nothing below risks
// putting the capability itself in a failure message.

const RELEASE_SECRET = "shh";
const RELEASE_TOKEN = "tok-release-1";
const RELEASE_TO = "reader@personal.example";

interface ReleaseCall {
  key: string;
  type: string | undefined;
}

/// A namespace narrowed to the two methods `fetch()` calls, the same way
/// `heldStore` narrows to the one `email()` calls. `deleteFailures` is how many
/// of the next delete attempts throw, and a mutable field on the returned handle
/// rather than a constructor option, because the handler retries a delete that
/// fails: telling a KV write that stumbles once apart from one that will never
/// land needs a stand-in that can fail a bounded number of times and then work.
function releaseStore(seed: Record<string, string> = {}, getFails?: string) {
  const store = new Map<string, ArrayBuffer>();
  for (const [key, value] of Object.entries(seed)) {
    store.set(key, new TextEncoder().encode(value).buffer as ArrayBuffer);
  }
  const getCalls: ReleaseCall[] = [];
  const deleteCalls: string[] = [];
  const state = { deleteFailures: 0 };
  const namespace = {
    async get(key: string, type?: string) {
      getCalls.push({ key, type });
      if (getFails) throw new Error(getFails);
      return store.get(key) ?? null;
    },
    async delete(key: string) {
      deleteCalls.push(key);
      if (state.deleteFailures > 0) {
        state.deleteFailures -= 1;
        throw new Error("KV delete failed");
      }
      store.delete(key);
    },
  } as unknown as KVNamespace;
  return { getCalls, deleteCalls, state, namespace };
}

/// `body: null` means "send no body at all", which a GET request requires -
/// Node's `Request` throws if a body is set on one. Every other case defaults
/// to a well-formed release request, so a test overrides only the one field
/// it means to break.
function releaseRequest(
  overrides: { method?: string; path?: string; secret?: string | null; body?: string | null } = {}
): Request {
  const headers = new Headers({ "content-type": "application/json" });
  if (overrides.secret !== null) headers.set("x-postage-secret", overrides.secret ?? RELEASE_SECRET);
  const init: RequestInit = { method: overrides.method ?? "POST", headers };
  if (overrides.body !== null) {
    init.body = overrides.body ?? JSON.stringify({ token: RELEASE_TOKEN, to: RELEASE_TO });
  }
  return new Request(`https://worker.test${overrides.path ?? "/release"}`, init);
}

interface MailgunCall {
  url: string;
  authorization: string | null;
  to: string;
  dkim: string;
  tracking: string;
  bytes: string;
}

function mailgunAnswers(status: number, body: unknown = {}): MailgunCall[] {
  const calls: MailgunCall[] = [];
  globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    const form = init?.body as unknown as FormData;
    const blob = form.get("message") as unknown as Blob;
    calls.push({
      url: String(input),
      authorization: new Headers(init?.headers).get("authorization"),
      to: String(form.get("to")),
      dkim: String(form.get("o:dkim")),
      tracking: String(form.get("o:tracking")),
      bytes: await blob.text(),
    });
    return new Response(JSON.stringify(body), { status });
  };
  return calls;
}

function mailgunFails(cause: unknown): void {
  globalThis.fetch = async () => {
    throw cause;
  };
}

test("a path other than /release answers not found, same as any unknown route", async () => {
  const store = releaseStore();
  const response = await worker.fetch(releaseRequest({ path: "/other" }), environment(store.namespace));

  assert.equal(response.status, 404);
  assert.equal(await response.text(), "Not found");
  assert.equal(store.getCalls.length, 0);
});

test("a GET to /release answers not found, only POST releases anything", async () => {
  const store = releaseStore();
  const response = await worker.fetch(
    releaseRequest({ method: "GET", body: null }),
    environment(store.namespace)
  );

  assert.equal(response.status, 404);
  assert.equal(await response.text(), "Not found");
});

/// The auth check beyond possession of the token: a token that is genuinely
/// held is not enough on its own, the caller also needs the secret this
/// worker and the gateway share. Seeding a real token and still getting
/// refused is what pins that the two checks are independent.
test("a valid token on its own is not enough without the shared secret", async () => {
  const store = releaseStore({ [RELEASE_TOKEN]: RAW_MESSAGE });
  const response = await worker.fetch(releaseRequest({ secret: "wrong" }), environment(store.namespace));

  assert.equal(response.status, 401);
  assert.equal(await response.text(), "Bad secret");
  assert.equal(store.getCalls.length, 0);
  assert.equal(store.deleteCalls.length, 0);
});

test("a request with no secret header at all is refused the same way as a wrong one", async () => {
  const store = releaseStore();
  const response = await worker.fetch(releaseRequest({ secret: null }), environment(store.namespace));

  assert.equal(response.status, 401);
  assert.equal(await response.text(), "Bad secret");
});

test("a body that is not JSON is refused as a bad request", async () => {
  const store = releaseStore();
  const response = await worker.fetch(releaseRequest({ body: "{not json" }), environment(store.namespace));

  assert.equal(response.status, 400);
  assert.equal(await response.text(), "Body must be JSON");
});

test("a request with no token at all is refused before KV is touched", async () => {
  const store = releaseStore();
  const response = await worker.fetch(
    releaseRequest({ body: JSON.stringify({ to: RELEASE_TO }) }),
    environment(store.namespace)
  );

  assert.equal(response.status, 400);
  assert.equal(await response.text(), "token and to are required");
  assert.equal(store.getCalls.length, 0);
});

test("a request with no destination is refused before KV is touched", async () => {
  const store = releaseStore();
  const response = await worker.fetch(
    releaseRequest({ body: JSON.stringify({ token: RELEASE_TOKEN }) }),
    environment(store.namespace)
  );

  assert.equal(response.status, 400);
  assert.equal(await response.text(), "token and to are required");
  assert.equal(store.getCalls.length, 0);
});

test("an empty-string token is treated as no token at all", async () => {
  const store = releaseStore();
  const response = await worker.fetch(
    releaseRequest({ body: JSON.stringify({ token: "", to: RELEASE_TO }) }),
    environment(store.namespace)
  );

  assert.equal(response.status, 400);
  assert.equal(await response.text(), "token and to are required");
});

test("an unknown token finds nothing held", async () => {
  const store = releaseStore();
  const response = await worker.fetch(releaseRequest(), environment(store.namespace));

  assert.equal(response.status, 404);
  assert.equal(await response.text(), "Nothing is held under that token");
  assert.deepEqual(store.getCalls, [{ key: RELEASE_TOKEN, type: "arrayBuffer" }]);
});

/// The lookup is a straight KV read on whatever string arrives - there is no
/// separate notion of a malformed token, only found or not found. Pinned so a
/// later change that starts validating shape is a visible one.
test("a malformed-looking token is looked up the same as any other, and found just as absent", async () => {
  const store = releaseStore();
  const response = await worker.fetch(
    releaseRequest({ body: JSON.stringify({ token: "' OR 1=1 --", to: RELEASE_TO }) }),
    environment(store.namespace)
  );

  assert.equal(response.status, 404);
  assert.equal(await response.text(), "Nothing is held under that token");
});

/// The read used to be the one call in this handler that no try/catch wrapped,
/// so a KV outage escaped as a rejected promise - an unhandled worker exception
/// where every other failure here produces a response. Nothing has been sent by
/// this point and the hold is untouched, so asking again is free, and the answer
/// is the one the rest of Postage gives for a fault of its own: temporary, retry.
test("a KV read failure answers that we are temporarily unavailable, rather than escaping", async () => {
  const store = releaseStore({}, "KV get failed");
  const calls = mailgunAnswers(200);
  const errors = collectedErrors();

  const response = await worker.fetch(releaseRequest(), environment(store.namespace));

  assert.equal(response.status, 503);
  assert.equal(await response.text(), RETRY);
  assert.equal(calls.length, 0, "nothing was sent, so retrying costs nothing");
  assert.deepEqual(errors, ["release lookup failed"]);
});

test("a valid token releases the message to the address the request names", async () => {
  const store = releaseStore({ [RELEASE_TOKEN]: RAW_MESSAGE });
  const calls = mailgunAnswers(200);

  const response = await worker.fetch(releaseRequest(), environment(store.namespace));

  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { sent: true });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, "https://api.mailgun.test/v3/usepostage.com/messages.mime");
  assert.equal(calls[0].to, RELEASE_TO);
  assert.equal(calls[0].bytes, RAW_MESSAGE);
  assert.deepEqual(store.deleteCalls, [RELEASE_TOKEN]);
});

test("a release goes out unsigned and untracked, so the sender's own signature still covers it", async () => {
  const store = releaseStore({ [RELEASE_TOKEN]: RAW_MESSAGE });
  const calls = mailgunAnswers(200);

  await worker.fetch(releaseRequest(), environment(store.namespace));

  assert.equal(calls[0].dkim, "no");
  assert.equal(calls[0].tracking, "no");
  assert.match(calls[0].authorization ?? "", /^Basic /);
});

test("a token that already released once finds nothing held on the second try", async () => {
  const store = releaseStore({ [RELEASE_TOKEN]: RAW_MESSAGE });
  mailgunAnswers(200);

  const first = await worker.fetch(releaseRequest(), environment(store.namespace));
  const second = await worker.fetch(releaseRequest(), environment(store.namespace));

  assert.equal(first.status, 200);
  assert.equal(second.status, 404);
  assert.equal(await second.text(), "Nothing is held under that token");
});

test("a delivery failure answers 502 with fixed wording, not Mailgun's own message, and the token is not spent", async () => {
  const store = releaseStore({ [RELEASE_TOKEN]: RAW_MESSAGE });
  mailgunAnswers(502, "boom from mailgun");
  const errors = collectedErrors();

  const response = await worker.fetch(releaseRequest(), environment(store.namespace));

  assert.equal(response.status, 502);
  assert.equal(await response.text(), "Could not send it");
  assert.equal(store.deleteCalls.length, 0);
  assert.deepEqual(errors, ["release delivery failed"]);
});

test("a delivery that fails before Mailgun answers still gets a response, not a crash, and not the raw network error", async () => {
  const store = releaseStore({ [RELEASE_TOKEN]: RAW_MESSAGE });
  mailgunFails(new Error("connect ECONNREFUSED"));
  const errors = collectedErrors();

  const response = await worker.fetch(releaseRequest(), environment(store.namespace));

  assert.equal(response.status, 502);
  assert.equal(await response.text(), "Could not send it");
  assert.equal(store.deleteCalls.length, 0);
  assert.deepEqual(errors, ["release delivery failed"]);
});

test("a delivery failure whose cause is not an Error still answers with something readable", async () => {
  const store = releaseStore({ [RELEASE_TOKEN]: RAW_MESSAGE });
  mailgunFails("boom");
  const errors = collectedErrors();

  const response = await worker.fetch(releaseRequest(), environment(store.namespace));

  assert.equal(response.status, 502);
  assert.equal(await response.text(), "Could not send it");
  assert.deepEqual(errors, ["release delivery failed"]);
});

/// The double release this handler used to allow, now closed. `deliverUntouched`
/// has already sent the message by the time the key is removed, and an unguarded
/// delete that threw rejected the whole promise: the caller was told nothing, the
/// key was never removed, and the same token released the same message again on
/// the next request. Retrying retires the token, and answering `sent: true` is
/// what stops the caller asking a second time for something that already
/// happened.
test("a delete that fails once is retried, so a spent token cannot release the same message twice", async () => {
  const store = releaseStore({ [RELEASE_TOKEN]: RAW_MESSAGE });
  store.state.deleteFailures = 1;
  const calls = mailgunAnswers(200);

  const first = await worker.fetch(releaseRequest(), environment(store.namespace));

  assert.equal(first.status, 200);
  assert.deepEqual(await first.json(), { sent: true });
  assert.equal(store.deleteCalls.length, 2, "the delete that failed was tried again");

  const second = await worker.fetch(releaseRequest(), environment(store.namespace));

  assert.equal(second.status, 404);
  assert.equal(await second.text(), "Nothing is held under that token");
  assert.equal(calls.length, 1, "the held message went out exactly once");
});

/// The residual this design accepts. Every attempt fails, so the key outlives its
/// own release and nothing in a stateless handler can record that it was spent -
/// the only durable record of "spent" is the write that will not land. What is
/// still owed is an answer, and the answer is success: the mail is already out,
/// saying anything else invites a retry of a request that worked and sends the
/// message twice. The key expires on the deadline set when it was held, so a
/// hold that outlives its release still does not outlive the day.
test("a delete that never succeeds still answers that the message was sent, rather than crashing", async () => {
  const store = releaseStore({ [RELEASE_TOKEN]: RAW_MESSAGE });
  store.state.deleteFailures = Number.MAX_SAFE_INTEGER;
  const calls = mailgunAnswers(200);
  const errors = collectedErrors();

  const response = await worker.fetch(releaseRequest(), environment(store.namespace));

  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { sent: true });
  assert.equal(calls.length, 1);
  assert.deepEqual(errors, ["hold not retired after release"]);
});

/// The token is the capability: whoever holds it can release the message, so a
/// failure on this path may name what went wrong and never what it went wrong
/// on. The one log this handler writes is the one worth checking.
test("a release that could not retire its hold keeps the token out of the log", async () => {
  const store = releaseStore({ [RELEASE_TOKEN]: RAW_MESSAGE });
  store.state.deleteFailures = Number.MAX_SAFE_INTEGER;
  mailgunAnswers(200);
  const written: string[] = [];
  console.error = (...args: unknown[]) => {
    written.push(args.map((argument) => JSON.stringify(argument)).join(" "));
  };

  await worker.fetch(releaseRequest(), environment(store.namespace));

  assert.equal(written.length, 1);
  assert.ok(!written[0].includes(RELEASE_TOKEN), "the token is not written down");
});

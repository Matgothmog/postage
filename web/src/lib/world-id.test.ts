import assert from "node:assert/strict";
import { after, afterEach, test } from "node:test";
import { IDKitErrorCodes } from "@worldcoin/idkit";
import type { IDKitResult } from "@worldcoin/idkit";
import { pollTimeoutMs } from "./rp-context";
import {
  RpContextError,
  describeWorldIdFailure,
  fetchRpContext,
  postWorldVerify,
  runSelfieCheck,
  verifyRequestBody,
  type RpContext,
  type SelfieCheckDeps,
  type SelfieCheckHandle,
} from "./world-id";

/// Matches `WELL_FORMED_PROOF` in `web/src/app/api/world/verify/route.test.ts`
/// — the exact v3.0 (Selfie Check) shape a real IDKit result carries.
const WELL_FORMED_PROOF: IDKitResult = {
  protocol_version: "3.0",
  nonce: "test-nonce",
  environment: "production",
  responses: [
    { identifier: "selfie", proof: "0xproof", merkle_root: "0xroot", nullifier: "world-nullifier" },
  ],
};

/// 300 seconds apart, matching `RP_CONTEXT_TTL_SECONDS` — so a test asserting
/// on the derived poll timeout is exercising the real relationship between
/// the two, not an arbitrary gap.
const RP_CONTEXT: RpContext = {
  rp_id: "app_test_rp_id",
  nonce: "ctx-nonce",
  created_at: 1_000,
  expires_at: 1_300,
  signature: "0xsig",
  action: "send-free",
};

const realFetch = globalThis.fetch;
let fetchHandler: (url: string, init?: RequestInit) => Response | Promise<Response> = () => {
  throw new Error("no fetch handler installed for this test");
};

globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
  const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
  return fetchHandler(url, init);
};

after(() => {
  globalThis.fetch = realFetch;
});

/// Mirrors `env.test.ts`'s handling of `IDENTITY_MODE`: save the real value
/// once, restore it after every test that touches
/// `NEXT_PUBLIC_WORLD_ENVIRONMENT`, so a test here can never leak its value
/// into an unrelated one.
const ORIGINAL_WORLD_ENVIRONMENT = process.env.NEXT_PUBLIC_WORLD_ENVIRONMENT;

afterEach(() => {
  if (ORIGINAL_WORLD_ENVIRONMENT === undefined) delete process.env.NEXT_PUBLIC_WORLD_ENVIRONMENT;
  else process.env.NEXT_PUBLIC_WORLD_ENVIRONMENT = ORIGINAL_WORLD_ENVIRONMENT;
});

test("verifyRequestBody omits proof entirely under mock mode", () => {
  assert.deepEqual(verifyRequestBody("tok"), { token: "tok" });
});

test("verifyRequestBody carries the proof through untouched under live mode", () => {
  assert.deepEqual(verifyRequestBody("tok", WELL_FORMED_PROOF), { token: "tok", proof: WELL_FORMED_PROOF });
});

test("postWorldVerify posts a bare token when no proof is given (mock path, unchanged)", async () => {
  let posted: unknown;
  fetchHandler = (url, init) => {
    assert.equal(url, "/api/world/verify");
    posted = JSON.parse(String(init?.body));
    return Response.json({ status: "cleared", delivered: true });
  };

  const result = await postWorldVerify("tok");

  assert.deepEqual(posted, { token: "tok" });
  assert.deepEqual(result, { delivered: true });
});

test("postWorldVerify posts a well-formed proof alongside the token under live mode", async () => {
  let posted: unknown;
  fetchHandler = (url, init) => {
    posted = JSON.parse(String(init?.body));
    return Response.json({ status: "cleared", delivered: false });
  };

  const result = await postWorldVerify("tok", WELL_FORMED_PROOF);

  assert.deepEqual(posted, { token: "tok", proof: WELL_FORMED_PROOF });
  assert.deepEqual(result, { delivered: false });
});

test("postWorldVerify throws the server's message when the status is not cleared", async () => {
  fetchHandler = () => Response.json({ status: "rejected", error: "World ID rejected the proof" });

  await assert.rejects(() => postWorldVerify("tok", WELL_FORMED_PROOF), /World ID rejected the proof/);
});

test("postWorldVerify fails with a plain message rather than throwing a raw parse error on a non-JSON body", async () => {
  fetchHandler = () => new Response("not json", { status: 200 });

  await assert.rejects(() => postWorldVerify("tok", WELL_FORMED_PROOF), /Verification failed/);
});

test("fetchRpContext posts the token to /api/world/context and returns the parsed rp_context", async () => {
  fetchHandler = (url, init) => {
    assert.equal(url, "/api/world/context");
    assert.equal(init?.method, "POST");
    assert.deepEqual(JSON.parse(String(init?.body)), { token: "tok" });
    return Response.json(RP_CONTEXT);
  };

  assert.deepEqual(await fetchRpContext("tok"), RP_CONTEXT);
});

test("fetchRpContext throws a generic message on a non-JSON error response rather than returning a broken context", async () => {
  fetchHandler = () => new Response("nope", { status: 500 });

  await assert.rejects(() => fetchRpContext("tok"), /Could not reach World ID/);
});

test("fetchRpContext relays the route's own refusal for a dangerous challenge, which does not invite a retry", async () => {
  fetchHandler = () => Response.json({ error: "This challenge cannot be verified" }, { status: 403 });

  await assert.rejects(() => fetchRpContext("tok"), /cannot be verified/i);
});

test("fetchRpContext relays the route's own rate-limit refusal, which does invite a retry", async () => {
  fetchHandler = () =>
    Response.json({ error: "Too many verification attempts. Wait a moment and try again." }, { status: 429 });

  await assert.rejects(() => fetchRpContext("tok"), /wait a moment/i);
});

test("fetchRpContext refuses a 200 response whose body is missing its timestamps rather than handing back a broken context", async () => {
  // The route itself never sends this — this is what a proxy, CDN
  // interstitial, or captive portal in front of it could send instead.
  fetchHandler = () => Response.json({ rp_id: "app_test_rp_id", nonce: "n", signature: "0xsig", action: "send-free" });

  await assert.rejects(() => fetchRpContext("tok"), /didn't understand/i);
});

test("fetchRpContext refuses a 200 response whose body isn't an rp_context at all", async () => {
  fetchHandler = () => Response.json({ ok: true });

  await assert.rejects(() => fetchRpContext("tok"), /didn't understand/i);
});

test("describeWorldIdFailure gives distinct copy for the sender stopping versus World saying no", () => {
  assert.notEqual(
    describeWorldIdFailure(IDKitErrorCodes.UserRejected),
    describeWorldIdFailure(IDKitErrorCodes.VerificationRejected)
  );
});

test("describeWorldIdFailure falls back to generic copy for an error code it does not special-case", () => {
  assert.match(describeWorldIdFailure(IDKitErrorCodes.GenericError), /verification failed/i);
});

const SIGNAL = "challenge-token";

function fakeDeps(overrides: Partial<SelfieCheckDeps> = {}): SelfieCheckDeps {
  return {
    appId: "app_test",
    signal: SIGNAL,
    fetchRpContext: async () => RP_CONTEXT,
    openSelfieCheck: async () => ({
      connectorURI: "https://worldcoin.org/verify/abc",
      pollUntilCompletion: async () => ({ success: true, result: WELL_FORMED_PROOF }),
    }),
    ...overrides,
  };
}

test("runSelfieCheck reports a config error and never touches the network without an app id", async () => {
  let fetchCalled = false;
  const outcome = await runSelfieCheck(
    fakeDeps({
      appId: undefined,
      fetchRpContext: async () => {
        fetchCalled = true;
        return RP_CONTEXT;
      },
    })
  );

  assert.deepEqual(outcome, { ok: false, message: "World ID isn't configured yet. Pay instead, or try again shortly." });
  assert.equal(fetchCalled, false);
});

test("runSelfieCheck fetches the rp_context using the same token it holds as the IDKit signal", async () => {
  let tokenSeen: string | undefined;
  await runSelfieCheck(
    fakeDeps({
      fetchRpContext: async (token) => {
        tokenSeen = token;
        return RP_CONTEXT;
      },
    })
  );

  assert.equal(tokenSeen, SIGNAL);
});

test("runSelfieCheck surfaces a clear message when the World ID context endpoint is unreachable", async () => {
  const outcome = await runSelfieCheck(
    fakeDeps({ fetchRpContext: async () => { throw new Error("network down"); } })
  );

  assert.deepEqual(outcome, {
    ok: false,
    message: "Could not reach World ID. Check your connection and try again.",
  });
});

test("runSelfieCheck surfaces the context endpoint's own refusal message, not a generic one, for a settled challenge", async () => {
  const outcome = await runSelfieCheck(
    fakeDeps({
      fetchRpContext: async () => {
        throw new RpContextError("This challenge has already been answered", 409);
      },
    })
  );

  assert.deepEqual(outcome, { ok: false, message: "This challenge has already been answered" });
});

test("runSelfieCheck surfaces the context endpoint's own refusal message for a dangerous challenge too", async () => {
  const outcome = await runSelfieCheck(
    fakeDeps({
      fetchRpContext: async () => {
        throw new RpContextError("This challenge cannot be verified", 403);
      },
    })
  );

  assert.deepEqual(outcome, { ok: false, message: "This challenge cannot be verified" });
});

test("runSelfieCheck reports IDKit's own rejection when the request cannot even be opened", async () => {
  const outcome = await runSelfieCheck(
    fakeDeps({ openSelfieCheck: async () => { throw new Error("malformed_request"); } })
  );

  assert.deepEqual(outcome, { ok: false, message: "Could not start World ID verification. Try again." });
});

test("runSelfieCheck reports the sender dismissing World App as a client-side failure, not a crash", async () => {
  const handle: SelfieCheckHandle = {
    connectorURI: "https://worldcoin.org/verify/abc",
    pollUntilCompletion: async () => ({ success: false, error: IDKitErrorCodes.UserRejected }),
  };
  let announcedUri: string | undefined;
  const outcome = await runSelfieCheck(
    fakeDeps({ openSelfieCheck: async () => handle, onConnectorReady: (uri) => (announcedUri = uri) })
  );

  assert.deepEqual(outcome, { ok: false, message: describeWorldIdFailure(IDKitErrorCodes.UserRejected) });
  // The link must have been shown before the sender had a chance to dismiss it.
  assert.equal(announcedUri, handle.connectorURI);
});

test("runSelfieCheck resolves the real IDKit proof on success", async () => {
  const outcome = await runSelfieCheck(fakeDeps());

  assert.deepEqual(outcome, { ok: true, proof: WELL_FORMED_PROOF });
});

test("runSelfieCheck hands the challenge token to IDKit as the signal", async () => {
  let signalSeen: string | undefined;
  await runSelfieCheck(
    fakeDeps({
      openSelfieCheck: async (_config, signal) => {
        signalSeen = signal;
        return {
          connectorURI: "https://worldcoin.org/verify/abc",
          pollUntilCompletion: async () => ({ success: true, result: WELL_FORMED_PROOF }),
        };
      },
    })
  );

  assert.equal(signalSeen, SIGNAL);
});

test("runSelfieCheck takes the action from the signed rp_context, not a caller-supplied value", async () => {
  let actionSeen: unknown;
  await runSelfieCheck(
    fakeDeps({
      fetchRpContext: async () => ({ ...RP_CONTEXT, action: "context-supplied-action" }),
      openSelfieCheck: async (config) => {
        actionSeen = config.action;
        return {
          connectorURI: "https://worldcoin.org/verify/abc",
          pollUntilCompletion: async () => ({ success: true, result: WELL_FORMED_PROOF }),
        };
      },
    })
  );

  assert.equal(actionSeen, "context-supplied-action");
});

test("runSelfieCheck derives the poll timeout from the signed rp_context's own window, not the SDK default", async () => {
  let timeoutSeen: number | undefined;
  await runSelfieCheck(
    fakeDeps({
      openSelfieCheck: async () => ({
        connectorURI: "https://worldcoin.org/verify/abc",
        pollUntilCompletion: async (options) => {
          timeoutSeen = options?.timeout;
          return { success: true, result: WELL_FORMED_PROOF };
        },
      }),
    })
  );

  assert.equal(timeoutSeen, pollTimeoutMs(RP_CONTEXT));
});

test("runSelfieCheck refuses a malformed rp_context from the real endpoint before ever opening the World App", async () => {
  fetchHandler = () => Response.json({ rp_id: "app_test_rp_id" }); // missing nonce, signature, action, timestamps

  let opened = false;
  const outcome = await runSelfieCheck(
    fakeDeps({
      fetchRpContext,
      openSelfieCheck: async () => {
        opened = true;
        throw new Error("should never be reached");
      },
    })
  );

  assert.equal(outcome.ok, false);
  assert.equal(opened, false);
});

test("runSelfieCheck refuses to open an unbound check rather than producing a proof with no signal", async () => {
  let opened = false;
  const outcome = await runSelfieCheck(
    fakeDeps({
      signal: undefined,
      openSelfieCheck: async () => {
        opened = true;
        throw new Error("should never be reached");
      },
    })
  );

  assert.equal(outcome.ok, false);
  // The point is that no proof exists to be replayed, not merely that the
  // server would have refused one.
  assert.equal(opened, false);
});

async function environmentSeenByIDKit(): Promise<unknown> {
  let environmentSeen: unknown;
  await runSelfieCheck(
    fakeDeps({
      openSelfieCheck: async (config) => {
        environmentSeen = config.environment;
        return {
          connectorURI: "https://worldcoin.org/verify/abc",
          pollUntilCompletion: async () => ({ success: true, result: WELL_FORMED_PROOF }),
        };
      },
    })
  );
  return environmentSeen;
}

test("runSelfieCheck targets production when NEXT_PUBLIC_WORLD_ENVIRONMENT is unset", async () => {
  delete process.env.NEXT_PUBLIC_WORLD_ENVIRONMENT;
  assert.equal(await environmentSeenByIDKit(), "production");
});

test("runSelfieCheck targets production when NEXT_PUBLIC_WORLD_ENVIRONMENT is the empty string", async () => {
  process.env.NEXT_PUBLIC_WORLD_ENVIRONMENT = "";
  assert.equal(await environmentSeenByIDKit(), "production");
});

test("runSelfieCheck targets World's sandbox when NEXT_PUBLIC_WORLD_ENVIRONMENT is set to sandbox", async () => {
  process.env.NEXT_PUBLIC_WORLD_ENVIRONMENT = "sandbox";
  assert.equal(await environmentSeenByIDKit(), "sandbox");
});

test("runSelfieCheck reports a config error, and never opens World App, on an unrecognised NEXT_PUBLIC_WORLD_ENVIRONMENT rather than silently falling back to production", async () => {
  process.env.NEXT_PUBLIC_WORLD_ENVIRONMENT = "prod";
  let opened = false;
  const outcome = await runSelfieCheck(
    fakeDeps({
      openSelfieCheck: async () => {
        opened = true;
        throw new Error("should never be reached");
      },
    })
  );

  assert.deepEqual(outcome, { ok: false, message: "World ID isn't configured yet. Pay instead, or try again shortly." });
  assert.equal(opened, false);
});

import assert from "node:assert/strict";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { after, beforeEach, test } from "node:test";

// No database is touched anywhere in cloudflare.ts, so this file needs neither
// a temp workspace nor a DATABASE_URL — only the two Cloudflare credentials
// `required()` demands and a stub standing in for the Cloudflare API itself.
process.env.CLOUDFLARE_ACCOUNT_ID = "test-account";
process.env.CLOUDFLARE_API_TOKEN = "test-token";

const PER_PAGE = 50;

function address(id: string, email: string, verified: string | null = null) {
  return { id, email, verified };
}

/// A full page of addresses that is not the account, with an optional match
/// planted at a given index — enough to prove the walk looked at every row of
/// the page it landed on, not just the first.
function fullPage(page: number, match?: { email: string; index: number }) {
  return Array.from({ length: PER_PAGE }, (_, index) => {
    if (match && index === match.index) return address(`p${page}-${index}`, match.email, null);
    return address(`p${page}-${index}`, `nobody-${page}-${index}@example.com`);
  });
}

let handleRequest: (url: URL) => { status: number; body: unknown } = () => {
  throw new Error("test did not install a handler for this request");
};

// Set up before importing `./cloudflare` and at the top level rather than
// inside a `before()` hook: on this runner, a `before()` that resolves through
// an I/O callback (here, a listening socket) races the file's `beforeEach` and
// loses — later hooks and tests observe the pre-override `fetch`, silently
// reaching the real Cloudflare API. Top-level await has no such race, and it is
// the same pattern `deliver/route.test.ts` already uses for the same reason.
const server = createServer((request, response) => {
  const url = new URL(request.url ?? "/", "http://127.0.0.1");
  const { status, body } = handleRequest(url);
  response.writeHead(status, { "Content-Type": "application/json" }).end(JSON.stringify(body));
});
await new Promise<void>((ready) => server.listen(0, "127.0.0.1", ready));
const { port } = server.address() as AddressInfo;
const base = `http://127.0.0.1:${port}`;

const realFetch = globalThis.fetch;
globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
  const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
  const rebased = url.replace("https://api.cloudflare.com/client/v4", base);
  return realFetch(rebased, init);
}) as typeof fetch;

const { destinationStatus, ensureDestination } = await import("./cloudflare");

beforeEach(() => {
  handleRequest = () => {
    throw new Error("test did not install a handler for this request");
  };
});

after(() => {
  globalThis.fetch = realFetch;
  server.close();
});

/// `findDestination` is not exported; `ensureDestination` only reaches it when
/// the create call is refused as a duplicate, which is the one door the module
/// gives it. Driving pagination through that door tests the real caller's path
/// instead of an implementation detail.
async function ensureViaDuplicate(
  email: string,
  listPage: (url: URL) => { status: number; body: unknown }
) {
  handleRequest = (url) => {
    if (url.pathname.endsWith("/addresses") && !url.searchParams.has("page")) {
      return {
        status: 409,
        body: {
          success: false,
          errors: [{ code: 1, message: "address already exists" }],
          result: null,
        },
      };
    }
    return listPage(url);
  };
  return ensureDestination(email);
}

test("a duplicate address findable on the first page is found without walking further", async () => {
  let pagesRequested = 0;
  const found = await ensureViaDuplicate("found@example.com", (url) => {
    pagesRequested += 1;
    assert.equal(url.searchParams.get("page"), "1", "must start the walk on page one");
    return {
      status: 200,
      body: {
        success: true,
        errors: [],
        result: fullPage(1, { email: "found@example.com", index: 10 }),
        result_info: { page: 1, per_page: PER_PAGE, total_count: 200 },
      },
    };
  });

  assert.equal(found.id, "p1-10");
  assert.equal(pagesRequested, 1, "a first-page match must not trigger a second request");
});

/// The regression the module's own comment names: reading only page one
/// stopped finding addresses at roughly the fiftieth signup. A destination
/// that shows up on neither the first nor the last page must still be found.
test("a duplicate address on a later page is found by walking past page one", async () => {
  const wanted = "later-signup@example.com";
  const found = await ensureViaDuplicate(wanted, (url) => {
    const page = Number(url.searchParams.get("page"));
    if (page > 3) throw new Error(`must stop once page ${page - 1} held the match`);
    const match = page === 3 ? { email: wanted, index: 5 } : undefined;
    return {
      status: 200,
      body: {
        success: true,
        errors: [],
        result: fullPage(page, match),
        result_info: { page, per_page: PER_PAGE, total_count: 200 },
      },
    };
  });

  assert.equal(found.id, "p3-5", "a match on page three of five must be found, not missed");
});

/// The negative case the pagination guards against: an address that truly is
/// not registered must come back as a clean failure once the account's own
/// page count says there is nothing left to check, not loop forever.
test("a duplicate address that cannot be found anywhere fails instead of hanging", async () => {
  let pagesRequested = 0;
  const totalPages = 4;
  await assert.rejects(
    () =>
      ensureViaDuplicate("nobody-registered@example.com", (url) => {
        pagesRequested += 1;
        const page = Number(url.searchParams.get("page"));
        const isLast = page === totalPages;
        return {
          status: 200,
          body: {
            success: true,
            errors: [],
            result: fullPage(page).slice(0, isLast ? 10 : PER_PAGE),
            result_info: { page, per_page: PER_PAGE, total_count: (totalPages - 1) * PER_PAGE + 10 },
          },
        };
      }),
    /already exists/,
    "a genuine miss must surface as the create refusal, not a silent success"
  );
  assert.equal(pagesRequested, totalPages, "the walk must cover every page the account reports");
});

test("a zero-date verified field becomes null rather than an unparseable date", async () => {
  handleRequest = () => ({
    status: 200,
    body: { success: true, errors: [], result: address("addr-1", "x@example.com", null) },
  });

  const status = await destinationStatus("addr-1");
  assert.equal(status?.verifiedAt, null);
});

test("a real verified timestamp survives as seconds since epoch", async () => {
  handleRequest = () => ({
    status: 200,
    body: {
      success: true,
      errors: [],
      result: address("addr-2", "x@example.com", "2026-01-01T00:00:00Z"),
    },
  });

  const status = await destinationStatus("addr-2");
  assert.equal(status?.verifiedAt, Math.floor(Date.parse("2026-01-01T00:00:00Z") / 1000));
});

test("an unparseable verified value never becomes a NaN bind", async () => {
  handleRequest = () => ({
    status: 200,
    body: { success: true, errors: [], result: address("addr-3", "x@example.com", "not-a-date") },
  });

  const status = await destinationStatus("addr-3");
  assert.equal(status?.verifiedAt, null, "an unparseable date must resolve to null, never NaN");
});

test("ensureDestination on a fresh address uses the create response directly", async () => {
  handleRequest = (url) => {
    assert.ok(url.pathname.endsWith("/addresses"));
    return {
      status: 200,
      body: { success: true, errors: [], result: address("new-addr", "fresh@example.com", null) },
    };
  };

  const result = await ensureDestination("fresh@example.com");
  assert.equal(result.id, "new-addr");
});

test("a non-duplicate refusal is never read as not-verified-yet", async () => {
  handleRequest = () => ({
    status: 500,
    body: { success: false, errors: [{ code: 999, message: "internal error" }], result: null },
  });

  await assert.rejects(
    () => destinationStatus("addr-x"),
    "an outage or a revoked token must surface as a failure, not as data"
  );
});

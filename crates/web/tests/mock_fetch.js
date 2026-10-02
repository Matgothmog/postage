// A scripted stand-in for `globalThis.fetch`, recording every request so the
// tests can check what the app sent. No network: a request nobody scripted is
// recorded as `unmatched` and fails like an offline `fetch`.
//
// A route is `{method, url, bodyIncludes?, responses: [...]}`. Matching is on
// the method, the exact URL (path and query, or the absolute RPC URL) and,
// when given, a substring of the body. Each match consumes the next response;
// the last one repeats. A response is `{status?, body?}` plus optionally
// `throw: "message"` (a network failure), `gate: "name"` (held until
// `openGate("name")`) or `delayMs`.

const realFetch = globalThis.fetch;
let routes = [];
let log = [];
const gates = new Map();

export function installFetch(routesJson) {
  setRoutes(routesJson);
  log = [];
  gates.clear();
  globalThis.fetch = mockFetch;
}

export function restoreFetch() {
  globalThis.fetch = realFetch;
}

/// Replaces the scripted routes, keeping the log.
export function setRoutes(routesJson) {
  routes = JSON.parse(routesJson).map((route) => ({ ...route, index: 0 }));
}

export function fetchLog() {
  return JSON.stringify(log);
}

export function openGate(name) {
  const gate = gates.get(name) ?? makeGate();
  gates.set(name, gate);
  gate.open();
}

function makeGate() {
  let open;
  const opened = new Promise((resolve) => {
    open = resolve;
  });
  return { opened, open };
}

function abortError() {
  return new DOMException("The operation was aborted.", "AbortError");
}

/// Resolves when `promise` does, or rejects if `signal` aborts first.
function abortable(promise, signal) {
  if (!signal) return promise;
  if (signal.aborted) return Promise.reject(abortError());
  return new Promise((resolve, reject) => {
    signal.addEventListener("abort", () => reject(abortError()), { once: true });
    promise.then(resolve, reject);
  });
}

async function mockFetch(input, init = {}) {
  const url = String(input);
  const method = (init.method ?? "GET").toUpperCase();
  const entry = { method, url, headers: init.headers ?? {}, body: init.body ?? null };
  log.push(entry);

  const route = routes.find(
    (candidate) =>
      candidate.method === method &&
      candidate.url === url &&
      (!candidate.bodyIncludes || String(entry.body ?? "").includes(candidate.bodyIncludes))
  );
  if (!route) {
    entry.unmatched = true;
    throw new TypeError(`mock fetch: no route for ${method} ${url}`);
  }
  const step = route.responses[Math.min(route.index, route.responses.length - 1)];
  route.index += 1;

  if (step.throw) throw new TypeError(step.throw);
  if (step.gate) {
    const gate = gates.get(step.gate) ?? makeGate();
    gates.set(step.gate, gate);
    await abortable(gate.opened, init.signal);
  }
  if (step.delayMs) {
    await abortable(new Promise((resolve) => setTimeout(resolve, step.delayMs)), init.signal);
  }
  if (init.signal?.aborted) throw abortError();
  return new Response(step.body ?? "", {
    status: step.status ?? 200,
    headers: { "Content-Type": "application/json" },
  });
}

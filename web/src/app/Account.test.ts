import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import ts from "typescript";

/// Account.tsx cannot be imported here. `node --experimental-strip-types`
/// recognizes only `.ts`/`.mts`/`.cts` and throws `ERR_UNKNOWN_FILE_EXTENSION`
/// on `.tsx` before `test/resolve-ts.mjs` (which only ever appends `.ts`) gets
/// a say - confirmed by hand, not inferred, with a standalone probe file
/// outside this repo. There is also no jsdom or React renderer installed to
/// mount the component even if the module did load, and no browser tool in
/// this environment either.
///
/// So the two fixes below are pinned by reading Account.tsx's own source at
/// test time, cutting the exact block each one lives in out by matching
/// braces, stripping its TypeScript with the compiler already installed for
/// `tsc`, and running that - not a hand-copy of it - as a real function. A
/// change to either block changes what runs here; an anchor that stops
/// matching throws rather than letting the test quietly stop meaning
/// anything.
const ACCOUNT_PATH = fileURLToPath(new URL("./Account.tsx", import.meta.url));
const ACCOUNT_SOURCE = readFileSync(ACCOUNT_PATH, "utf8");

function matchingBrace(source: string, openIndex: number): number {
  let depth = 0;
  for (let i = openIndex; i < source.length; i++) {
    if (source[i] === "{") depth++;
    else if (source[i] === "}") {
      depth--;
      if (depth === 0) return i;
    }
  }
  throw new Error("no matching closing brace found");
}

/// `anchor` must end in the block's opening `{`. Throws rather than returning
/// nothing if Account.tsx no longer contains it, so a rewritten fix fails
/// this suite instead of leaving it testing nothing.
function extractBraceBlock(source: string, anchor: string): { text: string; endIndex: number } {
  const start = source.indexOf(anchor);
  assert.notEqual(start, -1, `anchor not found in Account.tsx: ${JSON.stringify(anchor)}`);
  const openBrace = start + anchor.length - 1;
  const closeBrace = matchingBrace(source, openBrace);
  return { text: source.slice(start, closeBrace + 1), endIndex: closeBrace };
}

function stripTypes(source: string): string {
  return ts.transpile(source, { target: ts.ScriptTarget.ES2020 });
}

// --- Fix 2: 400 and 401 both mean the proof was refused --------------------

const refusedTheProofBlock = extractBraceBlock(
  ACCOUNT_SOURCE,
  "function refusedTheProof(status: number): boolean {"
);
const refusedTheProof = new Function(
  `"use strict";\n${stripTypes(refusedTheProofBlock.text)}\nreturn refusedTheProof;`
)() as (status: number) => boolean;

// --- Fix 1: what refresh() does with one server answer ---------------------

const ifBlock = extractBraceBlock(ACCOUNT_SOURCE, "if (response.ok) {");
const afterIf = ACCOUNT_SOURCE.slice(ifBlock.endIndex + 1);
const elseOpen = afterIf.match(/^\s*else\s*\{/);
assert.ok(elseOpen, "expected an else immediately after the if (response.ok) block in Account.tsx");
const elseCloseIndex = matchingBrace(ACCOUNT_SOURCE, ifBlock.endIndex + 1 + elseOpen![0].length - 1);
const decisionSource = ACCOUNT_SOURCE.slice(
  ACCOUNT_SOURCE.indexOf("if (response.ok) {"),
  elseCloseIndex + 1
);

type ApplyResponse = (
  response: { ok: boolean; status: number; json(): Promise<{ inbox: unknown }> },
  setInbox: (value: unknown) => void,
  setError: (value: string | null) => void,
  refusedTheProof: (status: number) => boolean
) => Promise<void>;

const wrappedDecision = `return async function applyResponse(response, setInbox, setError, refusedTheProof) {\n${decisionSource}\n};`;
const applyResponse = new Function(`"use strict";\n${stripTypes(wrappedDecision)}`)() as unknown as ApplyResponse;

test("refusedTheProof: only 400 and 401 count as a refused proof (fix 2)", () => {
  assert.equal(refusedTheProof(400), true);
  assert.equal(refusedTheProof(401), true);
  assert.equal(refusedTheProof(200), false);
  assert.equal(refusedTheProof(403), false);
  assert.equal(refusedTheProof(500), false);
  assert.equal(refusedTheProof(502), false);
});

test("a 400 (expired identity token, no wallet header) is reported as unauthorized, not network (fix 2)", async () => {
  const calls = { inbox: [] as unknown[], error: [] as (string | null)[] };
  const response = { ok: false, status: 400, json: async () => ({ inbox: null }) };
  await applyResponse(
    response,
    (v) => calls.inbox.push(v),
    (v) => calls.error.push(v),
    refusedTheProof
  );
  assert.deepEqual(calls.error, ["unauthorized"]);
  assert.deepEqual(calls.inbox, [], "a refused proof must not touch inbox");
});

test("an unrecognized failure (502) is still reported as network, not unauthorized (fix 2)", async () => {
  const calls = { inbox: [] as unknown[], error: [] as (string | null)[] };
  const response = { ok: false, status: 502, json: async () => ({ inbox: null }) };
  await applyResponse(
    response,
    (v) => calls.inbox.push(v),
    (v) => calls.error.push(v),
    refusedTheProof
  );
  assert.deepEqual(calls.error, ["network"]);
});

/// Mirrors the branch order Account.tsx's render picks between once `wallet`
/// is truthy: `inbox ? InboxPanel : loaded && error ? InboxError : loaded ?
/// ClaimInbox : Waiting`. Not executed from the file itself - JSX is out of
/// reach here the same way the rest of the module is - so its order is
/// pinned separately below, by checking the literal source still contains it
/// in this shape.
function pickPanel(state: {
  inbox: unknown;
  loaded: boolean;
  error: string | null;
}): "panel" | "error" | "claim" | "waiting" {
  if (state.inbox) return "panel";
  if (state.loaded && state.error) return "error";
  if (state.loaded) return "claim";
  return "waiting";
}

test("pickPanel mirrors Account.tsx's actual branch order (drift guard)", () => {
  assert.ok(
    ACCOUNT_SOURCE.includes(") : loaded && error ? (") && ACCOUNT_SOURCE.includes(") : loaded ? ("),
    "Account.tsx's render branch order changed - update pickPanel to match before trusting the test below"
  );
});

test("a successful retry with no inbox clears a stale error, so the claim flow stays reachable (fix 1)", async () => {
  // Reproduces the reviewer's stranding path. Step 1: on mount, the identity
  // token is not there yet, walletProof prompts for a signature, and the
  // user dismisses it. That rejection is caught by
  // `refresh().catch(() => setError("network"))` at the call site in
  // Account.tsx (the mount effect) - a one-line arrow with no branch to
  // regress, so it is applied directly here rather than extracted.
  let error: string | null = "network";
  let inbox: unknown = null;
  const loaded = true;

  // Step 2: the identity token has since arrived, and the retry succeeds -
  // this wallet has no inbox yet.
  const response = { ok: true, status: 200, json: async () => ({ inbox: null }) };
  await applyResponse(
    response,
    (v) => {
      inbox = v;
    },
    (v) => {
      error = v;
    },
    refusedTheProof
  );

  assert.equal(error, null, "a successful retry must clear the stale error, or the user is stranded");
  assert.equal(inbox, null);

  const panel = pickPanel({ inbox, loaded, error });
  assert.equal(panel, "claim", "the user must land on ClaimInbox, not be stuck on InboxError");
});

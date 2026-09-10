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
/// So the fixes below are pinned by reading Account.tsx's own source at test
/// time, cutting the exact block each one lives in out by matching braces,
/// stripping its TypeScript with the compiler already installed for `tsc`, and
/// running that - not a hand-copy of it - as a real function. A change to
/// either block changes what runs here; an anchor that stops matching throws
/// rather than letting the test quietly stop meaning anything.
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

// --- Which panel a signed-in page shows -------------------------------------

/// The order the render picks between its five panels, taken out of Account.tsx
/// itself rather than restated here. It used to be restated, guarded only by a
/// string match on the JSX; the JSX changed shape when the claim flow was merged
/// into the hero, and a guard that can only notice a rewrite is weaker than one
/// that runs the real thing.
const pickPanelBlock = extractBraceBlock(
  ACCOUNT_SOURCE,
  "function pickPanel(wallet: unknown, loaded: boolean, error: unknown, inbox: unknown, claim: unknown): Panel {"
);
type Panel = "waiting" | "error" | "inbox" | "strip" | "setup";
const pickPanel = new Function(
  `"use strict";\n${stripTypes(pickPanelBlock.text)}\nreturn pickPanel;`
)() as (
  wallet: unknown,
  loaded: boolean,
  error: unknown,
  inbox: unknown,
  claim: unknown
) => Panel;

const WALLET = { address: "0xWallet" };
const INBOX = { handle: "demo", destination: "demo@example.com" };
const CLAIM = { handle: "demo", destination: "demo@example.com" };

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

test("pickPanel keeps the precedence the render depends on, wallet and first load first", () => {
  assert.equal(pickPanel(null, false, null, null, null), "waiting");
  assert.equal(pickPanel(null, true, "network", INBOX, CLAIM), "waiting");
  assert.equal(pickPanel(WALLET, false, null, INBOX, CLAIM), "waiting");
  assert.equal(pickPanel(WALLET, true, "network", null, null), "error");
  assert.equal(
    pickPanel(WALLET, true, null, INBOX, CLAIM),
    "inbox",
    "a finished inbox beats a claim still on record for it"
  );
  assert.equal(pickPanel(WALLET, true, null, null, CLAIM), "strip");
  assert.equal(pickPanel(WALLET, true, null, null, null), "setup");
});

/// H1. `refresh()` re-runs every time its identity changes, which includes
/// every Privy identity-token rotation, and it never clears `inbox` on a
/// failure - it only sets `error`. Deciding the error first meant one blip
/// replaced a live inbox, whose rows were still sitting in state, with a
/// full-screen "Can't reach us." An error screen is the right answer only when
/// there is nothing behind it to show.
test("an inbox already read survives a failed refresh, error or not (H1)", () => {
  assert.equal(
    pickPanel(WALLET, true, "network", INBOX, null),
    "inbox",
    "a transient network error must not take a live inbox off the screen"
  );
  assert.equal(
    pickPanel(WALLET, true, "unauthorized", INBOX, null),
    "inbox",
    "a rotated identity token must not take a live inbox off the screen either"
  );
  assert.equal(
    pickPanel(WALLET, true, "network", INBOX, CLAIM),
    "inbox",
    "the inbox still wins over a claim still on record for it"
  );
});

test("a claim in progress is not hidden behind an unrelated network error (H1)", () => {
  assert.equal(
    pickPanel(WALLET, true, "network", null, CLAIM),
    "strip",
    "the claim the user is in the middle of outranks a failed account read"
  );
});

test("the error screen is still what a failed read with nothing behind it shows (H1)", () => {
  assert.equal(pickPanel(WALLET, true, "network", null, null), "error");
  assert.equal(pickPanel(WALLET, true, "unauthorized", null, null), "error");
});

/// M5. The send effect used to guard on `!loaded`, which only says the request
/// finished - not that it answered. A failed GET leaves `loaded` true and
/// `inbox` null, which reads exactly like a wallet with no inbox, and the claim
/// POST went out behind the error screen: one of the five a wallet gets in an
/// hour, spent where its own error could never be read. The gate is now the
/// panel itself, so "we do not know" can no longer pass for "no inbox".
test("a claim is never sent while the account read is the thing that failed (M5)", () => {
  assert.notEqual(
    pickPanel(WALLET, true, "network", null, null),
    "setup",
    "a failed read is not an empty account"
  );
  assert.notEqual(pickPanel(WALLET, true, "unauthorized", null, null), "setup");
  assert.notEqual(pickPanel(WALLET, false, null, null, null), "setup");
  assert.notEqual(pickPanel(null, true, null, null, null), "setup");
  assert.equal(
    pickPanel(WALLET, true, null, null, null),
    "setup",
    "an account read that answered with no inbox is still the one state a claim goes out from"
  );

  assert.match(
    ACCOUNT_SOURCE,
    /const panel = pickPanel\(wallet, loaded, inboxError, inbox, progress\);/,
    "the send gate must be the panel itself, not a second list of the same conditions"
  );
  assert.match(ACCOUNT_SOURCE, /sending\.current \|\| panel !== "setup"\) return;/);
});

/// H1's other half: the failure still has to be visible and still has to be
/// retryable. It just does not get to take the page over.
test("a refresh that failed over live data is a notice, not a replacement (H1)", () => {
  assert.match(
    ACCOUNT_SOURCE,
    /function RefreshFailed\(/,
    "expected a non-destructive notice for a failed refresh"
  );
  assert.match(ACCOUNT_SOURCE, /<RefreshFailed kind=\{error\} onRetry=\{retry\} \/>/);

  const hardScreens = ACCOUNT_SOURCE.match(/<InboxError /g) ?? [];
  assert.equal(hardScreens.length, 1, "the full-screen error belongs to the `error` panel alone");
});

/// H2. One browser has one slot, so a claim left in it is the claim the next
/// person to sign in on this machine is shown.
test("signing out forgets the claim this browser was holding (H2)", () => {
  const signOut = extractBraceBlock(ACCOUNT_SOURCE, "onClick={() => {").text;
  assert.match(signOut, /claim\.restart\(\)/, "sign-out must drop the pending claim");
  assert.match(signOut, /logout\(\)/);
});

test("the claim in hand is the one this wallet stored, and nobody else's (H2)", () => {
  assert.match(
    ACCOUNT_SOURCE,
    /const progress = claimFor\(record, wallet\);/,
    "what is shown must be scoped to the wallet that stored it"
  );
  assert.doesNotMatch(
    ACCOUNT_SOURCE,
    /readPendingClaim/,
    "the unscoped read is what handed one user another user's claim"
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

  const panel = pickPanel(WALLET, true, error, inbox, null);
  assert.equal(panel, "setup", "the user must land on the claim form, not be stuck on InboxError");
});

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { isValidHandle } from "@/lib/handle";
import { confirmStatement as confirmStatementDirect } from "@/lib/statements";
import { confirmStatement as confirmStatementReExported } from "@/lib/wallet-proof";

/// ClaimStrip.tsx cannot be imported here — same constraint documented in
/// Account.test.ts: `.tsx` is unrecognized by `node --experimental-strip-types`,
/// and there is no jsdom/React renderer or browser tool in this environment to
/// mount the component even if it loaded.
///
/// This file covers what used to live in PickHandle.test.ts and
/// FinishClaim.test.ts. Both components were merged into ClaimStrip.tsx — the
/// claim form as `ClaimSetup`, the confirmation screen as `ClaimStrip` — so the
/// two facts they pinned are pinned here, against the file the code moved to.
const SOURCE_PATH = fileURLToPath(new URL("./ClaimStrip.tsx", import.meta.url));
const SOURCE = readFileSync(SOURCE_PATH, "utf8");

/// The submit button's gate, read out of the real source and evaluated rather
/// than hand-copied. If `ready` is ever rewritten to stop calling
/// `isValidHandle` (reintroducing the bug where the button enabled for a handle
/// the server would reject), this file's regex still finds a `const ready = ...`
/// line, but the eval no longer matches the server-equivalent verdicts below and
/// the tests fail.
const readyLine = SOURCE.match(/const ready = (.+);/);
assert.ok(readyLine, "expected a `const ready = ...;` line in ClaimStrip.tsx");

// `ts.transpile` appends its own trailing `;`, which would otherwise close
// the `return (...)` wrapper early and leave a stray `);` behind it.
const readyExpression = ts.transpile(readyLine![1], { target: ts.ScriptTarget.ES2020 }).trim().replace(/;$/, "");
const ready = new Function(
  "isValidHandle",
  "name",
  "to",
  `"use strict";\nreturn (${readyExpression});`
) as (isValidHandleFn: (handle: string) => boolean, name: string, to: string) => boolean;

// Each case below mirrors a rejection branch `/api/inbox`'s `validate()`
// enforces server-side (see `web/src/lib/handle.test.ts` for the same
// boundaries proven against `isValidHandle` directly).

test("ready is false for a handle shorter than the server's minimum", () => {
  assert.equal(ready(isValidHandle, "a", "user@example.com"), false);
});

test("ready is false for a handle starting with punctuation, which the server's shape rule rejects", () => {
  assert.equal(ready(isValidHandle, "-demo", "user@example.com"), false);
});

test("ready is false for a handle with two dots running together, which the server rejects separately", () => {
  assert.equal(ready(isValidHandle, "demo..user", "user@example.com"), false);
});

test("ready is true for a handle and destination the server would accept", () => {
  assert.equal(ready(isValidHandle, "demo.user-1", "user@example.com"), true);
});

test("ready still requires an @ in the destination even once the handle is valid", () => {
  assert.equal(ready(isValidHandle, "demo", "not-an-email"), false);
});

/// Fix 3: importing `confirmStatement` from `@/lib/wallet-proof`, where the
/// header says both ends of the signed-header contract are meant to be read
/// from, rather than from `@/lib/statements` directly. Pinned two ways: that the
/// two names are the same function, and that the source actually reads it from
/// the one wallet-proof.ts names as the intended door.

test("wallet-proof re-exports the exact confirmStatement statements.ts defines (fix 3)", () => {
  assert.equal(
    confirmStatementReExported,
    confirmStatementDirect,
    "@/lib/wallet-proof must re-export the same confirmStatement, not a second copy"
  );
});

test("ClaimStrip.tsx imports confirmStatement from wallet-proof, not statements directly (fix 3)", () => {
  assert.match(
    SOURCE,
    /import\s*\{[^}]*\bconfirmStatement\b[^}]*\}\s*from\s*"@\/lib\/wallet-proof"/,
    "expected an import of confirmStatement from @/lib/wallet-proof"
  );
  assert.doesNotMatch(
    SOURCE,
    /import\s*\{[^}]*\bconfirmStatement\b[^}]*\}\s*from\s*"@\/lib\/statements"/,
    "confirmStatement must not be imported from @/lib/statements directly"
  );
});

// --- The way out, and the code field that used to burn attempts -------------

/// `readyToSubmit` is lifted out of the component for the same reason
/// `pickPanel` is in Account.tsx: it is the whole of a rule that cannot be
/// exercised in a browser here, so it is cut out of the real source, stripped
/// of its types and run.
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
/// nothing if ClaimStrip.tsx no longer contains it, so a rewritten fix fails
/// this suite instead of leaving it testing nothing.
function extractBraceBlock(source: string, anchor: string): string {
  const start = source.indexOf(anchor);
  assert.notEqual(start, -1, `anchor not found in ClaimStrip.tsx: ${JSON.stringify(anchor)}`);
  return source.slice(start, matchingBrace(source, start + anchor.length - 1) + 1);
}

const readyToSubmitBlock = extractBraceBlock(
  SOURCE,
  "function readyToSubmit(digits: string, busy: boolean, failed: boolean, lastSent: string | null): boolean {"
);
const readyToSubmit = new Function(
  `"use strict";\n${ts.transpile(readyToSubmitBlock, { target: ts.ScriptTarget.ES2020 })}\nreturn readyToSubmit;`
)() as (digits: string, busy: boolean, failed: boolean, lastSent: string | null) => boolean;

test("the code field sends itself on the sixth digit, and not before", () => {
  assert.equal(readyToSubmit("12345", false, false, null), false);
  assert.equal(readyToSubmit("123456", false, false, null), true);
});

test("a seventh keystroke does not re-send the six digits already sent (low)", () => {
  // The field slices to six, so a seventh keystroke leaves `digits` exactly as
  // it was. Sending it again spends one of the attempts this claim gets before
  // its code is dead for good.
  assert.equal(readyToSubmit("123456", false, false, "123456"), false);
  assert.equal(readyToSubmit("123457", false, false, "123456"), true);
});

test("the code field sends nothing while an attempt is already in flight", () => {
  assert.equal(readyToSubmit("123456", true, false, null), false);
});

test("the code field stops sending itself once an attempt has come back wrong", () => {
  assert.equal(readyToSubmit("123456", false, true, null), false);
});

/// H2, the half of it a mistyped destination runs into. Every route back to the
/// form used to be gated - the code field on `!claim.codeVerified`, one restart
/// on a stalled Cloudflare, another on a 410 or 429 - so a claim whose code
/// went to an address its owner cannot read had none of them, and no way back
/// but clearing site data.
test("the strip's way out is one control, in its own component (H2)", () => {
  const startOver = extractBraceBlock(SOURCE, "function StartOver({ onRestart }: { onRestart: () => void }) {");
  assert.match(startOver, /onClick=\{onRestart\}/, "StartOver must be wired to onRestart");

  const wired = SOURCE.match(/onClick=\{onRestart\}/g) ?? [];
  assert.equal(wired.length, 1, "one way out, not one per state the strip can be in");
});

test("the way out is rendered whatever state the claim is in (H2)", () => {
  assert.match(SOURCE, /<StartOver onRestart=\{onRestart\} \/>/, "expected StartOver to be rendered");
  assert.doesNotMatch(
    SOURCE,
    /[?&|:]\s*\n?\s*<StartOver/,
    "StartOver must not sit behind a conditional - that is the defect"
  );
  assert.doesNotMatch(
    SOURCE,
    /\bsetStuck\b/,
    "a restart gated on a 410 or 429 is what left everyone else with no way out"
  );
});

/// M6 and the other low, both in the strip's four-second poll: a 404 was
/// dropped along with every other non-200, and the handle went into the query
/// string unencoded while the one-shot reconcile encoded the same value.
test("the poll builds its URL with the shared encoder, not by interpolating the handle (low)", () => {
  assert.doesNotMatch(SOURCE, /handle=\$\{claim\.handle\}/, "the handle must be url-encoded");
  assert.match(SOURCE, /fetch\(verifyClaimUrl\(claim\.handle\)/);
});

test("the poll decides what a status means with the same rule the reconcile uses (M6)", () => {
  assert.match(SOURCE, /pollVerdict\(response\.status\)/);
  assert.match(SOURCE, /verdict === "gone"/, "a claim the server has dropped must not stay on screen");
  assert.match(SOURCE, /verdict === "wait"/, "a fault we cannot read must leave the claim alone");
});

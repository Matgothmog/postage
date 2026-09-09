import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { isValidHandle } from "@/lib/handle";

/// PickHandle.tsx cannot be imported here — same constraint documented in
/// Account.test.ts: `.tsx` is unrecognized by `node --experimental-strip-types`,
/// and there is no jsdom/React renderer or browser tool in this environment to
/// mount the component even if it loaded.
///
/// So the `ready` line — the submit button's gate — is pinned by reading it
/// out of the real source and evaluating it, not by hand-copying it. If
/// `ready` is ever rewritten to stop calling `isValidHandle` (reintroducing
/// the bug where the button enabled for a handle the server would reject),
/// this file's regex still finds a `const ready = ...` line, but the eval no
/// longer matches the server-equivalent verdicts below and the tests fail.
const SOURCE_PATH = fileURLToPath(new URL("./PickHandle.tsx", import.meta.url));
const SOURCE = readFileSync(SOURCE_PATH, "utf8");

const readyLine = SOURCE.match(/const ready = (.+);/);
assert.ok(readyLine, "expected a `const ready = ...;` line in PickHandle.tsx");

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

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

/// ChallengeActions.tsx cannot be imported here - see Account.test.ts for why
/// .tsx is out of reach for this test runner, and there is still no
/// jsdom/React renderer or browser tool in this environment to mount it even
/// if it loaded. The actual World ID logic this component drives —
/// `runSelfieCheck`, `postWorldVerify`, `describeWorldIdFailure` — is real,
/// executable code in `@/lib/world-id` and is exercised directly by
/// `world-id.test.ts`. What is left to pin here is the wiring: that this
/// component actually calls that module the way the mock/live contract
/// requires, rather than some inline reimplementation that quietly drifts
/// from it. Read from source text and matched by regex, the same approach
/// `FinishClaim.test.ts` uses for the same kind of fact.
const SOURCE = readFileSync(fileURLToPath(new URL("./ChallengeActions.tsx", import.meta.url)), "utf8");

const PAGE_SOURCE = readFileSync(fileURLToPath(new URL("./page.tsx", import.meta.url)), "utf8");

test("verifyHuman is wired to @/lib/world-id, not an inline reimplementation of the World ID calls", () => {
  assert.match(SOURCE, /from\s+"@\/lib\/world-id"/);
  for (const name of ["fetchRpContext", "openSelfieCheckWithIDKit", "postWorldVerify", "runSelfieCheck"]) {
    assert.match(
      SOURCE,
      new RegExp(`\\b${name}\\b`),
      `expected ChallengeActions.tsx to use ${name} from @/lib/world-id`
    );
  }
});

test("the mock branch posts a bare token — no proof — exactly as it always has", () => {
  const mockBranch = SOURCE.match(/identityMode === "mock"[\s\S]{0,200}/);
  assert.ok(mockBranch, "expected an identityMode === \"mock\" branch in verifyHuman");
  assert.match(mockBranch![0], /postWorldVerify\(token\)/);
});

test("the live branch runs the Selfie Check before posting, and forwards its proof", () => {
  const selfieCheckIndex = SOURCE.indexOf("await runSelfieCheck(");
  const verifyCallIndex = SOURCE.indexOf("postWorldVerify(token, selfieCheck.proof)");
  assert.ok(selfieCheckIndex !== -1, "expected verifyHuman to call runSelfieCheck under live mode");
  assert.ok(verifyCallIndex !== -1, "expected the live path to forward selfieCheck.proof, not a bare token");
  assert.ok(selfieCheckIndex < verifyCallIndex, "the proof must be obtained before it is posted");
});

test("a failed Selfie Check sets an error outcome instead of posting anything", () => {
  assert.match(SOURCE, /!selfieCheck\.ok/);
  assert.match(SOURCE, /setOutcome\(\{ kind: "error", message: selfieCheck\.message \}\)/);
});

test("the World App link is only shown while a connector URI is actually pending", () => {
  assert.match(SOURCE, /worldConnectorUri\s*&&/);
  assert.match(SOURCE, /onConnectorReady:\s*setWorldConnectorUri/);
});

test("verifyHuman resets verifying and the connector link in a finally, so a thrown error cannot strand the button", () => {
  const finallyBlock = SOURCE.match(/\}\s*finally\s*\{([\s\S]*?)\}\s*\n\s*\}/);
  assert.ok(finallyBlock, "expected a finally block in verifyHuman");
  assert.match(finallyBlock![1], /setVerifying\(false\)/);
  assert.match(finallyBlock![1], /setWorldConnectorUri\(null\)/);
});

test("page.tsx passes the server-computed identityMode down as a prop, not a public env var", () => {
  assert.match(PAGE_SOURCE, /import\s*\{\s*identityMode\s*\}\s*from\s*"@\/lib\/env"/);
  assert.match(PAGE_SOURCE, /identityMode=\{identityMode\(\)\}/);
  assert.doesNotMatch(SOURCE, /NEXT_PUBLIC_IDENTITY_MODE/);
});

test("the World ID action is read from the signed rp_context, not a second public env var of its own", () => {
  assert.doesNotMatch(SOURCE, /NEXT_PUBLIC_WORLD_ACTION/);
});

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

test("the pay lane only ever hands back an outcome the page above it can actually render", () => {
  // ChallengeActions returns <PayLane> before it reaches its own error render,
  // so an error handed up from the pay lane is shown to nobody. The narrower
  // return type is what stops that from being expressible at all.
  assert.ok(
    SOURCE.indexOf('if (lane === "paying")') < SOURCE.indexOf('outcome?.kind === "error" &&'),
    "the <PayLane> return still comes first — the narrowed onSettled type is load-bearing"
  );
  assert.match(SOURCE, /type SettledOutcome = Exclude<Outcome, \{ kind: "error" \}>/);
  assert.match(SOURCE, /onSettled: \(outcome: SettledOutcome\) => void/);
});

test("the settlement window running out reads as unconfirmed, never as a failed payment", () => {
  assert.match(SOURCE, /async function settle\(\): Promise<SettledOutcome \| null>/);
  assert.match(
    SOURCE,
    /return outcome\.kind === "error" \? null : outcome/,
    "an exhausted window must resolve to null, not to an error outcome"
  );
});

test("a payment that broadcast keeps its hash and is never offered a second one", () => {
  assert.match(SOURCE, /const \{ hash \} = await sendTransaction\(/);
  assert.match(SOURCE, /setBroadcastHash\(hash\)/);

  const broadcastView = SOURCE.match(/if \(broadcastHash\) \{[\s\S]*?\n  \}/);
  assert.ok(broadcastView, "expected PayLane to return a view of its own while a broadcast is unsettled");
  assert.match(broadcastView![0], /\{broadcastHash\}/, "the sender needs the hash to point at");
  assert.doesNotMatch(broadcastView![0], /handlePay|payLabel/, "a broadcast payment must not invite a re-click");
  assert.ok(
    SOURCE.indexOf("if (broadcastHash) {") < SOURCE.indexOf("const payLabel"),
    "the confirmation view must return before the Pay button is ever built"
  );
});

test("nothing navigates away from the page the World ID poll is running on", () => {
  // pollUntilCompletion runs in this page. An automatic redirect unloads it,
  // taking the verification and the QR fallback with it - fatal on a touch
  // device with no World App installed.
  assert.doesNotMatch(SOURCE, /window\.location/, "no automatic navigation on the sender's behalf");
  assert.doesNotMatch(SOURCE, /matchMedia/, "the connector fallback must not be gated on pointer type");
});

test("the connector panel offers both a link to tap and a code to scan", () => {
  assert.match(SOURCE, /href=\{worldConnectorUri\}/);
  assert.match(SOURCE, /<WorldIdQr uri=\{worldConnectorUri\} \/>/);
});

test("the human lane keeps a way through to paying, so a failed World ID is not a dead end", () => {
  const humanLane = SOURCE.match(/lane === "human" && \([\s\S]*?\n      \)\}/);
  assert.ok(humanLane, 'expected a lane === "human" branch rendering an escape of its own');
  assert.match(
    humanLane![0],
    /setLane\("paying"\)/,
    "?as=human must not remove the pay option the sender is still entitled to"
  );
});

test("page.tsx passes the server-computed identityMode down as a prop, not a public env var", () => {
  assert.match(PAGE_SOURCE, /import\s*\{\s*identityMode\s*\}\s*from\s*"@\/lib\/env"/);
  assert.match(PAGE_SOURCE, /identityMode=\{identityMode\(\)\}/);
  assert.doesNotMatch(SOURCE, /NEXT_PUBLIC_IDENTITY_MODE/);
});

test("the World ID action is read from the signed rp_context, not a second public env var of its own", () => {
  assert.doesNotMatch(SOURCE, /NEXT_PUBLIC_WORLD_ACTION/);
});

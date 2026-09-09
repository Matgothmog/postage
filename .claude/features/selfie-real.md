# selfie-real

## Goal

Switch the World ID "Selfie Check" human-verification flow from its mock implementation to
the real one, on the `refactor/polish` branch in the `wt-polish` worktree. Done when the
client actually obtains a World ID Selfie Check proof via IDKit and the `/api/world/verify`
route verifies it against World's v4 API under `IDENTITY_MODE=live`, with the existing
`IDENTITY_MODE=mock` path still working as the local/test fallback, and web tests, lint and
typecheck all at exit 0.

## Plan

- [ ] C1 — Restore `@worldcoin/idkit` + `@worldcoin/idkit-server` and the
      `web/src/app/api/world/context/route.ts` request-signing route on `refactor/polish`.
- [ ] C2 — Make `/api/world/verify` accept the client's World ID proof from the request body
      and pass it to `verifyWithWorld()`.
- [ ] C3 — Wire `verifyHuman` in `ChallengeActions.tsx` to IDKit's `selfieCheckLegacy()` and
      send the resulting proof to `/api/world/verify`.
- [ ] C4 — Document the env vars the live path requires and default them safely.
- [ ] C5 — Code review of the full diff at Opus/high.
- [ ] C6 — Security review of the full diff at Opus/high.
- [ ] C7 — End-to-end test of the pipeline: send a mail, receive the challenge, clear the
      human check on the mock path, confirm the held mail is released.
- [ ] C8 — Report how the mail pipeline can be exercised locally end to end, and which
      transports and credentials are real versus stubbed. (Prerequisite for briefing C7.)

## Dispatches

| Charge | Model / effort | Outcome |
|---|---|---|
| Refresh `.claude/context.md` and report the conventions, skills, and commands relevant to switching the selfie flow from mock to real | Sonnet / med | success |
| Restore `@worldcoin/idkit` and `@worldcoin/idkit-server` and the `web/src/app/api/world/context/route.ts` request-signing route on `refactor/polish` | Sonnet / med | dispatched |
| Report which MCP servers are configured and available in this project, and whether any of them can send or read mail for matgothmog.angelux@gmail.com | Haiku / low | dispatched |
| Make `/api/world/verify` accept the client's World ID proof from the request body and pass it to `verifyWithWorld()` | Sonnet / med | success |
| Report how the postage mail pipeline can be exercised locally end to end, and which transports and credentials are real versus stubbed | Sonnet / med | success |
| Wire `verifyHuman` in `ChallengeActions.tsx` to IDKit's `selfieCheckLegacy()` and send the resulting proof to `/api/world/verify` | Sonnet / med | blocked → re-dispatched at Sonnet / med with `page.tsx` added to scope → success |
| Document the env vars the live World ID path requires and default them safely | Haiku / low | success |
| Correct `.claude/context.md` — its Traps section claims `.env` files exist in `web/` and `contracts/`, and none do | Sonnet / med | success |
| Review the full selfie mock-to-real diff for correctness and convention drift | Opus / high | success — findings raised |
| Security-review the full selfie mock-to-real diff | Opus / high | success — findings raised |
| Bind the World ID proof to the challenge token and reject replayed nullifiers | Opus / high | success |
| Harden `/api/world/context` against anonymous unbounded signing | Opus / high | success |
| Report whether World's v4 verify response field is `results` and whether Selfie Check requires the sandbox environment | Sonnet / med | success |
| Make the signed rp_context the single source of the action and the polling window, and wire the client to it | Sonnet / med | success (merged the `WORLD_ACTION` twin charge into it — same seam, same files) |
| Stop `/api/world/verify` leaking internal error text and fix its 400/502 taxonomy | Sonnet / med | success |
| Refresh `.claude/context.md` for the anti-replay and rp_context changes, including the removed `NEXT_PUBLIC_WORLD_ACTION` | Sonnet / med | success |
| Re-review the hardened selfie diff, focusing on the anti-replay and rp_context code added since the first review | Opus / high | success — findings raised; report arrived out of format, accepted on content |
| Re-run the security review against the hardened selfie diff and confirm the four fixed findings are actually closed | Opus / high | success — one new critical found |
| Let a nullifier re-bind to a new sender so a poisoned victim can reclaim it | Opus / high | dispatched |
| Make `identityMode()` fail closed instead of defaulting to mock on an unrecognised value | Sonnet / high | dispatched |
| Fix `pollTimeoutMs` returning NaN and validate the rp_context instead of casting it | Sonnet / med | dispatched |
| Require exactly one selfie credential in `/api/world/verify` and stop logging the challenge token and sender-to-nullifier linkage | Sonnet / med | queued behind G1 (file overlap) |

## Decisions

- Target the `wt-polish` worktree on `refactor/polish` — because it is the current, refactored
  state with full test coverage (255 web / 51 worker / 68 forge, all green at `3732769`).
  Rejected: the `postage` worktree on `refactor/sweep`, which still has the idkit packages and
  the signing route installed and so needs no dependency add, because it is the older
  pre-polish code and the work would have to be merged forward afterwards. User's call,
  2026-09-09.
- Re-adding `@worldcoin/idkit` and `@worldcoin/idkit-server` is approved — because commit
  `3732769` deleted them as unreachable dead code, and the real path cannot exist without
  them. This is an M11 dependency gate and an M11 identity/auth gate; the user gave explicit
  go-ahead in this session, 2026-09-09.
- Keep the `IDENTITY_MODE=mock` path intact rather than deleting it — because it is what the
  existing test suite and local development run against.
- No new worktree or `feat/` branch — the user chose to work directly on `refactor/polish`,
  waiving the M17 one-feature-one-worktree step for this change.
- An end-to-end test is part of the definition of done — user's request, 2026-09-09 — but the
  mail leg is **mocked** rather than delivered to a real inbox. User's call, 2026-09-09, after
  being told the constraints: the goal is proving the pipeline is wired, not exercising a real
  mailbox. Two facts drove it: (a) under `IDENTITY_MODE=live` the Selfie Check proof is
  produced by the World App on a phone, so no subagent can generate one; (b) despite
  `claude mcp list` reporting a connected "claude.ai Gmail" server, no Gmail tool is actually
  exposed in the session's toolset — only an unauthenticated Google Drive one — so reading
  that inbox was never available. Rejected: gating the feature on a manual phone-in-hand run.
- Consequence to keep in view: C7 proves the pipeline, not the live World ID leg. A real
  `IDENTITY_MODE=live` proof has never been exercised end to end and remains unverified —
  record that under Not done, do not let a green C7 imply otherwise.
- C7 is the full product loop, not a unit test — user's clarification, 2026-09-09: send a
  mail, receive the challenge mail, clear the human check on the mock path, then confirm the
  originally held mail is released to the recipient.
- C7 runs locally with stubbed transports, not against a real Gmail inbox — user's call,
  2026-09-09, after C8 established the constraints. It drives `POST /api/mail/inbound`, takes
  the `challenge_url` straight from the JSON response (no mailbox read needed), clears the
  human check on the mock path, and asserts the Mailgun release call fires with the right
  recipient and MIME. Rejected: running against the deployed stack, which is the only thing
  that has ever put a real message in a real inbox, because it would exercise production and
  would test what is deployed rather than the wiring sitting uncommitted on this branch.
- The client learns the identity mode as a prop from the Server Component, not from a public
  env var — because `web/src/app/c/[token]/page.tsx:107-113` already passes `dangerous`,
  `held` and `lane` into `<ChallengeActions>` that way, and the mode is a non-secret
  `"live" | "mock"`. Rejected: a `NEXT_PUBLIC_IDENTITY_MODE` twin, because it is a second
  independently-set source of truth that can desync from the `IDENTITY_MODE` the server
  enforces — an operator flips one and forgets the other, and every sender gets "A World ID
  proof is required" with nothing client-side explaining why. Also rejected: inferring the
  mode from a failed `/api/world/context` call, because a transient network error under live
  mode would silently drop the user into a flow the server rejects.
- `NEXT_PUBLIC_WORLD_APP_ID` and `NEXT_PUBLIC_WORLD_ACTION` are public env vars, even though a
  public `NEXT_PUBLIC_IDENTITY_MODE` was rejected just above — because `IDKit.request()` needs
  `app_id` and `action` in the browser and `/api/world/context` returns only
  `{rp_id, nonce, created_at, expires_at, signature}`. The distinction that makes this safe:
  a wrong value here fails loudly (IDKit or World rejects the request or the signature),
  whereas a stale `NEXT_PUBLIC_IDENTITY_MODE` would silently misroute the sender. Both are
  documented as public in `docs/world-feedback.md` and follow the existing
  `NEXT_PUBLIC_PRIVY_APP_ID` convention.
- The signal-binding charge got `web/src/app/c/[token]/ChallengeActions.tsx` added to its scope
  for one line (`signal: token`) — because the challenge token lives only at that call site,
  and without it live mode would ship inert until a later charge wired it. The file's nominal
  owner had not been dispatched, so there was no real conflict. Both halves were kept: the
  `world-id.ts` module also fails closed when the signal is absent, so a future caller who
  forgets cannot silently produce an unbound proof rather than merely failing typecheck.
- The live World ID path ships unverified, and that is recorded rather than papered over —
  user's call, 2026-09-09. No real Selfie Check proof has ever been exercised against this
  code. Rejected: a recorded-proof fixture (would catch field-name errors but still needs one
  real proof to record), and a manual phone-in-hand run after deploy (deferred, not refused).

## Surprises

- The mock/real switch (`identityMode()`, `web/src/lib/env.ts:8-9`) and the real server-side
  verifier (`verifyWithWorld()`, `web/src/app/api/world/verify/route.ts:125-151`) were both
  already written. The actual gap is on the client: `verifyHuman` in `ChallengeActions.tsx:78`
  never calls World ID at all — it POSTs a bare `{ token }` in both worktrees. So flipping
  `IDENTITY_MODE` to `live` alone would not have worked.
- The mock derives its nullifier by hashing the sender's address
  (`route.ts:153-158`), making it one free pass per address rather than per person.
- `docs/world-feedback.md` already documents the exact wiring needed: the IDKit builder is
  `selfieCheckLegacy()` (not `selfieCheck()`), it needs `allow_legacy_proofs: true`, and the
  v2/v4 field names differ (`nullifier`, not `nullifier_hash`).
- `KNOWN_ISSUES.md` records this gap under "Blocking a real demo".
- The two worktrees diverge on local env files, opposite to what the context cache said: the
  `.env` files (`web/.env.local`, `contracts/.env`, `subgraph/.env`) exist in `postage` and do
  **not** exist in `wt-polish`. So nothing on this branch can be run against real credentials
  without creating them first.
- There is no `.claude/context.md` in the `postage` worktree at all — the file was committed on
  `refactor/polish`'s history, never on `refactor/sweep`. An earlier brief of mine asserted a
  `postage` copy needing sync; that was wrong, and no such file needs maintaining.
- The test runner is `node --experimental-strip-types`, which rejects TypeScript constructor
  parameter-properties at runtime (`ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX`) although `tsc` accepts
  them. Two subagents lost time to this before it was recorded in the context cache.
- No browser tool exists in this environment, so S12's screenshot step cannot be performed on
  any frontend charge here. The client wiring was verified by `next build` plus wiring tests.
- No `.claude/skills/` directory exists in either worktree; only the built-in `code-review`
  and `security-review` skills are available. `security-review` needs `origin/HEAD`, which is
  unset because nothing on `refactor/polish` has been pushed — C6 may be blocked on that.

## Anti-replay design, as built

Two guards, catching different attacks — both are needed:

- **Signal binding.** The client passes the challenge token as IDKit's `signal`; the server finds
  the `identifier === "selfie"` entry in the posted proof and requires its `signal_hash` to equal
  `hashSignal(token)` (from `@worldcoin/idkit/hashing`), checked *before* World is called. Sound
  because `signal_hash` is a public input to the ZK proof: altering it makes World reject the
  proof, so *verifies at World* + *carries our hash* means bound to this token. A proof with no
  `signal_hash` is refused, not tolerated. This kills replaying the same bytes into a different
  challenge.
- **Nullifier ledger** (`web/src/lib/db/nullifiers.ts`, new table, purely additive — `SCHEMA` runs
  on cold start, so no migration). This kills what the binding cannot see: one human verifying
  under several sender addresses to farm free lanes.

**Replay policy — SUPERSEDED in round two, kept here for the reasoning.** Round one bound a
nullifier to its first sender permanently and refused any other with a 403, accepting that a
person who changes email address loses the free lane for good. That rule was poisonable (see
Round-two review findings) and the 403 path no longer exists. World's own guidance — persist
`(action, nullifier)` UNIQUE and reject every duplicate — was rejected then and still is, because
`PASS_WINDOW_SECONDS` is 15 minutes, `gate.ts:121-123` states that re-earning a window with a
fresh proof "is the design, not a leak", and `recordPersonhood` treats repeat attestation across
the 90-day window as normal. Global-unique would mean one human clears exactly one challenge,
ever.

**Current policy: one nullifier names one sender, and any valid proof moves it.** First claim
inserts; the same sender re-presenting is a no-op that leaves `claimed_at` alone; a different
sender takes over and the previous holder is released in the same write. `claimNullifier` returns
`releasedFrom`, the displaced address — the one fact the caller cannot recover afterwards. A
takeover is logged with two sender addresses only, and the 200 body never names the previous
holder (pinned by test). Atomicity is a two-statement `client.batch(..., "write")` transaction,
not the single-statement shape `issued-contexts.ts` uses, because two tables must move together.
The audit trail is a `nullifier_rebinds` table — one row per takeover, never purged, because the
row count *is* the abuse signal — chosen over columns on the row, which would keep only the last
hop and would have needed a `migrations.ts` entry.

**Known incomplete until the pass-revocation charge lands:** a pass lives in `passes` under
`(handle, sender)` on a 15-minute window, separate state from the ledger row, so releasing a
binding does not release the lane the old address is already standing in. A farmer can therefore
hold several free lanes concurrently, bounded by how many Selfie Checks fit in one 15-minute
window — degraded from "impossible", not equal to it. The implementer wrote this limitation into
`bindNullifierToSender`'s doc comment rather than letting the code imply a guarantee it does not
keep.

Also fixed in passing: `toBytes32` returned World's nullifier in its original casing, feeding
`identityFor` → `getAddress`; mixed-case hex would have thrown `InvalidAddressError`, and two
spellings would have been two ledger rows. Lower-cased in place.

## `/api/world/context` hardening, as built

The security review's proposed fix does not exist: `getSessionCommitment(sessionId)` is a regex
check over a session id belonging to the separate `createSession`/`proveSession` flow, and
`SignRequestParams` accepts only `{signingKeyHex, action?, ttl?}` with no field on `RpContext` to
carry a commitment. Nothing in the SDK binds a signed context to a caller, so the binding is ours:
the route requires a valid, unsettled, non-dangerous challenge token, and writes each signed nonce
to `issued_rp_contexts` under a single atomic `INSERT … SELECT … WHERE COUNT < 5` — the same
one-statement ceiling `claimClassification` uses, and serverless-safe because the store is Turso
rather than process memory. TTL is an explicit 300s; `pollTimeoutMs(context)` derives the client's
polling window from the signature's own timestamps so the two cannot drift.

**Behaviour change accepted:** refusing settled challenges closes `gate.ts`'s `recover()` path,
which its comment marks deliberate. Narrow — it needs a settled challenge, an undelivered message,
and a lapsed pass — and judged correct for a *signing* endpoint; recovery should get its own path
rather than an unauthenticated signature.

## What a caller learns from `/api/world/verify`, after the error-taxonomy fix

- **200** — success body unchanged (`identity`, `nullifierHash`, `expiresAt`).
- **400** — only facts about the caller's own request or proof: missing/malformed proof, wrong
  challenge binding, an already-used or unrecognised signing context, or World's own
  `detail`/`code` about a proof World itself rejected. Nothing about our configuration.
- **403 / 404** — dangerous-tier refusal or an unknown challenge. The nullifier-already-claimed
  403 no longer exists; a different sender now takes the binding over rather than being refused,
  and the 200 body never names the displaced holder.
- **502** — one of six fixed generic strings. The caller learns only that an upstream dependency
  failed, never why; the `cause.message`, RPC URLs and World's raw outage detail go to
  `console.error` server-side only.
- **500** — Next's bare empty-body response for anything unanticipated, including every
  `required()` misconfiguration. The caller learns nothing; the missing variable name is logged.

The single-use nonce gate (`spendIssuedContext`) runs after the signal binding and before World
is called, so a never-issued or already-spent nonce is refused without costing an upstream call.
A DB failure while consuming fails closed at 502.

## Round-two review findings

The re-reviews confirmed three of the four original fixes closed — the signing oracle, the RPC
URL leak and the configuration disclosure. The proof binding is only **partially** closed, and
the fix round introduced a new critical.

**Critical, introduced by round one: the nullifier ledger is poisonable.**
`verify/route.ts:220` binds the nullifier to `challenge.sender` — the address on the token, not
the human who took the selfie — and nothing checks they are the same party. An attacker mails a
handle from their own address, receives `/c/<token>`, and hands that link to a victim. The
victim's Selfie Check passes signal and nonce binding legitimately, and the ledger writes an
irreversible row claiming their nullifier for the attacker. The attacker gains a free delivery
and permanent, unrecoverable denial of the victim's free lane, for the cost of one email. The
table has no delete, expiry, or admin path anywhere in the tree.

**Fix chosen (user's call, 2026-09-09): one nullifier, one *active* sender, movable by the
human.** A nullifier presented with a valid proof for a different sender moves the binding, so a
poisoned victim reclaims it by verifying again — the attack degrades from permanent denial to a
nuisance. Farming stays blocked because claiming a new address releases the old. Rejected:
time-boxing the binding to the 90-day credential window (a victim still loses their lane for a
quarter of a year through no fault of their own), and dropping the sender binding entirely
(removes poisoning but reopens the multi-address farming hole the ledger was added to close).

Also being fixed this round:

- **`identityMode()` fail-open is now the single point of failure.** Every guard added in round
  one lives inside the `identityMode() === "live"` branch, so `IDENTITY_MODE=Live` or a trailing
  space silently disables signal binding, the nonce gate, the World call and real nullifiers at
  once — while still writing rows into the shared ledger. Previously deferred as "harmless until
  live mode is reachable"; that reasoning no longer holds.
- **The two guards can read different credentials.** `route.ts:88` checks `signal_hash` on the
  first selfie entry in the *request*; `route.ts:388` takes the nullifier from the first selfie
  entry in the *response*. Nothing asserts there is exactly one.
- **Bearer token and identity linkage in logs.** `route.ts:139-141, 209-211, 233-236` log the live
  challenge token; `:268-271` puts `sender` and `nullifierHash` on one line — the exact
  email-to-pseudonym linkage `schema.ts:75-81` says the table avoids.
- **`pollTimeoutMs` returns `NaN`** for a context with missing timestamps (`rp-context.ts:43-44`,
  `Math.max(NaN, floor)` is `NaN`), so polling fires instantly. Reachable via the unchecked
  `as RpContext` cast at `world-id.ts:109`. `rp-context.test.ts:28-32` tests the floor only where
  it already works. Also `world-id.ts:252` lacks the `.catch(() => null)` that `:91` has.

Verified clean by measurement rather than assertion: the 5-per-token cap is genuinely atomic
(40 concurrent calls → exactly 5 winners; 20 concurrent consumes → exactly 1), there is no
permanent lockout from the nonce gate (bounded to one 300s window, pay lane stays open), no
timing side channel in the `signal_hash` comparison, and the status taxonomy is a token oracle
over an unguessable 122-bit space.

Deferred again, recorded: the nonce is spent before World is called so an upstream outage burns
a slot; mock-mode ledger rows are unbounded and keyed on an attacker-controlled `From`; World's
`detail` is reflected verbatim into our 400; `purgeExpiredContexts` is unguarded so a DELETE
failure kills signing; the static idkit import still ships 870 KB of WASM to mock-mode senders;
`claimNullifier`'s insert-then-select is sound only because nothing deletes from the table, and
`.claude/context.md:63-67` wrongly calls it a single atomic statement.

**Load-bearing assumption still unverified:** that World validates the proof against
`responses[i].signal_hash` as posted. The SDK types it `signal_hash?: string` — optional, with no
statement about verification — and our only test stubs World with unconditional success. If World
ignores or recomputes it on the `allow_legacy_proofs` v3 path, the signal binding is decorative
and a harvested proof clears challenges indefinitely. One recorded live proof would settle it.

## Review findings

Two Opus reviews ran on the completed diff. Both confirmed the World ID contract itself is
correct — `selfieCheckLegacy()` not `selfieCheck()`, `allow_legacy_proofs: true`, `nullifier`
not `nullifier_hash`, identifier `"selfie"` — and that the mock path, mode boundary, typing and
conventions are clean. The findings below are what they raised.

Being fixed before the end-to-end test (user's call, 2026-09-09):

1. **The proof is bound to nothing.** `world-id.ts:33` passes no `signal`; the nullifier is
   persisted nowhere and the gate is keyed on `(handle, sender)` (`gate.ts:99-101`). One
   captured proof clears any challenge for any sender for the 90-day window. The live path is
   strictly weaker than the mock it replaced, since the mock's nullifier was sender-derived.
2. **`WORLD_ACTION` / `NEXT_PUBLIC_WORLD_ACTION` are a desync pair.** The server signs one, the
   browser sends the other; drift gives `invalid_rp_signature` on every proof, invisible to
   local tests. This is the same failure mode the log rejected `NEXT_PUBLIC_IDENTITY_MODE` for
   — the "fails loudly" argument that justified the split holds only in production, which is
   the wrong place to discover it.
3. **`/api/world/context` is an anonymous unbounded signing oracle** (`route.ts:7-19`); the
   signed message names no user, session or challenge, and the nonce is discarded.
4. **`verify/route.ts:136-138` returns `cause.message`**, which viem builds with the full
   transport URL — leaking `ARC_RPC_URL` and any embedded key to any caller with a token.

Deferred, recorded as follow-ups:

- `identityMode()` fails open silently on `IDENTITY_MODE=Live` or a trailing space
  (`env.ts:8-9`). Pre-existing, but harmless until this diff made live mode reachable.
- Signed TTL is 300s while the client polls for 15 minutes — a slow sender gets
  `rp_signature_expired` surfaced as a generic "Try again". Partly addressed by F3.
- The pay button is `disabled={verifying}` for that whole window, locking a sender out of both
  lanes (`ChallengeActions.tsx:159`).
- All four catches in `world-id.ts` are bare `catch {}`; nothing logs why a live verification
  failed, so a config error would present as user churn.
- The static idkit import puts ~74 KiB plus an 870 KB WASM asset in `/c/[token]`'s first-load
  graph for every sender, including the 100% currently on mock.
- `postWorldVerify` never checks `response.ok`; a DB failure in `openGate` reaches the sender as
  a raw `Unexpected token '<'`.
- `KNOWN_ISSUES.md:17-46` is now false and still marked "Verified" — it claims World ID is
  unintegrated and the packages removed.
- `node --test 'src/app/c/[token]/ChallengeActions.test.ts'` runs **zero** tests: Node globs the
  path and `[token]` is a character class. Run the whole suite.

Both previously-unresolved assumptions are now settled against docs.world.org, fetched
2026-09-09 (two independent retrievals agreeing):

- **`results` is correct.** The documented success schema is
  `{success, action, nullifier, created_at, environment, session_id, results: [VerifyV4Result], message}`
  with `VerifyV4Result = {identifier, success, nullifier, code, detail}` — exactly what
  `verify/route.ts:186-191` models. Caveat: the installed SDK types the endpoint's response
  nowhere at all, so this is documented, not compiler-checked; `route.test.ts:32-36`'s fixture
  remains the only in-repo check and cannot catch drift.
- **Sandbox is not an SDK value.** `environment: "production"` — the default inherited by
  omission — is correct for real phones. "Sandbox app access" in `docs/world-feedback.md:83-95`
  is a Developer Portal grant, separate from the feature flag, and has no env var. Nothing to
  configure in code; two Portal grants must be turned on for the app.
- **`signal_hash` is request-side only.** It does not appear in the verify response. A relying
  party must compute `hashSignal(expectedSignal)` itself (exported from
  `@worldcoin/idkit-core/hashing`) and compare against the `signal_hash` on the proof the client
  supplied — sound because altering it invalidates the ZK proof. Note
  `verify/route.ts:29-31` types only `identifier` per response entry, so `signal_hash` is
  silently dropped today.
- **`max_verifications` is undocumented.** No primary source across three docs pages and two
  searches. It cannot be relied on as a World-side replay control; the nullifier ledger carries
  the weight. World's own guidance is to persist `(action, nullifier)` unique and reject
  duplicates.
- The SDK README still points at the retired `developer.worldcoin.org` domain while the repo and
  current docs use `developer.world.org` — SDK doc lag, not a repo bug.

Correction to an earlier entry here: `origin/main` is at `3732769`, so this branch **has** been
pushed. The `security-review` skill still could not run, but because the session's launch cwd
(`/home/matgothmog/3A/ETHGLOBAL`) is not a git repository — not for the unpushed-branch reason
recorded earlier.

## Summary

**Changed:** `web/src/lib/world-id.ts` (new), `web/src/lib/rp-context.ts` (new),
`web/src/lib/db/nullifiers.ts` (new), `web/src/lib/db/issued-contexts.ts` (new),
`web/src/app/api/world/context/route.ts` (restored, then hardened),
`web/src/app/api/world/verify/route.ts`, `web/src/app/c/[token]/ChallengeActions.tsx`,
`web/src/app/c/[token]/page.tsx`, `web/src/lib/db/passes.ts`, `web/src/lib/db/schema.ts`,
`web/src/lib/db/migrations.ts`, `web/src/lib/env.ts`, `web/.env.local.example`,
`web/package.json` (+ lockfile: `@worldcoin/idkit`, `@worldcoin/idkit-server` restored).

**Behavior:** Under `IDENTITY_MODE=live` a sender now completes a real World ID Selfie Check
in the World App, and the gateway verifies the proof against World's v4 API before releasing
held mail. `IDENTITY_MODE=mock` behaves exactly as before and remains the default. Beyond the
wiring, the identity boundary gained the controls it never had: the proof is bound to its
challenge by `signal_hash`, a signed rp_context is single-use and requires a valid unsettled
challenge, a nullifier names one sender at a time and moves when its owner re-proves (releasing
the previous address's earned passes in the same transaction), and an unrecognised
`IDENTITY_MODE` is a hard error rather than a silent downgrade to "everyone passes". No response
body carries internal error text; no log line carries a challenge token or links a sender to a
nullifier.

**Tests:** 383/383 web (from a 285 baseline), lint, typecheck and `next build` all at exit 0.
New coverage: the World ID client module, the rp_context TTL/poll-window derivation, both
ledgers including real concurrency tests, the fail-closed identity mode, the verify route's
error taxonomy and log hygiene, and one integration test walking the whole pipeline —
inbound mail held, challenge issued, human check cleared on the mock path, held message
released, delivery recorded.

**Not done:**
- **The live path has never been exercised against a real World App proof.** Everything is
  wired and typed, but no real Selfie Check has been through this code.
- **One load-bearing assumption is unverified**: that World validates the proof against
  `responses[i].signal_hash` as posted. The SDK types it optional and says nothing about
  verification, and our test stubs World with unconditional success. If World ignores or
  recomputes it on the `allow_legacy_proofs` v3 path, the signal binding is decorative. One
  recorded live proof settles it; nothing local can.
- The two Developer Portal grants (feature flag, Sandbox app access) must be enabled on the app.
- The end-to-end test proves the pipeline on the mock path with stubbed transports. It is not
  proof of real delivery: the mail worker, the Arc RPC and the classifier model are loopback
  stand-ins.

**Follow-ups worth filing:**
- The rebind DELETE is steerable at any address an attacker binds under their own nullifier,
  because `challenge.sender` is an unauthenticated SMTP `From`. Bounded — each cycle costs a
  real human selfie, and it can only reach addresses the attacker bound themselves.
- `nullifier_rebinds` accumulates a permanent address-linkage graph, a stronger
  de-anonymisation primitive than the `sender`+`nullifierHash` pairing that was deliberately
  removed from the logs. The takeover log line still puts two of one human's addresses together.
- The nonce is spent before World is called, so an upstream outage burns one of five slots.
- Mock-mode nullifier rows are unbounded and keyed on an attacker-controlled `From`.
- `purgeExpiredContexts` is unguarded: a DELETE failure kills signing entirely.
- The static idkit import ships ~74 KiB plus an 870 KB WASM asset to every sender, including
  the mock-mode ones who never call it.
- The pay button is disabled for the whole ~270s polling window, locking a sender out of both
  lanes if they walk away.
- `KNOWN_ISSUES.md:17-46` is now false and still marked "Verified" — it claims World ID is
  unintegrated and the packages removed.
- `passes` has no index on `sender` alone, so the revocation subquery scans the table. Fine at
  current scale; no measurement exists, so it was deliberately not optimised.

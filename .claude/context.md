# Repo context

Verified: 2026-09-09 · Scout run: Sonnet/medium

This is `wt-polish`, a linked worktree of the `postage` repo
(`origin` = `github.com/Matgothmog/postage.git`) on branch `refactor/polish`,
branched from `refactor/sweep` @ `6c5738a`; gitdir under
`../postage/.git/worktrees/wt-polish`. `../postage` is the main worktree and
stays on `refactor/sweep` — older, pre-polish (no `worker` tests, no
`shared/`, monolithic `web/src/lib/db.ts`). A mirror of this file lives at
`/home/matgothmog/3A/ETHGLOBAL/.claude/context.md`, outside any git repo,
never committed; treat *this* copy as the source of truth once both exist.
Nobody edits `.claude/features/*.md` or `.claude/CHANGELOG.md` from here —
those are the main agent's, per the top-level CLAUDE.md.

## Commands

World ID re-bind policy and pass revocation landed since the last full pass
(2026-09-08, HEAD `3732769`), uncommitted on top of it. `web` test/lint/
typecheck were re-run against that tree 2026-09-09 and are clean (**382/382
tests, up from 334** — verified directly by this scout run). `worker`/
`contracts`/`subgraph` were untouched and not re-run, only reconfirmed via
`git status` showing no diff under those three directories.

- install: `npm install` inside each of `web/`, `worker/`, `subgraph/` (three
  independent package.json's, no root workspace) — source:
  `{web,worker,subgraph}/package.json`.
- web test: `cd web && npm test` → `node --experimental-strip-types --import
  ./test/register.mjs --test 'src/**/*.test.ts'` — 382/382 pass, ~20s, no
  network (SDK redirected to a local stub — see Traps) — verified directly
  this run — source: `web/package.json:11`.
- web typecheck: `cd web && npm run typecheck` → `tsc --noEmit` — clean,
  verified this run.
- web lint: `cd web && npm run lint` → `eslint` (flat config,
  `web/eslint.config.mjs`) — clean, verified this run.
- web build: `cd web && npm run build` → `next build` (Turbopack) — succeeds,
  10 routes; not re-run this pass, no routing change since the last build.
- worker test: `cd worker && npm test` → `node --experimental-strip-types
  --test 'src/**/*.test.ts'` — 51/51 pass — source: `worker/package.json`.
- worker typecheck: `cd worker && npm run typecheck` → `tsc --noEmit` — clean.
- contracts test: `cd contracts && forge test` → 68/68 pass (4 suites),
  local only (forge-std + openzeppelin vendored in `contracts/lib/`).
- subgraph codegen/build: `cd subgraph && npm run codegen` / `npm run build`
  → `graph codegen` / `graph build` — succeed.

## Conventions

- Comments explain *why*, never *what*; `///`-style prose comments used even
  in plain TS — source: `web/src/lib/db/client.ts`, `web/src/lib/db/passes.ts`,
  `web/src/lib/db/nullifiers.ts`.
- File naming: kebab-case for multi-word lib modules (`wallet-proof.ts`,
  `rp-context.ts`), single lowercase word otherwise (`db/client.ts`,
  `db/passes.ts`); PascalCase for React components — source: `web/src/lib/*.ts`
  listing.
- Tests live beside the code they test, `*.test.ts` suffix, `node --test`
  native runner (no Jest/Vitest) — source: `web/package.json:11`,
  `worker/package.json`.
- One ceiling-per-token check is one atomic SQL statement, not
  read-then-write: `recordIssuedContext` (`web/src/lib/db/issued-contexts.ts`)
  folds the check and the write into one `INSERT ... WHERE` — same pattern
  `claimClassification` used pre-polish. `claimNullifier`
  (`web/src/lib/db/nullifiers.ts`) is **not** this shape any more — three
  tables have to move together, so it's a three-statement `client.batch`
  transaction instead (see Traps); still atomic, just not single-statement.
- TS strict mode on, `@/*` → `src/*` path alias in `web` — source:
  `web/tsconfig.json:22`. `worker/tsconfig.json` excludes `@types/node`.
- Solidity: `pragma solidity 0.8.28` pinned, MIT header, NatSpec `@notice` —
  source: `contracts/src/*.sol`, `contracts/foundry.toml`.
- One duplicated concept is stated once and imported: the rp_context
  TTL/poll-window pair (`web/src/lib/rp-context.ts`) and now
  `EARNED_PASS_CONDITION` (`web/src/lib/db/passes.ts`), which `claimNullifier`
  reaches for as raw SQL text because a `client.batch` statement can't call a
  function — only the SQL travels, so both call sites read one constant.
- New tables need no migration; new columns do. `SCHEMA` (`schema.ts`) runs
  `CREATE TABLE IF NOT EXISTS` on cold start, covering every table added this
  round (`nullifiers`, `nullifier_rebinds`, `issued_rp_contexts`) for free.
  `ADDED_COLUMNS` (`migrations.ts`) is the only path that reaches a column
  added to an *existing* table — `passes.paid_extended_at` is the first case
  of this in the feature, deliberately with no backfill (a pre-migration
  NULL-`uses_left` row can't be told earned from paid-and-extended after the
  fact). Several charges have conflated the two — check which case applies
  before assuming "no migration needed."

## Skills

None found. No `.claude/skills/` directory in this worktree, `../postage`, or
the container root above both. `code-review` and `security-review` are
available as built-in (non-repo-local) skills (see Traps for a correction on
where `security-review` actually fails).

## Traps

- **A World ID nullifier names exactly one sender, and any valid proof moves
  it — it does not bind permanently.** `claimNullifier`
  (`web/src/lib/db/nullifiers.ts`) takes the binding off whoever held it and
  gives it to the new claimant; the same sender re-presenting is a no-op. The
  earlier policy this file documented — first sender wins, a different one
  gets a 403 — is **gone**; that refusal path no longer exists anywhere in the
  route.
- **`claimNullifier` is a three-statement `client.batch(..., "write")`
  transaction, in a fixed order that matters.** (1) an audit-hop `INSERT ...
  SELECT ... RETURNING from_sender` into `nullifier_rebinds`, reading the
  outgoing holder off `nullifiers` before anything moves; (2) a `DELETE FROM
  passes` revoking the outgoing holder's earned passes, across every handle,
  matched against `EARNED_PASS_CONDITION`; (3) the `nullifiers` upsert that
  actually rebinds. (1) and (2) both must read the pre-move holder, which is
  only true because the upsert runs last — reordering breaks the audit trail
  or revokes the wrong sender's passes.
- **`EARNED_PASS_CONDITION`** (`web/src/lib/db/passes.ts`) is
  `"uses_left IS NULL AND paid_extended_at IS NULL"`, not just
  `uses_left IS NULL`. `addPaidUse` against an already-unlimited window only
  extends `expires_at` and leaves `uses_left` NULL, so without the second
  column a payment there was indistinguishable from an earned pass and the
  rebind DELETE above deleted something paid for. `revokeHumanPasses` (the
  old, narrower revocation helper) was removed as provably dead code; its
  tests were migrated onto the shipped `claimNullifier` path —
  `web/src/lib/db/nullifiers.test.ts:207`.
- **`identityMode()` fails closed** (`web/src/lib/env.ts`): unset/empty →
  `"mock"`; exactly `"live"` or `"mock"` accepted; anything else throws,
  including a value differing only by trailing whitespace — deliberately not
  trimmed, so a near-miss is a loud crash, not a silent mock downgrade.
  `web/.env.local.example`'s `IDENTITY_MODE` block documents this.
- **No log line in `/api/world/verify` carries a raw challenge token, or
  pairs a sender with a nullifier hash.** `tokenFingerprint(token)`
  (keccak256) is the correlator used everywhere the token would otherwise
  appear; a rebind is logged with both sender addresses only, never the
  nullifier. `requireSingleSelfieCredential` is enforced on both the incoming
  proof and World's response, so "the entry checked" and "the entry consumed"
  can't be two different credentials.
- **`pollTimeoutMs` can no longer return `NaN`** (`web/src/lib/rp-context.ts`):
  a missing or non-numeric timestamp now falls back to 0 before the floor
  applies, instead of poisoning the `Math.max`. `isRpContext`
  (`web/src/lib/world-id.ts`) replaced the unchecked `as RpContext` cast that
  used to be the only way a malformed context reached it.
- **`NEXT_PUBLIC_WORLD_ACTION` is gone; `WORLD_ACTION` is server-only.** The
  signed action comes back from `/api/world/context` and the client reads it
  off the signed `RpContext` rather than a public env var — source:
  `web/.env.local.example`, `web/src/app/c/[token]/ChallengeActions.tsx:107`
  (still reads `NEXT_PUBLIC_WORLD_APP_ID`, the one surviving public var).
- **`/api/world/context` requires a live, unsettled, non-dangerous challenge
  token**: unknown → 404, dangerous → 403, resolved → 409, unexpired contexts
  per token capped at 5 via one atomic `INSERT ... SELECT ... WHERE COUNT(...)
  < 5` in `web/src/lib/db/issued-contexts.ts` — 429 past that. A signed
  context is single-use: `consumeIssuedContext` marks it spent in the same
  statement that checks it.
- **`/api/world/verify`'s error taxonomy is fixed**: 4xx is always the
  caller's fault; 502 is always one of six fixed generic strings, real cause
  only in `console.error`; 500 is bare. No response body ever carries internal
  error text (RPC URLs, DB errors).
- **World's `max_verifications` is undocumented.** No primary source found
  across docs.world.org; the nullifier ledger carries the whole anti-farming
  burden, not a World-side control.
- **`node --test` globs a path with `[token]` in it as a character class** —
  `node --test 'src/app/c/[token]/ChallengeActions.test.ts'` silently runs
  **zero** tests. Run the whole suite instead.
- **`KNOWN_ISSUES.md:17-46` is stale, still marked "Verified."** Claims World
  ID is unintegrated and idkit removed — both false (packages are back at
  `web/package.json:17-18`). Do not hand-edit it.
- **World ID is wired end to end here; `../postage` still has it dead.**
  `web/src/lib/world-id.ts` holds all World ID I/O, gated on
  `identityMode() === "live"`.
- **`security-review`'s blocker is the launch cwd, not an unpushed branch** —
  `origin/HEAD` resolves fine inside `wt-polish` and `refactor/polish` is
  pushed. It still can't run from `/home/matgothmog/3A/ETHGLOBAL` itself,
  which is not a git repository.
- **An async `before()` hook that resolves through an I/O callback races
  `beforeEach`** on this Node version — use top-level `await` instead, as
  `web/src/app/api/challenge/deliver/route.test.ts` does.
- **No `.tsx` file can be imported by a test.** `node --experimental-strip-types`
  throws `ERR_UNKNOWN_FILE_EXTENSION`. `Account.test.ts`, `PickHandle.test.ts`,
  `FinishClaim.test.ts` extract functions from the `.tsx` source text instead.
- **TS constructor parameter properties (`constructor(private x: T)`) crash
  `node --experimental-strip-types`** with `ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX`
  even though `tsc` accepts them. Write the field assignment out in the
  constructor body.
- The web suite reaches Anthropic through the SDK, which finds credentials
  wherever it can. `web/test/model.ts` starts a loopback stub server and
  points both `ANTHROPIC_BASE_URL` and `ANTHROPIC_API_KEY` at it.
- **`forge install` stages `.gitmodules` and gitlinks even with
  `--no-commit`** — use `--no-git` instead; `contracts/lib` is plain vendored,
  gitignored, not real submodules here.
- Every test file touching the database mints its own `mkdtemp` temp
  directory and sets `DATABASE_URL` before importing the module under test —
  skipping this falls back to a shared default DB file.
- `web/src/lib/contracts.ts` (~1200 of 1219 lines is generated ABI) looks like
  dead weight but is a checked-against-artifacts copy — do not "clean up"
  without re-verifying against `contracts/out/*.json` first.
- **No `.env` files exist in this worktree** — only `*.env.example`
  templates. `../postage/{web/.env.local,contracts/.env,subgraph/.env}` do
  exist there (gitignored, untracked) — a fresh `git worktree add` never
  materializes another worktree's untracked files.
- **No browser tool is available in this environment.** S12's screenshot step
  cannot be performed for any frontend change — say so and rely on unit tests.

## Conflicts

- None found. Lint, typecheck, and all three suites (382 web + 51 worker +
  68 forge = 501 tests) are green on a clean run.

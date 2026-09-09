# Repo context

Verified: 2026-09-08 · Scout run: Sonnet/medium

This is `wt-polish`, a linked worktree of the `postage` repo
(`origin` = `github.com/Matgothmog/postage.git`) on branch `refactor/polish`,
branched from `refactor/sweep` @ `6c5738a`; its gitdir lives under
`../postage/.git/worktrees/wt-polish`. The other worktree, `../postage`, is
the main one and stays on `refactor/sweep` — older, pre-polish state (no
`worker` tests, no `shared/`, monolithic `web/src/lib/db.ts`). A mirror of
this file also lives at `/home/matgothmog/3A/ETHGLOBAL/.claude/context.md`,
one level up, outside any git repo — it is never committed and does not
travel with a branch; treat *this* copy (tracked, travels with `refactor/polish`)
as the source of truth once both exist. Nobody edits `.claude/features/polish.md`
or `.claude/CHANGELOG.md` from here — those are the main agent's, per the
top-level CLAUDE.md.

## Commands

All verified by running them in this worktree, 2026-09-08.

- install: `npm install` inside each of `web/`, `worker/`, `subgraph/` (three
  independent package.json's, no root workspace, no root `package.json`) —
  source: `{web,worker,subgraph}/package.json`.
- web test: `cd web && npm test` → `node --experimental-strip-types --import
  ./test/register.mjs --test 'src/**/*.test.ts'` — 255/255 pass, ~14s, no
  network (SDK redirected to a local stub — see Traps) — source:
  `web/package.json:9`.
- web typecheck: `cd web && npm run typecheck` → `tsc --noEmit` — clean.
- web lint: `cd web && npm run lint` → `eslint` (flat config,
  `web/eslint.config.mjs`) — clean.
- web build: `cd web && npm run build` → `next build` (Turbopack) — succeeds,
  10 routes (1 static, 9 dynamic).
- worker test: `cd worker && npm test` → `node --experimental-strip-types
  --test 'src/**/*.test.ts'` — 51/51 pass, ~0.4s — source: `worker/package.json`.
- worker typecheck: `cd worker && npm run typecheck` → `tsc --noEmit` — clean.
- worker deploy dry-run: `cd worker && npx wrangler deploy --dry-run` —
  succeeds, 114 KiB bundle, lists the `HELD` KV binding plus two env vars.
- contracts test: `cd contracts && forge test` → 68/68 pass (4 suites),
  ~44ms, local only (forge-std + openzeppelin vendored in `contracts/lib/`).
- contracts fmt: `forge fmt` (`foundry.toml` `[fmt] line_length = 100`) — not
  run this pass, no non-compliant files spotted by eye.
- subgraph codegen: `cd subgraph && npm run codegen` → `graph codegen` —
  succeeds, writes `subgraph/generated/`.
- subgraph build: `cd subgraph && npm run build` → `graph build` — succeeds,
  writes `subgraph/build/subgraph.yaml` plus 4 compiled `.wasm` data sources.

## Conventions

- Comments explain *why*, never *what*; `///`-style prose comments used even
  in plain TS, not just Solidity NatSpec — source: `web/src/lib/db/client.ts`,
  `shared/gateway-verdict.ts:1-14`, `contracts/src/PostageEscrow.sol`.
- File naming: kebab-case for multi-word lib modules (`wallet-proof.ts`,
  `quote-types.ts`, `claim-inbox-helpers.ts`), single lowercase word
  otherwise (`db/client.ts`, `mail.ts`, `gate.ts`); PascalCase for React
  components (`ClaimInbox.tsx`, `PickHandle.tsx`, `FinishClaim.tsx`) —
  source: `web/src/lib/*.ts`, `web/src/app/*.tsx` listing.
- Tests live beside the code they test, `*.test.ts` suffix, `node --test`
  native runner (no Jest/Vitest) — now the convention in both `web` and
  `worker` — source: `web/package.json:9`, `worker/package.json`.
- TS strict mode on, `@/*` → `src/*` path alias in `web` — source:
  `web/tsconfig.json:22`. `worker/tsconfig.json` pins `types` to
  `@cloudflare/workers-types` only, deliberately excluding `@types/node`
  (see Traps).
- Solidity: `pragma solidity 0.8.28` pinned (not `^0.8.28`), MIT license
  header, NatSpec `@notice` on contracts — source: `contracts/src/*.sol`
  headers, `contracts/foundry.toml`.
- One duplicated concept is stated once and imported, not copied — now
  extended across the worker/web boundary: the wire contract between them
  (`GatewayVerdict`/`GatewayNotice`) is declared once in `shared/` and
  imported by relative path from both sides rather than redeclared, `.ts`
  extension from `worker` (`allowImportingTsExtensions` in its tsconfig),
  no extension from `web` — source: `shared/gateway-verdict.ts`,
  `worker/src/index.ts:2`, `web/src/app/api/mail/inbound/route.ts:15`.
- Deployed contract addresses pinned in `DEPLOYMENTS.md`, mirrored in
  `web/src/lib/contracts.ts`; its ABI portion is a checked/generated
  artifact, not hand-trimmed even though most is unused — source:
  `../postage/.claude/features/sweep.md` Decisions (pre-polish, still true).

## Skills

None found. No `.claude/skills/` directory in this worktree, `../postage`,
or at the container root above both. `code-review` and `security-review` are
available as built-in (non-repo-local) skills — the latter needs a git repo
cwd and `origin/HEAD` (see Traps).

## Traps

- **An async `before()` hook that resolves through an I/O callback races
  `beforeEach`** on this Node version — a `globalThis.fetch` override set up
  that way silently never applies and the test reaches the real network. Use
  top-level `await` instead, as `web/src/app/api/challenge/deliver/route.test.ts`
  does (stub model started with a top-level `await startStubModel(...)`
  before any `beforeEach`/`test` registration).
- **No `.tsx` file can be imported by a test.** `node --experimental-strip-types`
  throws `ERR_UNKNOWN_FILE_EXTENSION` before `web/test/resolve-ts.mjs` even
  runs, and no DOM/renderer is installed. `web/src/app/Account.test.ts`,
  `PickHandle.test.ts`, and `FinishClaim.test.ts` work around it by
  extracting functions from the `.tsx` source text rather than importing it.
- The web suite reaches Anthropic through the SDK, and the SDK finds
  credentials wherever it can — `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`,
  or a logged-in profile — so **unsetting the env var is not sufficient** to
  guarantee no live call: a resolved profile can supply its own base URL
  too. `web/test/model.ts` closes this by starting a loopback stub server
  and pointing both `ANTHROPIC_BASE_URL` and `ANTHROPIC_API_KEY` at it, so
  the suite passes clean even with a real key present in the environment.
- **`forge install` stages `.gitmodules` and gitlinks even with
  `--no-commit`** — use `--no-git` instead, since `contracts/lib` is plain
  vendored directories, not real submodules in this repo (`contracts/lib/`
  itself is gitignored, but the outer repo's index/`.gitmodules` would still
  be touched without the flag) — source: `.gitignore` (`contracts/lib/`
  entry).
- **The `security-review` skill cannot run from a cwd that is not a git
  repo**, and inside this worktree its frontmatter needs `origin/HEAD`,
  which is unset — `git rev-parse --abbrev-ref origin/HEAD` → "ambiguous
  argument" — because nothing on this branch has ever been pushed (`origin`
  points at `github.com/Matgothmog/postage.git` but has no matching remote
  ref for this branch yet).
- Every test file that touches the database mints its own `mkdtemp` temp
  directory and sets `DATABASE_URL` before importing the module under test
  (e.g. `web/src/lib/db/client.test.ts:10-11`, `web/src/lib/gate.test.ts:13-14`);
  one that skips this falls back to a shared default DB file and interferes
  with concurrent test runs.
- `web/src/lib/privy.test.ts` gives each `readIdentity` test a fresh module
  instance via `await import("./privy.ts?instance=N")` because the JWKS
  cache lives for the process lifetime — sharing one instance would leak
  state between tests (`privy.test.ts:514-518`). `web/src/lib/auth.test.ts`
  freezes `Date.now` (`auth.test.ts:29,40,49`) because second-boundary
  assertions otherwise race real wall-clock time.
- `worker/src/node-test.d.ts` exists deliberately, not by omission: `worker`
  has no `@types/node` because `worker/tsconfig.json` pins `types` to
  `@cloudflare/workers-types` only, so nothing in shipped `src/` code can
  typecheck against a Node global the workerd runtime doesn't have. The
  declaration file gives `node:test`/`node:assert` narrowed types for tests
  only, deliberately under-typed to reject valid-but-unanticipated calls —
  source: `worker/src/node-test.d.ts:1-13`.
- `contracts/lib/openzeppelin-contracts/.gitmodules` exists on disk (part of
  the vendored copy) but is inert — the whole `contracts/lib/` tree is
  gitignored, so it was never a live submodule in this repo.
- `web/AGENTS.md` is Next.js boilerplate written by `next dev` — source:
  file content itself, which points at
  `node_modules/next/dist/server/lib/generate-agent-files.js`. It does not
  exist in this worktree (`find` and `git log --all -- web/AGENTS.md` both
  empty; `next dev` has never run here — the command list above only
  exercises `next build`). It exists, untracked, at
  `../postage/web/AGENTS.md`, but `git status` there stays clean for it too:
  `postage/.git/info/exclude` lists it ("local only, not part of the
  project"), and that exclude file lives in the gitdir common to both
  worktrees (`git rev-parse --git-common-dir` here resolves to
  `../postage/.git`) — so it will surface here the first time someone runs
  `next dev`, and it still won't show in `git status` when it does.
- `web/src/lib/contracts.ts` (~1200 of 1219 lines is generated ABI) looks
  like dead weight but is a checked-against-artifacts copy — do not "clean
  up" without re-verifying against `contracts/out/*.json` first.
- `.env` files exist on disk in `contracts/` and `web/` but are gitignored
  and not tracked (only `*.env.example` files are tracked).

## Conflicts

- None found. Lint, typecheck, and all three suites (255 web + 51 worker +
  68 forge = 374 tests) are green on a clean run, along with `next build`,
  `wrangler deploy --dry-run`, and subgraph `codegen`/`build`.

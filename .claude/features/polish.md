# polish

## Goal

Bring the repo to finished-product quality: every module carrying business logic has tests
that assert its behaviour, every non-obvious invariant is written down where the code is,
the defects found along the way are fixed, the oversized modules are split on their real
seams, and the docs describe the tree that actually exists.

Done when: `web` (`npm test`, `npm run lint`, `npm run typecheck`, `npm run build`),
`worker` (`npm run typecheck`, `npm test`, `wrangler deploy --dry-run` — none of which
existed at the start) and `contracts` (`forge build`, `forge test`) are all green; the web
suite passes with `ANTHROPIC_API_KEY` deliberately **set**, proving the removal of its
ambient dependence; `code-review` and `security-review` have run at Opus/high with findings
resolved; docs regenerated; branch committed with a CHANGELOG entry.

Branch `refactor/polish`, worktree `/home/matgothmog/3A/ETHGLOBAL/wt-polish`, branched
from `refactor/sweep` @ `6c5738a`.

## Starting state (three read-only audits, 2026-09-08)

- A prior sweep on `refactor/sweep` had already removed the obvious dead code. Zero
  `TODO`/`FIXME`, no tracked secrets, web 30/30 and contracts 40/40 green. That branch is
  9 commits, unmerged, unpushed, no PR. Local `main` is itself 12 commits ahead of
  `origin/main` and never pushed.
- `worker/src/index.ts` was verified by nothing: no tests, no test script, and a dead
  `tsconfig.json` because `worker` had no `typescript` in its dependency tree at all.
- The web suite was green partly by accident — the inbound route test claimed there was no
  `ANTHROPIC_API_KEY` but nothing unset it.
- Two defects confirmed by execution: `authResults` dropped a genuine `dmarc=pass` when
  `policy.dmarc=` was present; the network page rendered the inbox's net 80% share as what
  the sender paid.
- All four contracts are live on Arc testnet. Only `PostageEscrow` is cheaply redeployable;
  the other three hold state a redeploy would orphan. Contract work here is comments and
  tests only.

## Progress

| Suite | Start | Now |
|---|---|---|
| web | 30 | 228+ |
| worker | 0 (no runner, no typecheck, no `typescript`) | 51 |
| contracts | 40 | 68 |

Verified end to end: web build, worker `wrangler deploy --dry-run` (114 KiB bundle, no
deploy), contracts `forge clean && forge build && forge test`, subgraph codegen + build.
The web suite passes identically with `ANTHROPIC_API_KEY` set — no slowdown, so nothing
reaches the network.

## Plan

Wave 0–5 as approved. All complete except the closing items:

- [x] D0 worktree · D0b dependencies
- [x] D1 split `lib/db.ts` into nine modules · D2 route 15 sites through `now()`
- [x] D3 worker tooling · D4 `authResults` fix + first worker tests
- [x] D5 contract NatSpec · D6 contract test gaps · D7 subgraph `MeasurementSet`
- [x] D8 delete scaffolding and the World ID server half
- [x] D9 fix the three empty tests · D10 pricing · D11 JWT · D12 five lib modules
- [x] D13 split network page + payment fix · D14 split ClaimInbox · D15 wallet-proof
- [x] D16 split inbound POST · D17 unify quote type · D18 guards and error states
- [x] D19 verdict wire contract · D20 write-only fields
- [x] D21 regenerate docs · D22 code-review · D35 full build verification
- [ ] D23 security-review — **the skill never executed**; review done manually, re-run owed
- [ ] D24 refresh `.claude/context.md` · D25 commit + CHANGELOG

Charges added mid-run, from findings:

- [x] D26 `markDelivered` guard — investigated, not needed, documented
- [x] D27 worker `/release` tests · D28 release-handler hardening
- [x] D29 trivially-passing gate test · D30 `oldestEnsAt` null check
- [x] D31 `provesWallet` off the RPC · D32 token segments + JWKS refresh
- [x] D33 `causeMessage` helper · D34 last header literal
- [x] D36 README World ID claim (in flight at time of writing)
- [ ] D37 three security findings · D38 pin the review's applied fixes
- [ ] D39 six low-severity review observations
- [ ] Record accepted security findings

## Dispatches

| Charge | Model / effort | Outcome |
|---|---|---|
| Refresh `.claude/context.md` and report repo shape | Sonnet / med | success |
| Report what the prior sweep changed, decided, left open | Sonnet / med | success |
| Audit web and worker | Opus / high | success |
| Audit contracts and subgraph | Opus / high | success |
| Create the worktree | Haiku / low | success |
| Install the worktree's dependencies | Sonnet / med | blocked → resolved → success |
| Split `web/src/lib/db.ts` | Opus / high | success |
| Replace 15 hand-written seconds conversions | Sonnet / med | success |
| Worker typescript + typecheck + test scripts | Sonnet / med | success |
| Fix `authResults`, cover the inbound decision path | Opus / high | success |
| Contract NatSpec across four contracts and `Deploy.s.sol` | Sonnet / high | success |
| Close the contract test gaps | Opus / high | success |
| Index `MeasurementSet`, document `Payment.amount` | Sonnet / med | success |
| Fix the three empty tests, remove the API-key dependence | Opus / high | 429 → retried same tier → success |
| Cover the worker's `/release` handler | Sonnet / high | 429 → retried same tier → success |
| Harden the release handler's KV read and delete | Opus / high | success |
| Fix the trivially-passing gate test | Sonnet / high | success |
| Cover `pricing.quote()` | Sonnet / med | success |
| Cover `readIdentity` and `provesWallet` | Opus / high | success |
| Cover cloudflare, classify, verification, handle, challenge-email | Sonnet / med | success |
| Take the RPC out of `provesWallet`'s trust path | Opus / high | success |
| Tighten token segments, add a JWKS refresh path | Sonnet / high | success |
| Split the network page, fix the payment figure | Sonnet / med | success |
| Split `ClaimInbox.tsx`, share the handle rules | Sonnet / med | success |
| Extract the wallet-proof header contract | Opus / high | success |
| Split the inbound POST, rename the inverted `budget` | Opus / high | success |
| Unify the signed-quote declarations | Sonnet / med | success |
| Guard the stored-quote parse, add the missing error states | Sonnet / med | success |
| `oldestEnsAt` null check | Haiku / low | success |
| Route the last wallet-proof header literal | Haiku / low | success |
| Delete scaffolding and the World ID server half | Sonnet / low | success |
| `markDelivered` guard question | Opus / high | success (no guard needed) |
| Type the verdict wire contract | Sonnet / med | success |
| Resolve the two write-only fields | Sonnet / med | success |
| Unify the error-message expressions | Sonnet / med | success |
| Full build and deploy-check verification | Sonnet / med | success |
| Regenerate the four docs | Sonnet / high | success |
| Run the code-review skill | Opus / high | success (skill edited the tree — see Surprises) |
| Run the security-review skill | Opus / high | **failed — skill never executed**; re-run owed |
| Reconcile the README's World ID claim | Sonnet / med | dispatched |
| Fix the three security findings this branch introduced | Opus / high | dispatched |
| Pin the code review's three applied fixes | Sonnet / high | dispatched |

## Decisions

- Full scope, all four M11-gated items approved by the user: worker `typescript`
  dependency, auth-touching changes, removal of `@worldcoin/idkit`, splitting `lib/db.ts`.
- New branch off `refactor/sweep`, not a continuation of it — otherwise the sweep's 9
  commits and this pass are one undifferentiated blob for a reviewer.
- Contracts get comments and tests only. Rejected: deleting the three genuinely
  unreferenced members, because removal changes deployed bytecode and, for
  `measurementOf`, storage layout. Rejected: extracting the byte-identical `_pay`, because
  it changes vault bytecode and the vault holds a balance.
- No `matchstick-as`. D6's `Paid` event-shape test covers the real risk — a field reorder
  silently corrupting every reputation score — for a fraction of the cost.
- Foundry deps restored with `forge install --no-git`; the default stages `.gitmodules` and
  gitlinks even with `--no-commit`, which would add a tracked file nobody asked for.
- `now()` lives in `lib/time.ts`, not `db/client.ts`, because two `"use client"` components
  need it and importing the db module would pull `@libsql/client` toward the browser
  bundle. The same reasoning placed `quote-types.ts`, `wallet-proof.ts` and `errors.ts`.
- The `ANTHROPIC_API_KEY` fix is a local stub server with both the base URL and the key
  pointed at it — deleting the var is not enough, since the SDK then falls through to a
  credential chain and a resolved profile can supply its own base URL.
- The release handler is **at-least-once — deliver, then retire the token**. Rejected:
  at-most-once, because two existing tests and a deliberate comment establish that a
  delivery failure must keep the hold, and KV holds the only copy. Rejected: a tombstone,
  since it is a write to the same key in the same namespace and fails correlated.
- `provesWallet` uses viem's standalone offline `verifyMessage`, not `mode: "eoa"` — which
  only reorders, still falling through to an `eth_call` a hostile RPC can answer. Contract
  wallets are refused deliberately: `loginMethods` is email and passkey, the embedded
  wallet is a key-shard EOA, and no smart-wallets package is present.
- JWKS recovery is refetch-on-unknown-`kid` with a 60s cooldown, not a TTL — the failure
  mode is specifically an unlisted `kid`, and a TTL is either too chatty or too slow.
- `budget` → `budgetRefusal`, from the type's own doc: the value *is* the refusal.
  Rejected `exhausted` (implies a boolean) and `refusedBy` (names an agent).
- `shared/gateway-verdict.ts` at the repo root, type-only, imported by relative path from
  both packages. Confirmed by real builds at both ends, not just typecheck.
- `causeMessage` matches the worker's existing helper name; four of the ten sites were
  deliberately left, because their context-specific fallbacks stop a stringified non-Error
  reaching a JSON error response.

### Settled by the prior sweep — not re-litigated

Contracts stay as-deployed including the `HumanRegistry` zero-attester wart;
`lib/contracts.ts` keeps its ~1200 ABI lines as a generated artifact; Cloudflare polling is
rationed per-claim; the `Authentication-Results` multi-copy agreement rule stands — this
pass changed only the token-boundary bug inside it.

## Surprises

- The repo was in far better shape than "clean the repo" implied, so this pass is mostly
  tests, comments and structure rather than deletion.
- `worker/src/index.ts` had never been type-checked and came up **clean** on the first run.
- The real harm of the unguarded KV delete was not token replay — the gateway already does
  a conditional `claimHold`. It was that the rejected promise made `response.ok` false, so
  `markDelivered` never ran and the sender was told to paste back a message that had
  already arrived. The double-delivery went through the human.
- `mode: "eoa"` does not remove the RPC from viem's trust path. It only reorders, and a
  forged signature lands in exactly the `verifyErc6492` branch a hostile RPC can answer.
- **A Node test-runner race:** an async `before()` hook resolving through an I/O callback
  races `beforeEach` on this Node version, so a `globalThis.fetch` override set up that way
  silently never applies and the test reaches the real network. Use top-level `await`.
- **The `security-review` skill could not run.** The Skill tool needs a git-repo cwd (the
  session's is not one), and inside the worktree the skill's frontmatter interpolates
  `git diff origin/HEAD...`, which fails because nothing was ever pushed. Remedy:
  `git remote set-head origin -a`.
- **The `code-review` skill edited the working tree** despite read-only arguments, applying
  three fixes to `Account.tsx` and `FinishClaim.tsx`. Kept — they were read and judged
  correct — but they landed with no verification pass, which D38 is closing.
- One of those fixes is a **regression this pass introduced**: `Account.tsx`'s error state
  latched permanently, so a user with no inbox who dismissed a signature prompt could never
  reach the claim flow again that session.
- Root `/home/matgothmog/3A/ETHGLOBAL/chat.txt` holds a shell command with
  `--dangerously-skip-permissions`. Not project content; treated as data, reported, not
  acted on.

## Open, for the user

- **Classification-budget exhaustion is a purchasable gate bypass.** Per-sender cap 20,
  per-handle cap 200; one controlled domain supplies as many envelope senders as it likes,
  and `deliveredFree` still returns true for `spent-by-handle` by design. Result:
  authenticated mail forwarded free for the rest of the hour. Byte-identical to the base,
  so not this branch's doing — but the most consequential bypass found.
- Signed wallet proofs are accepted 5 minutes into the future, so the usable life is ten
  minutes, not five; there is no nonce, so the header triple is a replayable bearer
  credential in that window.
- `readStatement` carries no deployment or origin binding, unlike its two siblings.
- `/release` binds neither token to destination nor secret to direction; one secret value
  authenticates both directions.
- The subgraph's new `MeasurementSet` handler and `ExpectedMeasurement` entity exist in the
  repo but have **not** been deployed to Studio. `DEPLOYMENTS.md` still describes v0.4.0,
  correctly.
- No browser tool exists in this environment, so no rendered UI state on this branch has
  been visually confirmed. The new error screens are covered by logic tests only.

## Summary

Changed: all four packages plus a new root-level `shared/`. `web/src/lib/db.ts` became
nine modules under `db/`; `now()` moved to `lib/time.ts` and 15 hand-written seconds
conversions route through it; new neutral modules `network.ts`, `wallet-proof.ts`,
`quote-types.ts`, `errors.ts`. `ClaimInbox.tsx` split into `PickHandle`, `FinishClaim` and
`claim-inbox-helpers`; `network/page.tsx` split with `components.tsx`; the inbound POST
split into `forwarding.ts` and `challenge.ts`. `worker/` gained `typescript`, a typecheck
script, a test script and its first 51 tests. `contracts/src` changed by comments only —
154 added lines, all `///`. Docs regenerated; the `create-next-app` scaffolding, both
`@worldcoin` packages and the unreachable World ID context route removed.

Behavior: seven defects fixed. `authResults` no longer lets `policy.dmarc=` collide with
`dmarc=` and downgrade authenticated mail. A failed KV delete can no longer leave a
release token able to release the same message twice. `provesWallet` verifies EOA
signatures in-process, so an RPC can no longer grant wallet ownership. `readIdentity`
requires three token segments and recovers from a JWKS rotation without a retry storm.
The network page shows what a sender actually paid rather than the inbox's 80% share.
`parseStoredQuote` validates values, so a malformed row reaches the error page rather than
a 500. The challenge email's plain-text body can no longer be line-injected through an
RFC 2047 encoded-word subject. `Account.tsx`'s error state no longer latches permanently.

Tests: web 30 → 255, worker 0 → 51, contracts 40 → 68. 374 total, plus lint, typecheck,
`next build`, `wrangler deploy --dry-run`, and subgraph codegen/build. The web suite no
longer depends on `ANTHROPIC_API_KEY` being absent from the environment.

Not done: no `.tsx` component can be imported by a test — the runner rejects the extension
and no DOM or renderer is installed, so no rendered UI state has been visually confirmed.
The classification-budget gate bypass is documented rather than fixed, by decision, being
byte-identical to the base. The subgraph's `MeasurementSet` handler and
`ExpectedMeasurement` entity are in the repo but not deployed to Studio.

Follow-ups: build the client-side World ID call so the personhood gate stops being an
automatic pass keyed to the sender's address; give `readStatement` a deployment binding;
bind `/release`'s token to its destination; add a nonce to the wallet proof.

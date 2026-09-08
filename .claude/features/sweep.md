# sweep

## Goal

A review-and-refactor pass over the whole repo: kill duplication that can drift,
fix comments that describe code they are not attached to, close the cheap items
on KNOWN_ISSUES, and cut anything computed but never read. Done when build,
tests, typecheck and lint are green and a further pass finds nothing worth
changing.

## Plan

- [x] Round 1: shared tier table, worker auth header, db schema + budget, dead code
- [x] Round 2: the test that asserted something other than its name; one mail helper
- [x] Round 3: re-read round 1, fix the status code it got wrong, harden pagination
- [x] Round 4: name the seconds conversion; add a typecheck script
- [x] Round 5: verify the copied ABIs against the artifacts; correct KNOWN_ISSUES
- [x] Round 6: side effects out of state updaters
- [x] Round 7: converge — build, typecheck, lint, 17 node tests, 40 forge tests

## Decisions

- The worker believes an authentication result only when every copy of the header
  agrees — because the copies cannot be told apart without Cloudflare's
  authserv-id, which nobody has established. Rejected: parsing only the first
  copy, which assumes Cloudflare prepends and is the same unverified guess the
  bug came from; and refusing whenever more than one copy exists, which would
  hold a lot of legitimate relayed mail.
- The contracts are not touched. Their addresses are pinned in DEPLOYMENTS.md and
  in web/src/lib/contracts.ts, so editing the source would put it out of step
  with the deployed bytecode for no gain. The one wart found — HumanRegistry's
  constructor reverting `InvalidSignature()` for a zero attester — is recorded
  here rather than fixed.
- `web/src/lib/contracts.ts` is left as it stands. It is 1219 lines of which
  ~1200 is copied ABI, and only eight functions are called, but it was checked
  against `contracts/out/*.json` and is in sync; trimming it by hand is editing a
  generated artifact for a cosmetic win.

## Surprises

- `KNOWN_ISSUES` listed "`settleClaim` ignores `expires_at`" as a verified
  defect. It is not one. That column bounds the emailed code, which is checked
  against it when the code is entered; enforcing it again on promotion would
  strand every user who answers the code and then clicks Cloudflare's link more
  than fifteen minutes later. Both call sites now say so and the entry is gone.
- The inbound test named "an authenticated stranger cannot buy free delivery"
  passed `authenticated: false`. The branch with teeth — an authenticated sender
  who empties their own hourly slice and then writes a transactional subject —
  had never run. It does now, and it passes.
- Returning 409 for "this inbox has no wallet" made the worker tell the sender's
  server to retry, forever, over a permanent condition.

## Summary

Changed: `web/src/lib/{tiers,handle}.ts` (new), `db.ts`, `cloudflare.ts`,
`mail.ts`, `reputation.ts`, `classify.ts`, `claims.ts`, `graph.ts`,
`statements.ts`, `api/mail/inbound`, `api/inbox`, `api/challenge/deliver`,
`ClaimInbox.tsx`, `ChallengeActions.tsx`, `AutoRefresh.tsx`, `worker/src/index.ts`,
`KNOWN_ISSUES.md`.

Behavior: a sender can no longer hand the worker an authentication result that
contradicts Cloudflare's; a destination pointing back at our own domain is
refused instead of looping; an inbox with no wallet and that loop both bounce
with a reason rather than asking the sender's server to retry for days; the
destination lookup finds addresses past the fiftieth; the network page re-reads
itself as often as it says it does. Everything else is unchanged.

Tests: added the authenticated-sender half of the free-tier rule and renamed the
test that claimed to be it. 17 node tests, 40 forge tests, `next build`, `tsc`
and `eslint` all green.

Not done: World ID in the browser; `GET /api/inbox/verify` still promotes a claim
on an unauthenticated read; the long signup's code confirmation is still not
bound to a wallet; no rate limiting beyond claim emails; no scheduled job, so a
claim still strands if the tab closes; no destination deletion path.

Follow-ups: the three above marked as needing a schema change or an auth
decision; a `typescript` devDependency for the worker so it has its own
typecheck script.

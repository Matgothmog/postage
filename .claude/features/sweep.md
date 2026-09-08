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
- [x] Round 8: cap the Cloudflare polling, bind the code to its wallet, limit claims

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

- The Cloudflare question is rationed per claim, not per caller. Rejected:
  authenticating the poller, which would mean a signature prompt every four
  seconds on the one screen that is meant to need nothing; and making the route
  read-only, which leaves nothing to promote the claim and hands the user a page
  that never turns green.
- `attachDestination` counts as a check, because `ensureDestination` asked
  Cloudflare the same question moments earlier. Without it every signup spent two
  calls to learn one thing.
- Confirming a code requires the wallet the claim was started with, proved by
  Privy identity token or by signature — the same two proofs the claim itself
  takes. Checked before the attempt is counted, so a stranger cannot burn the
  real claimer's five guesses.
- Claims are limited per wallet as well as per destination, at five an hour. The
  destination limit is blind to one wallet naming a fresh address each time,
  which is exactly the shape that drains the Cloudflare destination cap.

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
- The app had no way to configure its RPC endpoint, so it used the public one in
  Arc's chain definition — shared with everyone else using that chain. Every held
  message reads `effectiveFloor` through it, and the test suite was the first
  thing to be rate limited off it. `ARC_RPC_URL` now overrides it, and the
  inbound test answers its own `eth_call` rather than reaching the network.

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

Not done: World ID in the browser; no scheduled job, so a claim still strands if
the tab closes — which is now visible, because a claim that spends its Cloudflare
budget says so and offers to start again; no destination deletion path, which is
the next thing the account cap will break; no rate limiting outside the claim
path.

Follow-ups: a `typescript` devDependency for the worker so it has its own
typecheck script; `web/src/lib/contracts.ts` could lose ~1000 lines of unused
ABI, though it was checked against the artifacts and is not stale.

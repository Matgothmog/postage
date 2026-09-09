## 2026-09-09 — refactor/polish

Tests went from 30 to 255 in web, 0 to 51 in worker — which had no test runner, no
typecheck and no `typescript` dependency at all — and 40 to 68 in contracts.
Fixed seven defects found on the way: `authResults` dropped a genuine `dmarc=pass`
whenever `policy.dmarc=` appeared beside it; a failed KV delete could leave a release
token able to release the same message twice; `provesWallet` let an RPC decide who
holds a wallet; `readIdentity` accepted tokens with junk past the signature; the
network page showed the inbox's net share as what the sender paid; a stored quote that
would not parse took the challenge page down instead of showing it; and an RFC 2047
subject could inject lines into the plain-text challenge email.
Split `lib/db.ts` into nine modules, `ClaimInbox` and the network page into components,
and the inbound POST into stages. Contracts changed by comments only — they are live,
and three of the four hold state a redeploy would orphan.

## 2026-09-08 — refactor/sweep

Review pass over the whole repo. The four tiers and the mail domain each had
several copies that could drift; both are now stated once. The worker believes an
`Authentication-Results` method only when every copy of that header agrees, so a
sender can no longer contradict Cloudflare and be believed. A destination on our
own domain is refused rather than looped, and that refusal — like an inbox with no
wallet — is a verdict with a bounce rather than a 409 that told the sender's
server to retry forever. Cloudflare's destination list is now read past the first
page. `KNOWN_ISSUES` gained one entry it was missing and lost one that was not a
bug.

Then two caps and a binding. `GET /api/inbox/verify` spent one Cloudflare API
call per anonymous request against an account-wide limit; the question is now
rationed per claim — one call every four seconds, two hundred for the life of a
claim — and a claim that spends its budget says so instead of leaving the page
spinning. `POST /api/inbox/verify` took a handle and a code and nothing else, so
mailing a stranger a code and getting them to type it completed a claim on a
wallet they had never seen; the caller now has to hold the wallet the claim was
started with. Claims are limited per wallet as well as per destination. The RPC
endpoint is configurable, because it was the public one and the tests were the
first thing rate limited off it.

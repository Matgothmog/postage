# Known issues

What is wrong with Postage as it stands, written down rather than remembered.
Everything here is either unfixed or unverified; fixed things live in the git
history instead.

Each item says how sure we are. **Verified** means it was reproduced against
running code. **Plausible** means the reasoning holds but nobody has confirmed
the behaviour, and confirming it is the first task, not fixing it.

Every item below was re-checked against the working tree at `bc9cc87` on
2026-09-09, and the ones that had gone stale say what replaced them. Line
numbers cite that commit.

## Blocking a real demo

Nothing in the mail path. A message from Gmail to a live handle has been held,
replied to in its own SMTP session, released through Mailgun and delivered
against the deployed stack.

**World ID is wired end to end on this branch. What the gate is worth depends
on `IDENTITY_MODE`, and nothing in this repo says what the deployment sets it
to.** Verified by reading, 2026-09-09. This entry used to say the browser had
never called `@worldcoin/idkit`, that the `rp_context`-signing route had been
deleted, and that `live` mode would fail outright. That was true when it was
written and is false now: it predates `bc9cc87 feat: switch World ID Selfie
Check from mock to real`, which built the half it said was missing.

`live` mode is not an automatic pass, and both halves of the handshake exist.
`ChallengeActions.verifyHuman` takes the live branch at
`web/src/app/c/[token]/ChallengeActions.tsx:106-118`: `runSelfieCheck`
(`web/src/lib/world-id.ts:216-260`) fetches a server-signed `rp_context`, opens
a real IDKit request — `IDKit.request(config).preset(selfieCheckLegacy(...))`
at `web/src/lib/world-id.ts:63` — and polls it to completion, after which
`postWorldVerify` (`:277-288`) forwards the proof and throws unless the server
answers `cleared`. `/api/world/verify` then runs four checks before anything
opens (`web/src/app/api/world/verify/route.ts:225-236`): the proof must be
present and well shaped (`:125-133`); its `signal_hash` must equal
`hashSignal(token)`, so a proof made for another challenge is refused
(`:114-119`); the signed context it was issued under must still be unspent,
which is one conditional statement and so single use (`:172-189`,
`web/src/lib/db/issued-contexts.ts:59-68`); and World's own Developer Portal
at `https://developer.world.org/api/v4` must answer with exactly one
successful `selfie` result carrying a nullifier (`:408-469`). Mail is released
only downstream of all four, through `openGate(token, "human")`
(`web/src/lib/gate.ts:43`), which that route is the only non-test caller of.
`web/src/app/api/world/context/route.ts` signs the `rp_context` and is fetched
from `web/src/lib/world-id.ts:129`; both `@worldcoin/idkit` and
`@worldcoin/idkit-server` are installed (`web/package.json:17-18`); and
`WORLD_ACTION` and `WORLD_RP_SIGNING_KEY` are documented at
`web/.env.local.example:14,20`.

`mock` mode runs none of that. `web/src/app/api/world/verify/route.ts:235` takes
`mockNullifier(challenge.sender)` — `keccak256("mock-selfie:" + sender)` at
`:474-476` — which is one free pass per *address* rather than per person, so the
Sybil resistance the free lane rests on is not switched on, and the onchain
attestation at `:266` is written for it just the same. `identityMode()`
(`web/src/lib/env.ts:13-17`) returns `"mock"` for unset or empty, so mock is
what a deployment gets unless somebody set the variable.

**Which of the two the deployed testnet runs cannot be established from this
repo, and neither can what code it is running.** `bc9cc87` is on no remote
branch — `git branch -r --contains bc9cc87` is empty — so the integration
described above is unpushed local work, and not something `origin/main` could
be serving. There is no `vercel.json`, nothing in DEPLOYMENTS.md naming a
branch or an `IDENTITY_MODE` value, and no `.env` file anywhere in the tree.
DEPLOYMENTS.md's own "Proven end to end" record — a message released "when the
sender said a person wrote it" — predates `bc9cc87`, so that release was
necessarily a mock pass; it says nothing about what is deployed now.
*Establish first* what `postage-seven.vercel.app` actually serves and what
`IDENTITY_MODE` is set to there. If the answer is mock, the paragraph above is
the live behaviour: an automatic pass keyed to the sender's address, with a
real onchain attestation written for it.

## Security

**The worker can still be handed an authentication result Cloudflare never
stated.** Plausible, and narrowed rather than closed. `Headers.get()` joins
duplicate `Authentication-Results` headers with `", "`, and a sender may write
one themselves. `authResults` now believes a method only when every copy of the
header agrees on it, so contradicting Cloudflare yields nothing where it
previously yielded the contradiction.

What is left is a message for which Cloudflare stated no result at all: there is
then nothing to disagree with. That is narrow — a forged claim only buys anything
if Cloudflare stated neither `dmarc` nor `spf`, because stating either makes the
two agree (the forgery changed nothing) or disagree (both are dropped).

*Establish first* whether Cloudflare strips attacker-supplied
`Authentication-Results` headers at all. If it does not, the complete fix is to
parse only the header bearing Cloudflare's own authserv-id, which means finding
out what that authserv-id is.

**`GET /api/inbox/verify` still promotes a claim on an unauthenticated read.**
Verified by reading, and no longer the item it was. It calls `settleClaim`, which
writes `markCloudflareVerified`, `createInbox` and `clearClaim`, so an anonymous
caller can enumerate in-flight claims by status code and drive one live.

Driving it live is the outcome the claim's own owner is waiting for, and it needs
both halves — a code only they received, and a link only they can click — so it
is not a way in. What the route did cost was one Cloudflare API call per request,
against a limit belonging to the whole account. That is now rationed per claim
rather than per request: at most one call every `CF_CHECK_INTERVAL_SECONDS`, and
at most `CF_CHECK_BUDGET` for the life of a claim. A thousand simultaneous
pollers cost what one does, and no claim can be made to answer questions forever.

What is left is the enumeration, against handles that are published on purpose.

**A verification code is recoverable from the database in seconds.** Verified:
holding both `inbox_claims` and `MESSAGE_ID_SECRET`, the six digit code behind a
stored hash was recovered by exhausting all 10^6 candidates in 3.4 seconds. Not a
new way in — anyone with both already owns the system — but the stored hash is
not a barrier. What protects a code is the server side attempt cap and, now, that
a code alone completes nothing. Never expose the hash, and do not add a client
side check against it.

**Most routes have no rate limit.** Verified by reading. The claim path now has
three: three per destination per hour, five per wallet per hour
(`web/src/app/api/inbox/route.ts:37,48`), and the Cloudflare ration above.
`/api/world/context`, which did not exist when this was written, has a fourth:
a challenge token may hold five unexpired signed contexts at a time
(`MAX_LIVE_CONTEXTS_PER_TOKEN`, `web/src/lib/db/issued-contexts.ts:24`). Under
`live` mode that ceiling is also the only bound on the expensive half of
`/api/world/verify`, since a context is spent before World or the chain is
touched (`web/src/app/api/world/verify/route.ts:232`); under `mock` mode that
route has no bound at all, and it spends real gas either way. Everything else
has none — `/api/mail/inbound` behind its shared secret,
`/api/challenge/resolve`, and the worker's `/release`, which spends a Mailgun
send.

**A release token can outlive its own release.** Verified by reading.
`retireHold` deletes the worker's KV entry only after Mailgun has already sent
the message, and retries the delete once on failure; if both attempts fail, the
token is still there even though the mail it guarded is gone. Bounded two ways:
the key still expires on the deadline set when the message was first held, so
nothing accumulates, and the web side's `claimHold` — one conditional update,
taken before `/release` is ever called — has already moved the challenge past
`delivered`, so the sender's own retry cannot ask the worker to send it again.
Documented at the call site rather than fixed: a delete that fails must not
become an error, because the mail is already out.

**Exhausting a handle's classification budget buys free delivery for
everyone else that hour.** Verified by reading
`web/src/app/api/mail/inbound/forwarding.ts` and the caps in
`web/src/lib/db/classifications.ts`. The per-sender ceiling is 20
classifications an hour and the per-handle ceiling is 200, and an
authenticated sender is not otherwise limited in how many envelope addresses
they write from, so one controlled domain can spend the whole handle pool
with ten addresses at twenty each. Past that pool the
classifier reads headers only, and `deliveredFree` still forwards a degraded
`important` verdict when the refusal is `spent-by-handle`, on the reasoning
that a drained handle pool is somebody else's doing. So the sender who drains
it themselves buys the header-only fallback for the rest of the hour, and
anything after that with a transactional-sounding subject and passing
authentication is forwarded free, skipping both the hold and the payment.
Pre-existing — byte-identical to `main` — not introduced by this branch, and
the most consequential bypass found here.

**A signed wallet proof is valid for ten minutes, not the five its name
suggests.** Verified by reading `web/src/lib/auth.ts` and the test at
`web/src/lib/auth.test.ts:75-79`. `FRESHNESS_SECONDS` is 300 and the bound
checked is `age < -FRESHNESS_SECONDS || age > FRESHNESS_SECONDS`, so a
statement timestamped up to five minutes into the future is accepted
alongside one up to five minutes into the past — a ten-minute window under a
name and comment that read as five. The future half is deliberate, for a
client whose clock runs fast, and the test asserts it rather than merely
tolerating it. There is no nonce and no single-use record anywhere the header
triple is checked, so within that window the wallet address, timestamp and
signature are a replayable bearer credential — good for whatever exact
action was signed, and no wider, because `web/src/lib/statements.ts` binds
each statement to the wallet and, where they apply, the handle or
destination.

**`readStatement` is the one signed statement with no deployment binding.**
Verified by reading `web/src/lib/statements.ts`. `claimStatement` and
`confirmStatement` both embed `postageAddress(handle)`, which carries
`MAIL_DOMAIN`; `readStatement` names only the wallet and a timestamp. It is
the statement `GET /api/inbox` checks (`web/src/app/api/inbox/route.ts:81`),
so a signature over that exact text collected anywhere else that shares the
wallet — a staging deployment, or any other site that asks someone to sign
it — is valid there too for the freshness window, and reading the inbox
discloses the handle's forwarding address. A SIWE-style domain line in the
statement would close it.

**`/release` does not check that the token and the destination belong
together, and one secret covers three different jobs.** Verified by reading
`worker/src/index.ts`. The handler reads `token` and `to` from the request
body independently and never cross-checks them — it relays whatever is held
under `token` to whatever `to` names — and the only gate is
`x-postage-secret` matching `env.POSTAGE_SECRET`. That same value is the web
app's `MAIL_WEBHOOK_SECRET`: it authenticates `POST /api/mail/inbound` from
the worker to the gateway (`web/src/app/api/mail/inbound/route.ts:78`) and
the gateway's own calls to `/release` (`web/src/lib/hold.ts:26-32`), where
`to` is normally `inbox.destination` looked up server-side rather than
attacker-supplied. Holding that one secret is enough to redirect a held
message's contents to an address of the holder's choosing through a Mailgun
relay under the account's domain — the worker enforces no relationship
between the two. Separate from `/release` having no rate limit, which the item
above already records, and unfixed.

**A sender can turn a real DKIM failure into `unknown`, and `unknown` is
accepted as far as delivery is concerned.** Plausible. `authResults` in
`worker/src/index.ts` returns `null` for a method when copies of
`Authentication-Results` disagree, and `senderIsAuthenticated` in
`web/src/app/api/mail/inbound/route.ts:37-39` reads `payload.dkim !==
"fail"`, so a `null` dkim passes that check the same as a `pass` would. A
sender who adds their own `dkim=pass` alongside Cloudflare's real
`dkim=fail` turns the header into a disagreement, and the disagreement into
`null`. Practical gain is nil as things stand: the other conjunct requires
`spf === "pass"`, which already pins the envelope domain, and on a domain
that clears SPF the DKIM signature could simply be omitted rather than
forged. The `null`-is-unknown rule the worker enforces is safe exactly as
long as no consumer treats unknown as good, and this one does.

## Correctness

**A held message that is never answered is silently dropped.** Verified by
design. To reply to a sender inside their own SMTP session the message has to be
accepted, and an accepted message tells their provider it was delivered. If they
never answer, the hold expires and nothing arrives, with no bounce to tell them
so. The alternative — refusing the session — is what happens to a sender we
cannot safely reply to, and it costs them the message unless they follow the
link. Neither is free; this trade was made deliberately, for the one that does
not ask a person to write the message twice.

**A claim strands if the tab closes.** Verified by reading. `settleClaim` is
called from the two `verify` route handlers and from the short signup, all driven
by a live request. There is no cron and no `scheduled` worker handler, and
`api/mail/inbound` never touches `inbox_claims`. Entering the code, closing the
tab, and clicking Cloudflare's link later leaves the claim unpromoted forever,
and `GET /api/inbox` reads only `inboxes`, so the user lands back at the start
with no way to resume. `clearClaim` runs only on success, so abandoned rows keep
their code hash indefinitely.

**There is no way to delete a destination.** Verified by reading. One is created
per inbox and none are ever removed, and the Cloudflare account has a cap on
them; reaching it kills signup permanently. `findDestination` now walks the
pages rather than the first fifty, so the lookup no longer fails first — which
means the cap is now the thing that will actually bite. The walk is itself
bounded at `MAX_PAGES` of `PER_PAGE` (`web/src/lib/cloudflare.ts:88,94`), so
past two thousand registered addresses the lookup starts failing again, this
time silently as a miss.

## Contracts

Four things the NatSpec pass documented rather than fixed, because all four
contracts are already live on Arc testnet and a redeploy would orphan the
registered signing key, the vault's balances, or every existing attestation.
Written down in the source at the sites named below; repeated here so a claim
about the contracts does not require reading Solidity to find.

**`EnclaveRegistry.revoke` accepts a signer that was never registered, or one
already revoked.** Verified by reading. There is no `isRegistered[signer]`
guard and no zero-address guard, so a stray owner call still succeeds and emits
`EnclaveRevoked` either way. The subgraph is built purely from events, so that
call can materialise a phantom `Enclave` entity or double-count a revocation
downstream. Permissive on purpose, per the function's own NatSpec: the owner is
trusted not to make that call.

**Two constructors take an address that must accept a plain ETH transfer, and
neither checks for it.** Verified by reading. `PostageEscrow`'s `vault_` and
`PostageVault`'s `relayer_` are both immutable; if either address cannot accept
ETH, the dependent path reverts forever — every `payToSend` at `_pay` for the
escrow, every `refillRelayer` for the vault, and in the vault's case the entire
sponsorship pool is stranded with no sweep, since `withdrawTreasury` can only
ever reach `treasuryBalance`. Documented at both constructors rather than
guarded against, since neither address can be changed after the fact anyway.

**ETH forced into `PostageVault` outside `receive()` breaks the invariant the
fuzz tests check.** Verified by reading. `treasuryBalance + sponsorshipPool ==
address(this).balance` holds only for ETH that arrives through `receive()`; ETH
sent via `selfdestruct` or received as a coinbase payment bypasses it — there is
no `fallback` — lands in the contract's balance, and stays there permanently
unaccounted for. Left unfixed deliberately: closing it would mean redeploying a
vault that already holds real funds.

**`HumanRegistry`'s constructor rejects a zero `attester_` by reverting
`InvalidSignature`, an error that names nothing about a signature.** Verified by
reading. The revert itself is correct — a zero attester would leave every
attestation unverifiable — but the error it reverts with is a leftover from an
error set fixed before this check was added. Left as the wrong name rather than
renamed: adding or renaming a constructor error changes the contract's deployed
interface.

## Unverified against the real thing

**`readIdentity` has never been run against a live Privy token.** The signature
check, the issuer and audience checks and the `linked_accounts` parse are written
against Privy's documented format, and the tests that exercise them
(`web/src/lib/privy.test.ts`) mint their own ES256 keypair and serve their own
JWKS, so what they pin is that the code matches our reading of the format — not
that the reading is right. A token that does not verify falls back to the
emailed-code path rather than failing open, so the risk is a signup quietly
taking the long way rather than a stranger getting through. Establish first that
the short path actually fires.

**One provider clearing Cloudflare's DMARC precondition says little about the
rest.** `message.reply()` works for Gmail. How many other senders qualify is
unmeasured, and every one that does not gets the bounce instead — which still
carries the link, but is not the experience this was built for.

**The `dangerous` tier has not been exercised end to end.** The classifier's
verdict cannot be forced from outside, so no real message has ever been called
dangerous on the deployed stack. The branches that act on it are run, with the
verdict stubbed: `web/src/app/api/mail/inbound/challenge.test.ts:72-77` pins
that nothing is held (`challenge.ts:63`),
`web/src/app/api/mail/inbound/forwarding.test.ts:103-115` that no pass carries
it, and `web/src/lib/gate.test.ts:97-105` that no lane delivers it. What is
unverified is the classifier reaching that verdict about real mail, not the code
that acts on it once it has.

## Smaller things

- The code is in the subject line, so it is readable from a lock screen preview
  without opening the mailbox — which is the property it exists to prove. Only on
  the long signup path, which most people will not take.
- The `expiresIn` the API returns is dropped client side, so nothing on the
  confirmation screen says how long the code is good for.
- The classifier called a plainly personal message `commercial` in testing. It is
  held either way and proving personhood still clears it for nothing, so this
  costs a real sender only if they decline to prove it — but the tier is meant to
  describe the message, and there it was wrong.
- `reset()` (`web/src/lib/db/client.ts:62`), which empties every table, is
  exported from a shipped application module rather than kept test-only. No route
  reaches it, but it sits in the app's module graph. It now refuses to run when
  `NODE_ENV` is `"production"` (`:62-64`), which `next build` and `next start`
  both set, so what is left is the export itself rather than the erasure.
- `GET /api/inbox/verify?handle=` is unauthenticated and reveals whether a handle
  is mid-claim.
- No `.tsx` file in this repo can be imported by a test. `node
  --experimental-strip-types` throws `ERR_UNKNOWN_FILE_EXTENSION` on `.tsx` before
  `web/test/resolve-ts.mjs`'s custom resolver — which only ever appends `.ts` —
  gets a chance to run, and there is no jsdom, no React Testing Library, no
  `react-test-renderer`, and no browser tool in this environment to mount a
  component even if the module did load. Confirmed with a standalone probe file
  outside the repo, not inferred. Four test files work around it rather than
  closing it — `web/src/app/Account.test.ts`, `FinishClaim.test.ts`,
  `PickHandle.test.ts`, and `web/src/app/c/[token]/ChallengeActions.test.ts` —
  all by reading the component's own source text at test time and matching a
  stable anchor string in it. Two go further: `Account.test.ts` and
  `PickHandle.test.ts` cut out one function or expression, strip the TypeScript
  with the `typescript` devDependency already installed for `tsc`, and run that
  through `new Function`, so the real current source is exercised rather than a
  hand-copy and an edited fix is tested as written. `FinishClaim.test.ts` and
  `ChallengeActions.test.ts` assert against the text itself and execute none of
  it, so what they pin is that the wiring is present, not that it behaves. Either
  way those tests break the moment the anchor they match moves or is renamed —
  they then throw rather than silently passing on nothing, which is deliberate,
  but it makes the anchor strings inside those four files load-bearing in a way
  nothing else in the suite is. No rendered output on this branch has been
  visually confirmed.
  Lifting the limitation would take a `.tsx` loader transform for the test
  runner plus jsdom and Testing Library to actually mount and assert against
  markup; offered and declined.

## Deliberately not done

**Held mail is stored for a day.** A sender must be able to say who wrote it and
have the message they already sent arrive, which means it has to still exist.
Cloudflare cannot defer an SMTP session and offers no reachable temporary
rejection, so there was no way to make the sender's own server hold it instead.
It lives in the worker's KV namespace with an expiry Cloudflare enforces,
`dangerous` mail is never held, and the value is deleted as it is released.

**A release depends on Mailgun.** Cloudflare's `send_email` refuses raw MIME
whose `From:` is not on this account, and `message.forward()` cannot be called
outside the session that received the message, so releasing one byte for byte
needs a third relay. If Mailgun is unreachable the gate still opens and the
sender is offered the paste-it-back route; the message is not lost, but it is not
the message they sent either.

**Privacy inside the server.** The destination address and the pass list are
stored in plaintext, five parties read every message, and a released one is
handled by a sixth. This was a decision, not an oversight — see
[ARCHITECTURE.md](ARCHITECTURE.md#what-privacy-would-actually-take) for what
closing it would take and why it means running the MTA inside an enclave.

**Onchain metadata.** `Paid` carries the inbox address, tier and amount, so the
ledger publishes a profile of what an inbox receives even though message ids are
keyed commitments. Stealth addresses would unlink the recipient; Arc's
confidential contracts would hide the payment outright. Neither is available yet.

## Checked and found not to be problems

Recorded so nobody spends time on them twice.

- **Undrained response bodies in `mail.ts` do not leak sockets.** Measured on
  node v23.11.1: thirty unread small bodies peaked at two sockets. The mechanism
  only bites above undici's 64 KB buffer.
- **The Cloudflare token scope is right.** Email Routing Addresses *Write*
  subsumes read.
- **`message.reply()` needs no `send_email` binding.** It is a method on the
  inbound message and names no binding; workerd accepts it with none declared.

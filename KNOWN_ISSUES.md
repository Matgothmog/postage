# Known issues

What is wrong with Postage as it stands, written down rather than remembered.
Everything here is either unfixed or unverified; fixed things live in the git
history instead.

Each item says how sure we are. **Verified** means it was reproduced against
running code. **Plausible** means the reasoning holds but nobody has confirmed
the behaviour, and confirming it is the first task, not fixing it.

## Blocking a real demo

**The Cloudflare API token has lost its Email Routing permission.** Verified.
`GET /accounts/{id}/tokens/verify` reports the token active, but every call to
`/email/routing/addresses` answers `10000 Authentication error`, three times in
a row, with the same account id that worked earlier the same day. So signup
fails at `ensureDestination` before it ever reaches Resend. A token with *Email
Routing Addresses Write* fixes it, in `web/.env.local` and in the deployment.

**`RESEND_API_KEY` has never been exercised.** The claim path dies at Cloudflare
first, so nothing has proved the key or `MAIL_FROM` work. `usepostage.com` also
has to be verified in Resend before it can mail arbitrary recipients.

**World ID is not actually integrated in the browser.** Verified.
`@worldcoin/idkit` is a dependency, `/api/world/context` signs an `rp_context`
correctly and `/api/world/verify` parses a Selfie Check result correctly, but
nothing client side ever calls either. `ChallengeActions.verifyHuman` posts a
bare wallet address. With `IDENTITY_MODE=mock` the nullifier is
`keccak256("mock-selfie:" + wallet)`, which is one free pass per *wallet*
rather than per person, so the Sybil resistance the free lane rests on is not
switched on. In `live` mode the flow would fail outright.

**Nothing else is outstanding in the environment.** Re-checked against the
deployment by exercising the code path that reads each one: `APP_URL`,
`NEXT_PUBLIC_PRIVY_APP_ID`, `DATABASE_URL`, `DATABASE_AUTH_TOKEN`,
`MAIL_WEBHOOK_SECRET`, `MESSAGE_ID_SECRET`, `CLASSIFIER_PRIVATE_KEY`,
`ANTHROPIC_API_KEY` (verdicts come back `degraded: false`, so the model really
runs), `GRAPH_QUERY_URL`, `GRAPH_API_KEY`, `WORLD_RP_ID`, `WORLD_ACTION`,
`WORLD_RP_SIGNING_KEY` and `CLOUDFLARE_ACCOUNT_ID` all answer correctly.
`IDENTITY_MODE`, `ATTESTER_PRIVATE_KEY` and `RELAYER_PRIVATE_KEY` are still
unproven, because exercising them posts a real attestation on Arc.

## Security

**The worker trusts a header an attacker may control.** Plausible, and the most
serious thing on this list. `worker/src/index.ts` `authResults` runs
`\b<method>=(\w+)` over the whole `Authentication-Results` value, and
`Headers.get()` joins duplicate headers with `", "`. If Cloudflare does not
strip an `Authentication-Results` header the sender wrote themselves, and if
Cloudflare's own header carries no `dmarc=` token, the regex can return the
attacker's `dmarc=pass`. That satisfies `senderIsAuthenticated()` and takes the
allowlist fast path — delivering with no classification and no payment, which
is exactly what commit `d732d58` set out to prevent.

*Establish first* whether Cloudflare strips attacker-supplied
`Authentication-Results` headers. If it does not, parse only the header bearing
Cloudflare's own authserv-id rather than regexing the joined string.

**`GET /api/inbox/verify` mutates state.** Verified by reading; it calls
`settleClaim`, which writes `markCloudflareVerified`, `createInbox` and
`clearClaim`. An anonymous caller can therefore enumerate in-flight claims by
status code, drive a stranger's claim live, and spend one Cloudflare API call
per request against an account-wide rate limit. The polling needs to be either
authenticated or read-only with promotion moved elsewhere.

**Confirming a code is not bound to a wallet.** Verified by reading.
`POST /api/inbox/verify` takes `{handle, code}` and nothing else, so someone who
receives an unsolicited code and enters it completes a claim whose wallet the
attacker chose: mail reaches the victim, while the attacker's wallet holds
`earnings` and `setFloorPrice`. Creating a claim now needs a wallet signature,
which makes this awkward rather than easy, but the confirm step should carry the
same proof.

**Nothing rate limits anything except claim emails.** Verified by reading. The
three-per-hour throttle added to `POST /api/inbox` covers the email bomb. Every
other route — `/api/mail/inbound` behind its shared secret, `/api/challenge/
resolve`, `/api/world/verify`, which spends real gas — has no limit at all.

## Correctness

**A claim strands if the tab closes.** Verified by reading. `settleClaim` is
called only from the two `verify` route handlers, both driven by the browser's
`setInterval`. There is no cron, no `scheduled` worker handler, and
`api/mail/inbound` never touches `inbox_claims`. So entering the code, closing
the tab, and clicking Cloudflare's link later leaves the claim unpromoted
forever, and `GET /api/inbox` reads only `inboxes`, so the user lands back at
the start with no way to resume. `clearClaim` runs only on success, so abandoned
rows keep their code hash indefinitely, which contradicts the comment on it.

**A failed forward is an unhandled exception.** Verified by reading.
`worker/src/index.ts` wraps only the call to the gateway in `try`;
`message.forward()` sits outside it. A destination Cloudflare will not accept
therefore throws rather than taking the deliberate `setReject` path that makes
the sending MTA retry.

**Nothing detects a mail loop.** Verified by reading. `api/mail/inbound` returns
`inbox.destination` without ever comparing it to the recipient it was called
for. A destination on our own domain is now refused at claim time, so this needs
an inbox that predates that check or a direct database edit — but nothing stops
it structurally, and each cycle spends a classify call, a chain read and a
challenge row.

**`settleClaim` ignores `expires_at`.** Verified by reading. The TTL is enforced
on the confirm path and skipped on both paths that promote a claim.

**`findDestination` reads one page of fifty.** Verified by reading.
`ensureDestination` falls back to it when Cloudflare reports an address already
exists, and `result_info` is ignored. One destination is created per inbox and
none are ever deleted, so past roughly fifty signups this returns null for older
addresses and signup fails with Cloudflare's raw duplicate message. Needs
pagination, and separately a deletion path — the account has a destination cap,
and reaching it kills signup permanently.

## Smaller things

- `ConfirmClaim` has no way back. A 410 or 429 leaves the user on a dead screen;
  reloading works but nothing says so.
- The poll does not abort its in-flight fetch on unmount.
- `maxLength={6}` silently truncates a pasted `is 123456` to `is 123`, which
  passes the length check and spends one of five attempts.
- The code is in the subject line, so it is readable from a lock screen preview
  without opening the mailbox — which is the property it exists to prove.
- `mail.ts` hardcodes "15 minutes" while `CODE_TTL_SECONDS` is the source of
  truth, and the `expiresIn` the API returns is dropped client side.
- Two different `ClaimState` interfaces share a name across files.

## Documentation that has drifted

- `ARCHITECTURE.md:207` still says claiming a handle "is therefore the entire
  signup". Two email confirmations ago that was true.
- The trust boundaries table lists neither `CLOUDFLARE_API_TOKEN`,
  `RESEND_API_KEY` nor `MESSAGE_ID_SECRET`.

## Deliberately not done

**Privacy inside the server.** The destination address and the allowlist are
stored in plaintext, and five parties read every message. This was a decision,
not an oversight — see
[ARCHITECTURE.md](ARCHITECTURE.md#what-privacy-would-actually-take) for what
closing it would take and why it means running the MTA inside an enclave.

**Onchain metadata.** `Paid` carries the inbox address, tier and amount, so the
ledger publishes a profile of what an inbox receives even though message ids are
keyed commitments. Stealth addresses would unlink the recipient; Arc's
confidential contracts would hide the payment outright. Neither is available
yet.

## Checked and found not to be problems

Recorded so nobody spends time on them twice.

- **Undrained response bodies in `mail.ts` do not leak sockets.** Measured on
  node v23.11.1: thirty unread small bodies peaked at two sockets. The mechanism
  only bites above undici's 64 KB buffer.
- **The Cloudflare token scope is right.** Email Routing Addresses *Write*
  subsumes read, so the comment in `.env.local.example` is accurate.

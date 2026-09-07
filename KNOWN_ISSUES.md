# Known issues

What is wrong with Postage as it stands, written down rather than remembered.
Everything here is either unfixed or unverified; fixed things live in the git
history instead.

Each item says how sure we are. **Verified** means it was reproduced against
running code. **Plausible** means the reasoning holds but nobody has confirmed
the behaviour, and confirming it is the first task, not fixing it.

## Blocking a real demo

Nothing in the mail path. A message from Gmail to a live handle has been held,
answered, released and delivered against the deployed stack.

Nothing in the environment. Every variable is confirmed and signup has been run
end to end against production: a signed claim returned `200`, the code arrived,
a wrong code decremented the allowance, the right one promoted the claim to an
inbox, and the claim row was dropped. What remains below is code, not config.

**World ID is not actually integrated in the browser.** This is still the only
thing standing between the demo and a complete story. Verified.
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
`WORLD_RP_SIGNING_KEY`, `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN` all
answer correctly.

`IDENTITY_MODE`, `ATTESTER_PRIVATE_KEY` and `RELAYER_PRIVATE_KEY` were proven by
posting a real attestation: the relayer submitted it, `isHuman` reads true
onchain, the subgraph indexed it, and gas came to 75,395 — the figure
[DEPLOYMENTS.md](DEPLOYMENTS.md) records. Sponsorship therefore works end to
end: a wallet holding nothing verified without paying.

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

**A verification code is recoverable from the database in seconds.** Verified:
holding both `inbox_claims` and `MESSAGE_ID_SECRET`, the six digit code behind a
stored hash was recovered by exhausting all 10^6 candidates in 3.4 seconds. This
is not a new way in — anyone with both already owns the system — but it does
mean the stored hash is not a barrier, and the server side attempt cap is the
only thing protecting a code. Keep it that way: never expose the hash, and do
not add a client side check against it.

**Nothing rate limits anything except claim emails.** Verified by reading. The
three-per-hour throttle added to `POST /api/inbox` covers the email bomb. Every
other route — `/api/mail/inbound` behind its shared secret, `/api/challenge/
resolve`, `/api/world/verify`, which spends real gas, and the worker's
`/release`, which spends a Mailgun send — has no limit at all.

**A held message that is never answered is silently dropped.** Verified by
design: to reply to a sender inside their own SMTP session the message has to be
accepted, and an accepted message tells their provider it was delivered. If they
never answer, the hold runs out and nothing arrives, with no bounce to tell them
so. The alternative — refusing the session — is what happens to a sender we
cannot safely reply to, and it costs them the message unless they follow the
link. Neither is free; this trade was made deliberately, for the one that does
not ask a person to write the message twice.

**`readIdentity` has never been run against a real Privy token.** The signature
check, the issuer and audience checks and the `linked_accounts` parse are
written against Privy's documented format and are exercised by nothing. A token
that does not verify falls back to the emailed-code path rather than failing
open, so the risk is a signup that quietly takes the long way rather than one
that lets a stranger through. Establish first that the short path actually
fires.

**`message.reply()` and the release both work in production.** Run end to end
against a real message from Gmail: held, replied to in the same session, and on
answering, released through Mailgun and delivered. The worker logged the inbound
and the `POST /release` thirty-three seconds apart, the hold was gone from KV
afterwards, and the challenge row read resolved with `held_until` null.

What is still unmeasured is how many senders clear Cloudflare's DMARC
precondition for a reply. One provider qualifying does not say much about the
rest, and every sender who does not gets the bounce instead, which still carries
the link.

## Correctness

**A claim strands if the tab closes.** Verified by reading. `settleClaim` is
called only from the two `verify` route handlers, both driven by the browser's
`setInterval`. There is no cron, no `scheduled` worker handler, and
`api/mail/inbound` never touches `inbox_claims`. So entering the code, closing
the tab, and clicking Cloudflare's link later leaves the claim unpromoted
forever, and `GET /api/inbox` reads only `inboxes`, so the user lands back at
the start with no way to resume. `clearClaim` runs only on success, so abandoned
rows keep their code hash indefinitely, which contradicts the comment on it.

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

- The code is in the subject line, so it is readable from a lock screen preview
  without opening the mailbox — which is the property it exists to prove. Only
  on the long path now, which most people will not take.
- `mail.ts` hardcodes "15 minutes" while `CODE_TTL_SECONDS` is the source of
  truth, and the `expiresIn` the API returns is dropped client side.
- Two different `ClaimState` interfaces share a name across files.
- The classifier called a plainly personal message `commercial` in testing. It
  is held either way and proving personhood still clears it for nothing, so this
  costs a real sender only if they decline to prove it — but the tier is meant
  to describe the message, and there it was wrong.

## Deliberately not done

**Held mail is stored for a day.** A sender must be able to say who wrote it and
have the message they already sent arrive, which means it has to still exist.
Cloudflare cannot defer an SMTP session and offers no reachable temporary
rejection, so there was no way to make the sender's own server hold it instead.
The window is one day, it lives in the worker's KV namespace rather than the
database and carries an expiry Cloudflare enforces, `dangerous` mail is never
held, and the value is deleted as it is released — see
[ARCHITECTURE.md](ARCHITECTURE.md#held-means-held).

**A release depends on Mailgun.** Cloudflare's `send_email` refuses raw MIME
whose `From:` is not on this account, and `message.forward()` cannot be called
outside the session that received the message, so releasing one byte for byte
needs a third relay. If Mailgun is unreachable the gate still opens and the
sender is offered the paste-it-back route; the message is not lost, but it is
not the message they sent either. See
[ARCHITECTURE.md](ARCHITECTURE.md#mailgun--carrying-a-release).

**Privacy inside the server.** The destination address and the allowlist are
stored in plaintext, five parties read every message, and a released one is
handled by a sixth. This was a decision, not an oversight — see
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

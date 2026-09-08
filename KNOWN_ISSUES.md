# Known issues

What is wrong with Postage as it stands, written down rather than remembered.
Everything here is either unfixed or unverified; fixed things live in the git
history instead.

Each item says how sure we are. **Verified** means it was reproduced against
running code. **Plausible** means the reasoning holds but nobody has confirmed
the behaviour, and confirming it is the first task, not fixing it.

## Blocking a real demo

Nothing in the mail path. A message from Gmail to a live handle has been held,
replied to in its own SMTP session, released through Mailgun and delivered
against the deployed stack.

**World ID is not actually integrated in the browser.** Verified, and the only
thing between the demo and a complete story. `@worldcoin/idkit` is a dependency,
`/api/world/context` signs an `rp_context` correctly and `/api/world/verify`
parses a Selfie Check result correctly, but nothing client side ever calls
either — `ChallengeActions.verifyHuman` posts a bare challenge token. With
`IDENTITY_MODE=mock` the nullifier is `keccak256("mock-selfie:" + sender)`, which
is one free pass per *address* rather than per person, so the Sybil resistance
the free lane rests on is not switched on. In `live` mode the flow would fail
outright.

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
three: three per destination per hour, five per wallet per hour, and the
Cloudflare ration above. Everything else has none — `/api/mail/inbound` behind
its shared secret, `/api/challenge/resolve`, `/api/world/verify`, which spends
real gas, and the worker's `/release`, which spends a Mailgun send.

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
them; reaching it kills signup permanently. `findDestination` now walks every
page rather than the first fifty, so the lookup no longer fails first — which
means the cap is now the thing that will actually bite.

## Unverified against the real thing

**`readIdentity` has never been run against a live Privy token.** The signature
check, the issuer and audience checks and the `linked_accounts` parse are written
against Privy's documented format and are exercised by nothing. A token that does
not verify falls back to the emailed-code path rather than failing open, so the
risk is a signup quietly taking the long way rather than a stranger getting
through. Establish first that the short path actually fires.

**One provider clearing Cloudflare's DMARC precondition says little about the
rest.** `message.reply()` works for Gmail. How many other senders qualify is
unmeasured, and every one that does not gets the bounce instead — which still
carries the link, but is not the experience this was built for.

**The `dangerous` tier has not been exercised end to end.** The classifier's
verdict cannot be forced from outside, so the branch that refuses without holding
has only been read, not run.

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

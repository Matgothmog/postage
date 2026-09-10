# Integrating World ID into Postage

Notes written while building [Postage](https://github.com/Matgothmog/postage) for
ETHOnline 2026, an email app where verified people send for free and everyone
else attaches refundable USDC postage. World ID is the free lane, so the
integration is load bearing rather than decorative.

Written as we hit each thing, not reconstructed afterwards.

## At a glance

A judge's checklist mapping each finding below to confusing / missing /
broken / hard to test, with where it's discussed. Rows are derived from the
findings that follow, not separate from them.

| Finding | Category | Where |
|---|---|---|
| v4 migration is undocumented outside the API reference; search still surfaces v2 material | confusing | :86-98 |
| RP signing key never surfaced with the same prominence as `app_id`/`rp_id`, the least discoverable required credential | missing | :100-118, :503-510 |
| Selfie Check's IDKit builder name absent from the credential page | missing | :120-133 |
| `environment: sandbox` absent from 3 of the 4 IDKit pages that define `environment` | confusing | :142-152 |
| Verify endpoint's own schema forbids the value the sandbox docs instruct you to send | broken | :154-166 |
| `allow_legacy_proofs` undocumented as required; SDK throws without it | broken | :168-184 |
| Verification-limit error carries three unrelated names across doc eras and layers | confusing | :186-196 |
| Bridge domain named three different ways; one candidate is a dead host | confusing | :198-206 |
| Portal's own MCP and REST interfaces disagree on whether `environment` exists at creation time | broken | :208-213, :537-540 |
| Archived SDK repo ranks first in search; live repo findable only via `package.json`; zero issues found for "selfie" or "sandbox" on either | missing | :222-243, :492-496 |
| Feature-flag grant and Sandbox-access grant are two independent gates, not shown as distinct states | confusing | :245-257 |
| No Portal screen, badge, or API field answers whether Selfie Check is enabled for an app | missing | :259-295, :526-530 |
| Sandbox account reset promised twice; no page names the interface | missing | :303-313 |
| `max_verifications` defaults to 1 per account, forever — indistinguishable from a broken integration on a second attempt | hard to test | :314-320, :320-370 |
| RP context signature valid 300 seconds; the clock starts before the user even sees the World App screen | hard to test | :325-332 |
| iOS semi-cold reinstall unreliable around invite codes mid-flow | hard to test | :383-388, :320-370 |
| Verify-tap produces total silence with no error surface anywhere; one of three causes (a universal-link app-id mismatch) is genuinely World's | broken, hard to test | :390-446 |
| Diagnosis throughout came from enumerating SDK exports and probing an undocumented endpoint, not from any Portal-provided debugging surface | missing | :521-547 |
| Sandbox App states (install / invite / signed-in / verified) are never named in World's own material | missing | :320-370 |

## What went well

**The nullifier design fits the problem exactly.** Postage needs one free-send
identity per person, and a per-action nullifier gives that without us holding
anything about the user. We store the nullifier against a wallet and refuse a
second wallet for the same nullifier. That was about ten lines of Solidity and
it is the whole Sybil story.

**Proof verification is unauthenticated.** Posting the IDKit result to the
verify endpoint with no API key is a good default. It meant the verification
path had no secret to leak, which mattered because the rest of our backend does
hold keys.

**Selfie Check is the right credential for this shape of product.** We are
gating a low-value action and speed matters more than strict uniqueness. Being
able to pick that trade-off, rather than being handed Orb-or-nothing, is why
the free lane is usable at all.

**The 90 day credential lifetime translated cleanly onchain.** We put it
straight into the attestation's `expiresAt`, so the free pass lapses with the
credential rather than outliving it.

**The bridge-and-polling design meant our origin never mattered.** Selfie
Check's return path is a WASM bridge that the phone polls directly
(`bridge.worldcoin.org`), not a redirect URI the Portal has to know about in
advance. There is no registered callback URL and no origin allowlist anywhere
in the Portal. That one architectural choice removed an entire category of
"which URL did you register" bugs before we ever had the chance to hit them —
we tested first over a bare LAN IP and later through an ephemeral tunnel
hostname, and World never noticed the difference.

**`sandbox-access.md` was right, and decisive, once we found it.** After two
independent lines of investigation pointed the wrong way — one arguing we
needed a second, staging-scoped action, the Portal's own action list arguing
the opposite — the sandbox page's plain statement that a sandbox proof needs
`environment: sandbox` on the client and nothing else on the server settled it
in one paragraph. It was the single most useful page in this entire
integration.

**The Developer Portal's MCP server worked on the first try.** Registering it
and asking it for our app's configuration answered questions — RP registration
status, on-chain initialization flags, the exact action list — that no public
endpoint or piece of documentation could. It is the only tool we found all
session that could see past the public precheck response into what the Portal
actually has on record.

## Friction

### The v4 migration is not reflected outside the API reference

We built the first version against v2 documentation, which is still what search
turns up. The reference page documents v4, and the differences are silent
rather than loud:

- the response field is `nullifier`, not `nullifier_hash`
- `verification_level` and `credential_type` are gone
- the endpoint takes `rp_id`, with `app_id` only for backward compatibility

Code written from the v2 docs compiles, runs, and simply never finds the
nullifier. A deprecation banner on v2 material pointing at v4 would have saved
us a rewrite.

### The RP signing key is the least discoverable requirement

Protocol 4.0 requires an `rp_context` on every proof request, signed server side
with an RP signing key. We had built and shipped a working verification path
before discovering this, because:

- the verify endpoint needs no authentication, so we concluded World ID needed
  no server secret at all
- `app_id` and `rp_id` are public and were the two values the portal surfaced
  to us most prominently
- the requirement only became visible when we read `RpContext` in the
  TypeScript definitions and found `signature`, then traced it to
  `signRequest({ signingKeyHex })` in `@worldcoin/idkit-server`

The integrate page does say the signing key should be stored as a secret. What
is missing is anything in the getting-started path saying *you need a third
credential, here is where it lives in the portal*. We would suggest listing all
three values together at RP registration, labelled by where each may appear:
`app_id` public, `rp_id` public, `signing_key` server only.

### Finding the Selfie Check preset required reading the SDK types

The credential page describes Selfie Check well but does not name the IDKit
builder. We found `selfieCheckLegacy()` by enumerating the exports of
`@worldcoin/idkit-core`. Two things were surprising once we got there:

- there is no plain `selfieCheck()`, and the neighbouring `identityCheck()` is
  document based, so the obvious guess is wrong
- it returns World ID 3.0 proofs, so the request needs
  `allow_legacy_proofs: true` and the result carries a `responses` array rather
  than a flat proof

Both are correct once known, and neither is guessable. A code sample on the
credential page would close this.

### The documentation contradicts itself in at least six places

Once the integration was finally working we went back through everything we
had read, specifically looking for what we had gotten wrong or simply missed.
Six outright contradictions turned up between pages that are each,
individually, perfectly clear.

**A. Three pages don't know the fourth one exists.**
[`idkit/integrate`](https://docs.world.org/world-id/idkit/integrate),
[`idkit/reference`](https://docs.world.org/world-id/idkit/reference), and the
[testing guide](https://docs.world.org/world-id/id/testing) all describe
`environment` as a two-value choice — `production` or `staging` — and are
explicit that `staging` means "testing with the simulator." The word "sandbox"
appears on none of them. The
[sandbox access page](https://docs.world.org/world-id/sandbox/sandbox-access),
which is the one you actually need for a real phone, tells you flatly to set
`environment: sandbox`. A developer who reads the general IDKit docs first —
the obvious reading order — has no way to know a third value exists.

**B. The sharpest one: World's own request schema forbids the value its own
sandbox instructions tell you to send.** The verify endpoint's published
schema — [`world-id/reference/api`](https://docs.world.org/world-id/reference/api),
`POST /api/v4/verify/{rp_id}` — declares `environment` as
`enum: [production, staging]`, defaulting to `production`. There is no
`sandbox` in that enum, on the exact endpoint the sandbox page instructs you to
send a sandbox proof to ("send it to the production verify endpoint"). Taken
literally, the schema says a sandbox proof is invalid input to the only verify
call that exists. It isn't — IDKit's three-way client-side connector selector
and the Portal's two-way action-scope enum are different axes entirely — but
nothing on either page says so, and we spent real time on the wrong hypothesis
(that a missing staging-scoped action was blocking us) before a fourth,
independent line of evidence ruled it out.

**C. `allow_legacy_proofs` isn't documented as required, and the type
declaration doesn't read as optional either — only the prose does.** It
appears in every code sample set to `true`, and nothing in the parameter
reference marks it as required, so the natural assumption from the docs alone
is that it can be omitted. The installed runtime says otherwise before you
even reach a request: `allow_legacy_proofs: boolean;`, with no `?`, in
`idkit-core@4.2.4`'s own config type
(`node_modules/@worldcoin/idkit-core/dist/index.d.ts:59`) — the same file
that marks `environment` optional a few lines later at `:65`
(`environment?: "production" | "staging" | "sandbox";`, the line
`web/src/lib/world-id.ts:151-158` correctly cites). `tsc` catches a missing
`allow_legacy_proofs` at compile time for anyone who lets the type flow
through; the risk is narrower than "fails at runtime instead of at a type
check" — it's that the docs give no reason to expect the field is mandatory
at all, so a caller who types the request loosely (an inline object literal
without `IDKitRequestConfig`, a spread, an `any`) only finds out from IDKit's
own runtime throw.

**D. Three unrelated names for the same failure.** Hitting a verification
limit surfaces as `exceeded_max_verifications` or `already_verified` in the
older v2-era material that search still turns up, as nothing at all in the v4
schema, and as `max_verifications_reached` compiled directly into IDKit's
shipped WASM binary as a literal enum variant. None of the three strings
appears anywhere in our own error-mapping table,
`describeWorldIdFailure` (`web/src/lib/world-id.ts:193-214`) — a real
verification-limit failure falls straight through to its generic `default`
case, indistinguishable there from any other unrecognised error code. Three
vocabularies, one event, and the one place in our own code meant to name it
doesn't recognise any of them.

**E. Nobody agrees on the bridge's own domain.** Our own network trace shows
`bridge.worldcoin.org` handling the real request-and-poll traffic during a live
run. The [`wallet-bridge`](https://github.com/worldcoin/wallet-bridge)
repository's own README names a different one — "Simple relay message of
end-to-end encrypted arbitrary payloads.
[bridge.world.org](https://bridge.world.org)" — and that domain does not
resolve at all (`getaddrinfo ENOTFOUND`). We are not asserting which is
authoritative, only that a source developers would reasonably trust points at a
dead host.

**F. The Portal's own two interfaces disagree on whether `environment` exists
at creation time.** The Developer Portal's MCP tool for creating an action,
`create_world_id_action`, takes an `environment` parameter with a two-value
enum. The REST `CreateActionRequest` schema in the same Portal's published
OpenAPI spec has no `environment` property at all. Two documented ways to do
the same operation, disagreeing on whether one of the fields exists.

None of these is large on its own. Together they read as a sandbox path
grafted onto documentation written before it existed, with nobody having gone
back through the rest of the set since. A single page cross-referencing the two
`environment` axes — IDKit's client-side connector selector and the Portal's
action-scope enum, sharing a name on purpose — would have closed most of this
at once.

### The archived repo everyone finds first

`github.com/worldcoin/idkit-js` is what a search for the SDK's GitHub issues
turns up first, and it carries a banner: "This repository was archived by the
owner on Apr 14, 2026. It is now read-only." Its own description still reads
"Building the Identity SDK," and its most recent release is
`@worldcoin/idkit@2.4.2` — a full major version behind the `^4.2.3` we actually
have installed. The live source for the SDKs everyone is running is a separate
repository, `github.com/worldcoin/idkit`, discoverable only by reading each
package's own `repository` field in its published `package.json`. GitHub gives
no redirect from the archived repo and nothing on its front page points at the
live one.

The practical cost: there is nowhere to search for whether anyone else has hit
your problem. We checked both repositories for any issue mentioning "selfie"
or "sandbox" — zero results, on either. Not "the issue is closed, here's the
fix" — the search terms return nothing, meaning every Selfie Check or sandbox
problem anyone has ever filed has gone through private support email instead,
leaving no trail for the next developer to find. We'd suggest two independent
fixes: a pinned issue on the archived repo pointing at the live one, and simply
letting the recurring sandbox and Selfie Check questions live as issues on the
live repo rather than only in an inbox.

### Two separate gates that are easy to conflate

Testing Selfie Check needs both:

1. the feature flag enabled on the application, requested through a World
   contact
2. Sandbox app access, requested in the portal with an Apple or Google account
   email

These are granted by different routes and it is not obvious they are
independent. We initially assumed the sandbox invite implied the credential was
enabled. Naming them distinctly in the docs, and showing both states somewhere
in the portal, would make it clear which one is outstanding.

### No way to inspect app configuration programmatically

We could not determine from the SDK or any documented endpoint whether Selfie
Check was enabled for our app. We eventually used what we believed at the time
to be an undocumented endpoint, `POST /api/v1/precheck/{app_id}`, which is how
we discovered a different problem entirely: our action did not exist. It
returns

    {"code":"required","detail":"No action found for this app.","attribute":"action"}

for a valid app with no actions, versus `not_found` for an unknown app. That
distinction is genuinely useful and we would happily see it documented.

*Correction, added later:* that endpoint is not actually undocumented — it now
appears in World's own OpenAPI specification as "Get Action Metadata." We
simply missed it, which is its own small lesson about how easy "undocumented"
is to conclude from one unsuccessful search. What we said above still half
stands, though: its documented response schema names nothing like
`enable_face_check`, `is_staging`, or `can_user_verify` — the fields we
actually needed. Those exist only in example payloads scattered through the
spec, never as declared schema properties anywhere we could find, so anyone
relying on them, as we did, is relying on implementation detail rather than a
contract World has committed to keeping stable.

That gap mattered on its own, separately from the missing-action problem above.
There is no Portal screen, no badge, no API field, and no distinguishing error
code that answers "is Selfie Check enabled for this app" directly.
`enable_face_check: true` in the precheck response is the only signal we ever
found for it, and it is exactly the kind of undocumented field described above
— we are trusting a value nothing promises will keep meaning the same thing.
Reading the app's full configuration through the authenticated Portal API
confirmed everything else about the app in detail — engine, status,
`is_staging`, the RP registration, on-chain sync flags — but exposed nothing
about which credentials are enabled, in either environment. A single boolean on
the app object, or a badge next to the credential name in the Portal's own UI,
would have turned what became two separate investigations, weeks apart, into
one lookup each time.

### Sandbox specifics we only learned by hitting them

A handful of sandbox behaviors are undocumented, under-documented, or scattered
enough that we're recording them together rather than let the next team
rediscover each one separately:

- **Sandbox accounts are "resettable," but no page says through what
  interface.** Both the
  [sandbox overview](https://docs.world.org/world-id/sandbox/what-is-sandbox)
  ("Resettable accounts. Delete an account and sign up again as often as you
  need.") and the
  [access page](https://docs.world.org/world-id/sandbox/sandbox-access)
  ("delete and recreate accounts freely") promise this in near-identical
  language. Neither says whether that happens inside World App itself, in the
  Developer Portal, or by writing to support. We never needed to find out
  because our test account still had its one verification left when we needed
  it — but "as often as you need" reads very differently once that isn't true.
- **`max_verifications` defaults to 1, and it applies per account, not per test
  run.** The action backing our sandbox testing — `send-free`,
  `max_verifications: 1`, `max_accounts_per_user: 1`, read directly from the
  app's own configuration — allows exactly one verification per World ID
  account, ever. A second attempt with the same sandbox account fails in a way
  indistinguishable from a broken integration unless you already know the
  limit exists.
- **That failure has three different names depending on which layer reports
  it** — see contradiction D above, worth restating here because it is
  specifically a sandbox trap: burn your one verification while testing, and
  the next error string you see could be any of three unrelated-looking ones.
- **The RP context signature is valid for exactly 300 seconds.** Documented
  plainly on
  [`idkit/signatures`](https://docs.world.org/world-id/idkit/signatures)
  (`ttl_seconds = 300`), not a surprise once read, but worth having in one
  place: if a phone takes longer than five minutes between our backend signing
  the context and World App acting on it, expect the request to fail with
  `rp_signature_expired` — and that clock starts well before the user even
  sees the World App screen.

### Test users and Sandbox App states

The production World App shows no Selfie Check option at all in this
tester's hands — the Sandbox World App build is, currently, the only
environment where this integration's flow runs end to end.

**States the Sandbox App actually goes through, as we hit them:**

1. **Not installed.** Getting the build at all requires TestFlight (iOS) or a
   private Play track (Android), granted per Apple/Google account email
   through the Portal's "World ID Sandbox" tab (see Navigation, above).
2. **Invited, not yet signed in.** Where the beta-reliability note applies:
   an invite code, unreliable specifically on an iOS semi-cold reinstall (see
   "Beta reliability note," below).
3. **Signed in, not yet verified.** A World ID account exists inside the
   Sandbox build; nothing we read or hit distinguishes this state from
   "verified."
4. **Verified.** The account has spent its one Selfie Check verification
   (see "test-user management," next, and "Verified end to end," in What we
   built).

We never saw a name for any of these states in World's own material — the
enumeration above is ours, built from what the flow actually required at
each step.

**Test-user management is a one-shot, per-account workflow, not a quota you
refill.** `max_verifications: 1` and `max_accounts_per_user: 1` apply to the
account, forever, not to a test run (see "Sandbox specifics we only learned
by hitting them," above). The only documented recourse is deleting and
recreating the account — both the sandbox overview and the access page
promise this in near-identical language, and neither names the interface it
happens through. We never had to find out, because our one test account
still had its verification unused when we needed it.

Access to a tester account is itself gated behind a real person's Apple or
Google account email, submitted once in the Portal (see "Two separate gates
that are easy to conflate," above). That makes test-user management a
named-individual workflow rather than a disposable-account one: adding a
tester means asking World to grant a specific email, not generating
throwaway credentials on demand.

<!-- AUTHOR: did you ever find where to reset/delete a sandbox test account,
or is it still unknown? If you tried and it didn't work through World App,
the Portal, or support, that's worth stating outright. -->

<!-- AUTHOR: did adding this tester's Apple/Google email to the Sandbox
require anything beyond submitting it in the Portal — an approval wait, a cap
on testers, anything else worth a line? -->

### Beta reliability note

The sandbox testing page flags that iOS semi-cold (reinstall on a new device)
is unreliable around invite codes mid-flow. We planned our demo around the hot
path because of it. Flagging that on the credential page too, not only in the
sandbox section, would help teams choose a demo path early.

### A silent failure with three separate causes

Once we had a Sandbox World App build, a corrected `environment` setting, and a
real phone in hand, tapping the verify button did nothing. No error text
appeared anywhere — not on the page, not in an obvious place in the browser
console, and nothing at all in our own server log, because every failure mode
IDKit knows about renders client-side only. Three separate bugs stood between
that silence and a working flow. We want to be precise about which of them
were actually World's to fix, because two of them were entirely ours.

The first was ours. Next.js 16 blocks a dev server's hot-reload socket for any
origin not explicitly allowed, and loading the page from a LAN address instead
of `localhost` tripped it silently — the verify button existed in the DOM, but
React had never attached to it, so there was no click handler to fire, no
fetch, and no error to show. The only trace anywhere was one line in the dev
server's own log (`Blocked cross-origin request to Next.js dev resource
/_next/hmr … add it to "allowedDevOrigins" in next.config.js`) and a failing
WebSocket connection in the browser console — nothing about World ID at all.
Adding `allowedDevOrigins` to a new `next.config.ts` fixed it outright. Nothing
World could have done differently here.

The second was also ours, or at least nobody's but the wallet SDK's. Fixing
hydration let the page mount far enough to hit a hard, unconditional throw:
`PrivyProvider` refuses to initialize an embedded wallet over plain HTTP on any
host but `localhost` or `127.0.0.1`, no config flag and no allowlist. That
crash trips Next's own error boundary before the verify button ever mounts. The
fix was a TLS tunnel in front of the dev server, and it cost nothing on World's
side, because — see "what went well" above — nothing in the Selfie Check flow
inspects the host page's origin or scheme at all.

The third is the one we think World should actually hear about, because we
don't think a developer should have to reverse-engineer it. Whatever value
`environment` carries, IDKit builds a "Continue in World App" link on a
`world.org` host for the phone to tap — ours, once everything else was fixed,
resolved to `https://sandbox.world.org/verify?…`. If `environment` had been
left unset it defaults to `production` silently (the field is optional in
`idkit-core`'s own type declaration, and the compiled JS falls back with
`config.environment ?? "production"`), and the link generated would point at
plain `world.org` instead. We checked what each of those two hosts actually
publishes for iOS universal links. `world.org`'s own
`apple-app-site-association` names `35RXKB6738.org.world.id` as the app allowed
to open its `/verify/*` links (a second app id, `org.worldcoin.insight`, is
also listed in the same file but claims a different set of paths).
`sandbox.world.org`'s own file names a completely different app —
`35RXKB6738.org.world.sandbox.id` — for that identical `/verify` path. A
Sandbox World App build is provisioned against the sandbox identifier, not the
production one, so a production-environment link is a link no app on that
phone is registered to open. iOS has nothing to show for that — no error, no
prompt, nothing — which is indistinguishable from every other silent failure in
this integration. We had already set `environment` correctly by the time we
reached a real phone, so this specific mechanism was never the actual cause of
either failure we hit; but we verified both files directly, the mechanism is
real, and nothing in the sandbox documentation warns that the two environments'
universal links point at two different, mutually exclusive apps. A tester who
adds a Sandbox build to their phone without also setting `environment` would
see exactly the silent nothing we spent a day chasing, for a completely
different reason than the one we eventually found.

## Developer Portal

Selfie Check's own docs and integration flow are covered above. This section
is about the Portal itself — the tool you use to register, read, and manage
an app — considered on its own, separate from the client SDK or the
credential docs.

### Navigation

**We reached a working onchain personhood attestation without navigating the
Portal's web UI for any World ID setting.** RP registration status, on-chain
sync flags, the exact action list — every one of those came back from a
single MCP call each (see "The Developer Portal's MCP server worked on the
first try," in What went well, above). That is itself a finding about
navigation, not an absence of one: for this integration's shape, the
structured MCP and REST surfaces made the web UI's own navigation redundant
before we ever needed to reason about where something lived in it.

The one navigation path we did use in the browser has no API or MCP
equivalent: Sandbox tester access, requested through Portal sidebar → **World
ID Sandbox** → an iOS tab (Apple Account email → TestFlight) or an Android tab
(Google Play account email → a testing link).

There is also nothing to navigate *to* for a callback URL or an origin
allowlist — see "The bridge-and-polling design meant our origin never
mattered," above. A developer used to registering redirect URIs elsewhere
will look for that screen and not find one, because the Portal has none.

<!-- AUTHOR: beyond the Sandbox tab, how many clicks/pages did it take in the
web UI to get from login to an app's World ID action list? Any confusing
nesting (team > app > action) worth naming? -->

### Search

**Every Portal fact in this document was retrieved without the Portal's own
in-app search.** RP registration, on-chain sync flags, the action list — all
of it came from either the MCP server's structured calls or direct
authenticated REST calls (see "The Developer Portal's MCP server worked on
the first try," in What went well, above). For an integration built this way,
a search box inside the Portal UI was never on the critical path: asking the
MCP tool for the app's configuration answered the question faster and more
precisely than a search would have. That is a real data point on the
in-app search's role here, not a gap in our notes.

The only search we can actually report on using is a general web search for
SDK material, and that one misfires: it surfaces the archived `idkit-js`
repository first, a full major version behind what's installed, with no
redirect to the live one (see "The archived repo everyone finds first,"
above).

<!-- AUTHOR: did the Portal's own search find "Selfie Check", a credential
list, or anything at all — or did we never use it? -->

### Product discovery

The Portal surfaces `app_id` and `rp_id` prominently — they were the two
values we saw first and most often. The RP signing key, just as required once
World ID 4.0's `rp_context` is involved, was not surfaced with the same
prominence; we found it by reading `RpContext` in the TypeScript definitions,
not from anything the Portal put in front of us (see "The RP signing key is
the least discoverable requirement," above). That is a product-discovery gap
in the literal sense: the Portal is where you'd expect to discover which
credentials an app needs, and it surfaces two of the three.

We have no record of the Portal ever presenting a browsable catalog of
available World ID credential types — Orb, Device, Selfie Check — for an app.
Everything we learned about Selfie Check specifically came from World's
public docs site, not from inside the Portal.

<!-- AUTHOR: does the Portal UI show a list of credential types you can
enable for an app, or is enabling one something you only ever did by
contacting developers@toolsforhumanity.com? -->

### Debugging guidance

Every diagnosis in this integration was assembled by us, not surfaced by the
Portal:

- Whether Selfie Check was enabled for our app was answered by no Portal
  screen, badge, or API field (see "No way to inspect app configuration
  programmatically," above) — we ended up trusting `enable_face_check` in an
  undocumented precheck response, a field that appears only in example
  payloads, never as a declared schema property.
- When the verify button did nothing on a real phone, nothing reached our own
  server log — every failure mode IDKit knows about renders client-side only
  (see "A silent failure with three separate causes," above). We found the
  actual cause by driving a real browser against the page ourselves and
  reading its console and network tab directly, not from any tool World
  provided.
- Where the Portal's own two interfaces disagree with each other — its MCP
  tool for creating an action takes an `environment` field its own REST
  schema doesn't have (contradiction F, above) — the only way we found that
  was reading both schemas side by side ourselves.

The debugging surface that existed for this integration was: enumerate the
SDK's exports, read the installed package's type declarations and compiled
source, and probe a REST endpoint that we believed at the time to be
undocumented. We found no Portal-provided logs, verification history, proof
inspector, or error-code reference anywhere in this process. That absence is
itself the finding.

<!-- AUTHOR: is there a logs, verification-history, or proof-inspector tab
anywhere in the Portal that we never opened, or did we look and find none? -->

## What we built

Proofs are verified off-chain against the Developer Portal, because the World ID
router is on World Chain and our settlement is on Arc. Our backend signs an
EIP-712 attestation of `(wallet, nullifierHash, expiresAt)`, and that is posted
to a `HumanRegistry` contract on Arc.

The attestation is deliberately submittable by anyone, since the signature names
the wallet it belongs to. That turned out to matter more than we expected: it
let a relayer post attestations on behalf of users, funded by a vault that
collects a share of postage claimed from spam. The result is that verifying
costs the user nothing at all, not even gas. A wallet holding zero balance can
verify and send.

So spam pays for verification, and World ID is what decides who is on which
side of that.

### Verified end to end

A real-phone Selfie Check proof completed successfully at
**2026-09-09T17:10:42Z**: the challenge resolved (`settled_by = human`), the
nullifier was bound, and the onchain `HumanRegistry.attest` attestation
succeeded on its first live execution — this path had only
ever run under `IDENTITY_MODE=mock` before that. `POST /api/world/verify`
returned 200 in 8.1 seconds with no error lines.

No transaction hash or explorer link was captured at the time. It has since
been recovered:
[`0x48c7b5cd756cdd017d1aa0dc83e4bcdee1ee86c7ec0a8ea47eda27fff34537ee`](https://testnet.arcscan.app/tx/0x48c7b5cd756cdd017d1aa0dc83e4bcdee1ee86c7ec0a8ea47eda27fff34537ee),
block 61264198, calling `HumanRegistry` at
`0x0F9A1C7E971df81ADC1b0335a527b30B6F136D05`, status `ok`. It was mined
2026-09-09T17:10:39Z, three seconds before the 17:10:42Z proof-success
timestamp above — consistent with the relayer submitting it the moment the
signed attestation was ready.

**Recovering it is its own finding, because our own subgraph could not
produce it.** `HumanAttestation` (`subgraph/schema.graphql`) carries no
transaction-hash or block field — unlike `Payment` in the same schema, which
has `tx: Bytes!`. Worse, the entity is upserted by wallet
(`HumanAttestation.load(event.params.wallet)` in `subgraph/src/registry.ts`),
so a later renewal overwrites `attestedAt` in place rather than appending a
new row. By the time we went looking, the closest thing the subgraph had on
record for this wallet pointed at 18:13:18Z — 63 minutes after the actual
event, because a renewal had silently taken over that field. The subgraph
was not missing the data by omission; it was actively pointing at the wrong
event.

We recovered the real hash from the Arc explorer's REST API instead, then
decoded the transaction's calldata to confirm the wallet and nullifier it
carried matched the attestation we were looking for, and reconciled the
count against the contract's total `attest` call history — 15 calls in all,
which resolve to 9 distinct wallets plus 6 renewals, consistent with the
upsert behavior above. None of that should have been necessary: storing
`event.transaction.hash` in the `HumanAttestation` mapping, the same way the
`Payment` mapping already stores `tx`, would have made this attestation
self-evidencing from the subgraph alone, the same as a payment already is.

# Integrating World ID into Postage

Notes written while building [Postage](https://github.com/Matgothmog/postage) for
ETHOnline 2026, an email app where verified people send for free and everyone
else attaches refundable USDC postage. World ID is the free lane, so the
integration is load bearing rather than decorative.

Written as we hit each thing, not reconstructed afterwards.

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
Check was enabled for our app. We eventually used the undocumented
`POST /api/v1/precheck/{app_id}`, which is how we discovered a different
problem entirely: our action did not exist. It returns

    {"code":"required","detail":"No action found for this app.","attribute":"action"}

for a valid app with no actions, versus `not_found` for an unknown app. That
distinction is genuinely useful and we would happily see it documented, ideally
alongside a field reporting which credentials are enabled. Right now the only
way to answer "is my integration ready" is to open the dashboard and look.

### Beta reliability note

The sandbox testing page flags that iOS semi-cold (reinstall on a new device)
is unreliable around invite codes mid-flow. We planned our demo around the hot
path because of it. Flagging that on the credential page too, not only in the
sandbox section, would help teams choose a demo path early.

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

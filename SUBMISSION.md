# Postage — ETHOnline 2026

Postage puts a price on strangers' email and only waives it for a verified
human.

## The problem

Email has no cost function. Sending a million messages costs the sender
approximately what sending one costs, so every mechanism that decides what
reaches an inbox has to work by guessing, and the whole burden of the guess
falls on the recipient. A filter that guesses wrong in one direction wastes
their attention; wrong in the other, it loses their mail. Nobody on the sending
side pays for either outcome.

Postage does not try to guess better. It moves the cost to the sender and lets
anyone who is actually a person skip it.

## How it works

1. You give out `you@usepostage.com` and keep the mailbox you already have.
   Cloudflare Email Routing is the MX for the domain and a catch-all rule sends
   every message to an Email Worker.
2. The worker hands the parsed fields to the gateway, which classifies the
   message into one of four tiers (`ARCHITECTURE.md:47-52`). Mail the recipient
   is waiting for — a login code, a receipt, a delivery update — is delivered at
   once, free, and never held: a login code nobody can pay for is a login code
   that never arrives. Mail that tries to deceive is never delivered at all, and
   being a person does not clear it.
3. Everything else from an unknown sender is **held** — the raw bytes go into
   Workers KV with the hold's deadline attached, and nothing about what the
   message says is written to the database (`ARCHITECTURE.md:292-317`,
   `:318-325`).
4. The sender gets a **reply to the message they just sent**, threaded to it by
   `In-Reply-To`, inside the same SMTP session that carried it. It asks one
   question with a link for each answer: did a person write this, or a machine?
   Nothing asks them to write the message again, because Postage still has it.
5. A **human** clears it for free by proving personhood with World ID Selfie
   Check. No wallet, no account, no gas — the attestation is relayed and paid
   for out of the vault.
6. A **machine** pays a small USDC fee on Arc instead. The price is not a flat
   fee: it is quoted per message from the sender's Graph-indexed history, then
   signed EIP-712 by the classifier key and enforced by `PostageEscrow`.
7. On release, Mailgun puts **the bytes that arrived** back on the wire, so the
   sender's own DKIM signature still covers the message and their address is
   still in `From:`. Nothing is appended, tagged or rewritten.

The recipient never changes email provider and never learns a new inbox. The
sender never installs anything.

Two independent signals set the price, and it is worth being exact about which
does what, because they are often conflated:

- The **Claude classifier** (`web/src/lib/classify.ts`) reads only the mail
  itself — from, to, subject, body, the SPF/DKIM/DMARC results the receiving
  MTA already computed, and the URLs in the body (`MailFacts`,
  `classify.ts:6-16`). It returns one of four tiers. It does not read Graph
  data, or anything else about the sender's onchain history.
- The **Graph-indexed history** (`web/src/lib/reputation.ts:48-68`) is read
  separately, at request time, and multiplies the tier's base price by
  deterministic arithmetic in `web/src/lib/pricing.ts:70-89`.

Two lanes, one number. The tier sets the base; indexed history moves it.

## What's live

| | |
| --- | --- |
| App | https://postage-seven.vercel.app |
| Public ledger | https://postage-seven.vercel.app/network |
| Repository | https://github.com/Matgothmog/postage |
| Subgraph query endpoint | `https://api.studio.thegraph.com/query/1758667/usepostage/v0.4.0` |
| Chain | Arc testnet, chain id 5042002 |
| Explorer | https://testnet.arcscan.app |
| Mail worker | `https://postage-mail.postage-worker.workers.dev` — `POST /release` only; a `GET` answers **404 by design**, not because the worker is down (`DEPLOYMENTS.md:106`) |

Four contracts, **deployed** (`DEPLOYMENTS.md:3-10`). Their Solidity source is
not verified on the explorer, so the addresses below show bytecode rather than
code — the claim made here is not "source-verified" but "exercised": every
revert the design depends on was fired against these live addresses, and the
accepted paths settled (`DEPLOYMENTS.md:48-63`).

| Contract | Address |
| --- | --- |
| `PostageVault` | `0xd488a385529e9eec44a17b686f3b9372071f22dc` |
| `EnclaveRegistry` | `0xf6afced17443571c79036f9d543ddbc2c5a645f9` |
| `HumanRegistry` | `0x0f9a1c7e971df81adc1b0335a527b30b6f136d05` |
| `PostageEscrow` | `0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7` |

**It has actually run.** The `/network` page is rendered from the subgraph
(`web/src/lib/network.ts:101`) and needs no sign-in, so the first two figures
below can be read straight off it rather than taken on trust:

- **1 settled payment of $0.03.** The page shows it as a single settlement row
  reading `$0.03`, beside a `Paid to inboxes` stat reading `$0.02`. That pair is
  the 80/20 split arriving rounded to cents: the exact figures are **0.024 USDC
  to the inbox's earnings and 0.006 USDC to the vault**, the split
  `PostageEscrow.payToSend` computes at
  `contracts/src/PostageEscrow.sol:188-192`, indexed and confirmed
  (`DEPLOYMENTS.md:94-98`). The split is implied by the page, not displayed as a
  split.
- **9 onchain human attestations** in `HumanRegistry` — the page's `People
  verified` stat.

Two further runs are evidenced off the page rather than on it, because the
subgraph does not carry what would prove them:

- **A real-phone World ID Selfie Check proof succeeded on 2026-09-09 at
  17:10:42Z**, from World App on a physical device: the challenge resolved with
  `settled_by = human` and the nullifier was bound. The onchain half is
  `HumanRegistry.attest`, submitted by the relayer three seconds ahead of that,
  in transaction
  [`0x48c7b5cd…37ee`](https://testnet.arcscan.app/tx/0x48c7b5cd756cdd017d1aa0dc83e4bcdee1ee86c7ec0a8ea47eda27fff34537ee)
  — block **61264198**, mined **2026-09-09T17:10:39Z**, status `ok`, calling
  `HumanRegistry` at `0x0F9A1C7E971df81ADC1b0335a527b30B6F136D05`. Its calldata
  carries a nullifier hash whose low 20 bytes *are* the wallet argument beside
  it, which is technical highlight 3's
  `address(uint160(uint256(nullifierHash)))` identity visible onchain, and an
  `expiresAt` 90 days out. (`recordPersonhood` is the **server** function that
  signs and submits this — `web/src/app/api/world/verify/route.ts:706` — not the
  contract call; the contract call is `attest`.) The subgraph cannot corroborate
  this particular attestation and never will: `HumanAttestation` carries no
  transaction hash, and it upserts by wallet, so a later renewal overwrote
  `attestedAt`. The explorer is the evidence here, not `/network`.
- A message from Gmail to a live handle went the whole way on the deployed
  stack: held in KV, replied to inside its own SMTP session, released through
  Mailgun 33 seconds later, KV empty afterwards (`DEPLOYMENTS.md:156-162`).
  Nothing about free mail is written to a chain, so this run leaves no trace on
  `/network` by design.

**What is testnet or sandbox, stated plainly.** Everything above is Arc
**testnet**, not mainnet — the USDC is testnet USDC. The World ID flow verifies
against World's Developer Portal, and the successful proof was obtained from the
**Sandbox** World App build. This is a working proof of concept, not a service
to point real mail at.

## The three prize tracks

The event allows a submission to select at most three partner prizes. These are
the three.

### The Graph — Best AI Tooling or AI Use Case with The Graph (From Scratch)

**What the track asks for**, as the prize page states it — five requirements:

1. **The Graph is load-bearing**: the agent or app uses The Graph (Subgraphs,
   the Subgraph MCP, or Substreams) as its source of blockchain data.
2. **Live data from a Graph provider** — "Mocked, local-only, or static datasets
   do not qualify."
3. **Meaningful work with the data** — "reasoning, decisions, automation, or a
   natural-language interface, not just printing a raw query result."
4. **Open-source, with a clear README or SKILL.md so judges can run it**, plus a
   **public repository** and a **two-to-four-minute demo video**.
5. **Select the pool that matches how you built** — Start Fresh for net-new
   work, Continuity for extending an existing repo or product.

Every one is answered row by row in the [requirement
matrix](#per-prize-requirement-matrix) below. What follows is the case for 1 and
3 — the two a judge has to be persuaded of rather than simply shown.

**Graph data is a hard dependency of the inbound mail path for every sender the
reputation system is about.** `senderSignals` calls
`requireConfigured("GRAPH_QUERY_URL", "GRAPH_API_KEY")` before it will price
anything (`web/src/app/api/mail/inbound/challenge.ts:34`), and an unset variable
throws straight past the `Promise.allSettled` meant to contain it. Remove
`GRAPH_QUERY_URL` or `GRAPH_API_KEY` and pricing does not degrade — it fails.

**The precise scope, because this claim is easy to state too broadly.** The call
is reached only when the sender already has a wallet on file:
`senderWallet ? await senderSignals(senderWallet) : null`
(`challenge.ts:57`). A **first-time sender with no wallet recorded never touches
The Graph at all** and is priced with `signals: null`. So the hard failure hits
exactly the population the reputation system exists for — returning senders —
which is the case The Graph is supposed to be load-bearing for in the first
place. On that path, `gatherSignals` settles its two queries rather than
throwing, so a *transient* Graph failure softens a price instead of dropping
mail (`web/src/lib/reputation.ts:46-54`); a Graph that was never configured is a
misconfiguration, and the inbound path refuses it outright rather than quoting a
price from nothing.

**Live data at request time, not a cached snapshot.** Every query goes out with
`cache: "no-store"` (`web/src/lib/graph.ts:12-18`). Both Graph surfaces are
used, through two entry points:

- `queryPostage` (`graph.ts:29-31`) reads the project's own subgraph over
  `GRAPH_QUERY_URL` — what this sender has done inside Postage.
- `queryNetwork` (`graph.ts:33-36`) reaches public subgraphs on the
  **decentralized network gateway**, at
  `https://gateway.thegraph.com/api/.../subgraphs/id/...` (`graph.ts:35`), for
  a sender with no history here. The ENS subgraph id is pinned at
  `graph.ts:5`.

**The subgraph indexes all four contracts** — `PostageEscrow`,
`EnclaveRegistry`, `HumanRegistry`, `PostageVault` (`subgraph/subgraph.yaml`) —
across seven entities including `Sender`, `Payment`, `HumanAttestation`,
`Enclave` and `ExpectedMeasurement` (`subgraph/schema.graphql`). It indexes not
only who paid but **which signing keys were ever allowed to price mail, and
under what measurement**, so the pricing authority is auditable from the same
data as the prices.

**What is actually done with the data: an automated decision, enforced
onchain.** The path is:

```
subgraph  ─▶ graph.ts:29-36  ─▶ reputation.ts:48-68  ─▶ pricing.ts:70-89
                                                            │
                                          EIP-712-signed quote
                                                            │
                                                 PostageEscrow.payToSend
```

`pricing.ts:70-89` is deterministic arithmetic over the indexed signals: a
sender with reported spam has up to 4× the inbox **floor** added to their
multiplier (`pricing.ts:71-74` — what is added is a multiple of `ONE`, the
multiplier standing for exactly one floor, *not* of the tier's own base. For the
`commercial` and `human` tiers those happen to be the same number, so the
markup behaves like "4× the base" there and would not at a tier whose base is
higher); a
sender with three or more clean paid messages is halved
(`:75-78`); an ENS name discounts 30% and one older than a year a further 20%
(`:79-88`). The result is clamped to the inbox's floor and a ceiling (`:94`),
signed, and then **enforced by the chain** — `payToSend` reverts `BelowFloor` or
`UnknownEnclave` rather than trusting the number.

`DEPLOYMENTS.md:94-98` records this working on real indexed data: after one
payment and one spam report, the sender reads `paidCount 1, spamReports 1,
spamRate 1` from the subgraph and is quoted at five times the floor instead of
the minimum.

**On the AI half, precisely.** The AI in this product is the Claude classifier
that assigns the tier, and it is a real dependency — but it reads mail facts
only (`classify.ts:6-16`). It is not fed Graph data, and this document does not
claim otherwise. The Graph's role is the other input to the same price, and it
is the one the escrow can be made to enforce.

Full trace, query by query: [`docs/the-graph.md`](docs/the-graph.md).

### World — Selfie Check

**What the track asks for**, as the prize page states it — four requirements:

1. **Uses Selfie Check**, or a Selfie Check-compatible World ID credential flow,
   **in a meaningful way**.
2. **Treats Selfie Check as a risk, eligibility, fairness, continuity, or
   abuse-prevention signal.**
3. **Includes a feedback document** covering the Selfie Check docs and
   integration flow; Developer Portal navigation, search, product discovery and
   debugging guidance; Sandbox App states, proof flows, test users, errors and
   edge cases; and what was confusing, missing, broken or hard to test.
4. **Shows a working app.**

Worth naming what the track does *not* ask: using the Sandbox World App is not a
requirement here. "Uses the World ID Sandbox App to test the project remotely"
belongs to World's other prize, **AgentKit Continuity**, which this submission
does not enter. Postage's proof happens to have been obtained from the Sandbox
build — see Known limitations for why it had to be — but that is a fact about
this integration, not a box the track asks to tick.

**Selfie Check is the abuse-prevention mechanism, not a badge.** The free lane
exists only because there is a proof of personhood behind it: if a proof could
be minted, replayed or reused, the free lane would be the cheapest bulk-mail
channel in the product. So the proof is bound three ways: to a person, to one
message, and in time.

**One nullifier, one identity — by construction, then again onchain.** The
onchain identity is derived from the nullifier itself: `identityFor` turns the
low 160 bits of the nullifier hash into a checksummed address with
`getAddress` (`web/src/app/api/world/verify/route.ts:685-687`), so one person
maps to one record before any contract check runs. `HumanRegistry` then keeps
`nullifierOwner` (`contracts/src/HumanRegistry.sol:33`) and reverts
`NullifierAlreadyBound` if a nullifier is ever presented for a different wallet
(`:67-68`). The contract's own comment is explicit that this second check is
defence-in-depth against a future caller that does not derive the address that
way (`HumanRegistry.sol:27-32`) — the off-chain derivation is what makes the
binding one-to-one today.

**A harvested proof cannot be moved onto another message.** The challenge token
is hashed into the proof as the signal, and the hash is a public input to the
zero-knowledge proof, so it cannot be edited without the proof failing at World.
`requireBoundToChallenge` compares World App's returned `signal_hash` against
`hashSignal(token)` and refuses a mismatch — or a proof carrying no
`signal_hash` at all (`web/src/app/api/world/verify/route.ts:161-166`, the
comparison at `:163`). A proof that both verifies at World and carries this hash
was made for this one challenge.

**A pass expires, and so does the credential.** Proving personhood opens a
**15-minute** window (`PASS_WINDOW_SECONDS`, `web/src/lib/db/passes.ts:5`), and
the attested credential itself is written with a **90-day** lifetime
(`CREDENTIAL_LIFETIME_SECONDS`, `web/src/app/api/world/verify/route.ts:20`)
chosen to match the Selfie Check credential's own lifetime (`:18`). Writing
again tomorrow means answering again. Personhood here is a check that somebody
was there a moment ago, not a permanent property of an address.

**Exactly one Selfie Check credential is accepted.** The verifier filters the
proof's responses for the `selfie` identifier and refuses anything that is not
exactly one such credential (`verify/route.ts:137-143`), and separately re-checks
World's response the same defensive way (`:967-975`). Selfie Check issues World
ID 3.0 proofs, which IDKit's v4 default rejects outright, so the request sets
`allow_legacy_proofs` explicitly — the constant at `web/src/lib/world-id.ts:14`,
passed into the request config at `:266`. (`selfieCheckLegacy`, imported at `:1`
and applied at `:68`, is the preset builder that binds the signal; it is not the
thing that makes a 3.0 proof acceptable.)

**The proof costs the sender nothing.** The attestation is signed by the backend
and submitted by a relayer, because `attest()` is callable by anyone — the
signature names the wallet it belongs to (`HumanRegistry.sol:56-58`). The
relayer is funded from a cut of spam payments, so a wallet holding nothing can
verify.

Required feedback document: [`docs/world-feedback.md`](docs/world-feedback.md).

### Privy — Best Financial Flow

**What the track asks for**, as the prize page states it — five requirements:

1. **Integrate Privy as a core part of the product.**
2. **Create or use at least one Privy wallet.**
3. **Complete at least one functional financial flow using a generally
   available Privy feature** — transfers, bridging, stablecoin conversions,
   swaps, self-service Earn vaults, onramps "or other supported wallet actions".
   Flows that would need commercial or guided onboarding may be mocked, but a
   mock does not satisfy this requirement.
4. **Provide a working demo and access to the project's source code.**
5. **Clearly explain how Privy improves the user experience.**

**Privy is the only authentication in the product.** There is no second login
path and no injected-wallet fallback: `PrivyProvider` is the app's root provider
and the app renders a configuration error instead of a UI when
`NEXT_PUBLIC_PRIVY_APP_ID` is absent (`web/src/app/providers.tsx:8-20`). Login
methods are email and passkey (`:26`), and an **embedded wallet is minted on
login** for anyone who does not have one —
`embeddedWallets: { ethereum: { createOnLogin: "users-without-wallets" } }`
(`providers.tsx:27-28`) — on Arc, which is set as both `defaultChain` and the
only supported chain (`:34-35`).

That matters because of who is paying. The person clicking an unlock link is a
stranger who wrote one email and has no wallet, no seed phrase and no reason to
install either.

**The financial flow is a real value transfer, not a signature.** The embedded
wallet sends a transaction carrying USDC value straight to
`PostageEscrow.payToSend`: `useSendTransaction` from Privy
(`web/src/app/c/[token]/ChallengeActions.tsx:274`), then `sendTransaction({ to:
POSTAGE_ESCROW, value: BigInt(quote.amount), data: encodeFunctionData(...
"payToSend" ...) })` (`:342-357`). **On Arc, native gas is USDC**
(`ARCHITECTURE.md:144-148`), so `value` is a stablecoin amount — the payment and
the gas that moves it are the same unit, and there is no ERC-20 `approve` step
for a first-time user to get wrong (`ARCHITECTURE.md:157-160`).

Because a broadcast transaction is not yet a mined one, settlement is polled
rather than asserted (`ChallengeActions.tsx:326-336`), and the money lands as
accrued earnings on the recipient's inbox (`PostageEscrow.sol:189`) with 20%
routed to the vault (`:188`, `:192`).

**Privy's identity token does the other half.** Claiming a handle requires
proving both that the claimer holds the wallet earnings will accrue to and that
they can read the address the handle points at. The identity token is a
short-lived JWT whose claims list the accounts Privy verified; it is checked
here against the app's public JWKS (`web/src/lib/privy.ts:63`, `readIdentity` at
`:158`) with no app secret and no call-out — so signup asks for a handle and
nothing else (`ARCHITECTURE.md:242-252`). Only `NEXT_PUBLIC_PRIVY_APP_ID` ever
reaches the browser (`ARCHITECTURE.md:375-376`).

Notes on the integration: [`docs/privy-notes.md`](docs/privy-notes.md).

## Technical highlights

### 1. A price cannot be invented — provenance-gated pricing

`PostageEscrow.payToSend` recovers the EIP-712 signer of the quote and reverts
`UnknownEnclave` unless that key is registered
(`contracts/src/PostageEscrow.sol:162-193`; the check at `:178-179`).
`EnclaveRegistry.register` binds each accepted signer to the measurement
published at the time it was registered
(`contracts/src/EnclaveRegistry.sol:67-76`, the binding at `:73`). A price
therefore cannot exist unless a key the registry accepted signed it, under the
measurement that was published at the moment it was accepted — as a precondition
the chain enforces, not a promise. What that measurement currently *names* is a
label rather than a code identity, which is the next paragraph's subject and the
limit of the claim. Quotes are single-use
(`AlreadySettled`, `:171`) and expire (`QuoteExpired`, `:172`), so a cheap quote
cannot be banked or replayed onto another message.

The honest part: the key registered today belongs to an **ordinary server
process**, and the registered measurement says so —
`keccak256("stage1-plain-classifier-not-attested")` (`DEPLOYMENTS.md:38-46`).
This is not a hardware measurement and is not described as one anywhere in the
repo. The type is `bytes32`, shaped for a Nitro enclave's PCR0; stage 2
registers a real measurement and revokes this key, with **no contract change** —
only who is allowed to sign.

### 2. DKIM survives the release, because a second relay carries it

`message.forward()` can only be called during the execution that received the
message, so a message released an hour after it arrived needs another way out —
and it must not touch the bytes. Cloudflare's own `send_email` cannot do it: it
rejects raw MIME whose envelope sender does not match the `From:` header, and
that address must be on a domain the account owns (`From: header does not
match mail from`). The only way through it is to rewrite `From:`, which is the
one edit a forward must never make (`ARCHITECTURE.md:206-219`).

So the release goes out through Mailgun's MIME endpoint, and every option in
`deliverUntouched` turns something off — `o:dkim: no`, `o:tracking: no`,
`o:tracking-clicks: no`, `o:tracking-opens: no`
(`worker/src/index.ts:220-247`, the flags at `:233-236`). Click tracking is the
one that matters: it rewrites every link in the body, and the body is exactly
what the sender's signature covers. Tracking is off at the domain level too, so
it cannot come back by someone dropping a per-message flag
(`DEPLOYMENTS.md:130-133`). Verified against the live API with a raw message
carrying a third-party domain's own DKIM signature: Mailgun accepted it on
`/messages.mime`, returned the message's own `Message-ID` rather than minting
one, and logged the sender and subject as written (`DEPLOYMENTS.md:135-140`).

### 3. Gasless pseudonymous personhood

The onchain identity of a verified person is
`address(uint160(uint256(nullifierHash)))` — an address derived from the
nullifier that **nobody holds the private key to**
(`web/src/app/api/world/verify/route.ts:355-360`). It is a name, not an account:
one person is one record by construction, and there is no wallet to link the
record back to.

Nothing is asked of that identity, including gas. `attest()` is callable by
anyone because the signature names the wallet it belongs to
(`HumanRegistry.sol:56-58`), so a relayer posts it. The relayer is funded from
`PostageVault.refillRelayer`, which **anyone may call** because the funds can
only ever move to the relayer — a keeper can top it up without gaining the
ability to move money anywhere else (`contracts/src/PostageVault.sol:82-95`).
The pool it spends from is 70% of the vault's 20% cut of each payment
(`PostageVault.sol:76-79`). Sponsoring one attestation costs 0.00188 USDC, so a
single $0.03 commercial message funds roughly **two** verifications
(`DEPLOYMENTS.md:78-82`). The `/network` page says `pays for ~8 more` beside the
same idea, and the two are not in conflict — they divide by different things:
"two" is what one message *adds*, while the page divides the **whole accumulated
sponsorship balance** by that same per-attestation cost
(`deriveAggregates`, `web/src/lib/network.ts:69`).

Spam pays for the free lane.

## How to run it

See the "Run it" section of [README.md](README.md). The four trees
(`contracts/`, `web/`, `worker/`, `subgraph/`) install and test independently —
there is no root workspace.

## Testing

| Suite | Result |
| --- | --- |
| `web` (`node --test`) | 492 passing |
| `worker` (`node --test`) | 51 passing |
| `contracts` (`forge test`) | 68 passing |
| `web` lint, typecheck, build | exit 0 |

611 tests in total. Contract coverage includes the negative paths the design
depends on: a payment with an unregistered signature reverting `UnknownEnclave`,
a quote under the floor reverting `BelowFloor`, a spam report from anyone but
the recipient reverting `NotTheRecipient`, and a quote spent twice reverting
`AlreadySettled` — each also exercised against the live deployment
(`DEPLOYMENTS.md:48-63`).

## Known limitations

Stated because a judge will find them anyway, and because the second one bounds
what the demo can show.

- **Arc testnet, not mainnet.** All four contracts are on chain 5042002. Nothing
  is deployed to Arc Mainnet (`DEPLOYMENTS.md:164-166`). The USDC amounts are
  testnet USDC.
- **The production deploy cannot serve the Selfie Check free lane. The sandbox
  preview can.** This is the sharpest limitation here, so it is stated flatly
  rather than hedged. The production site runs the **production** World
  environment, and that is forced rather than chosen: `web/next.config.ts:23-36`
  throws at build time if `VERCEL_ENV === "production"` while
  `NEXT_PUBLIC_WORLD_ENVIRONMENT` is anything other than unset or
  `"production"`, and unset itself resolves to `"production"`
  (`web/src/lib/world-id.ts:175-177`). The site builds and serves, so production
  is on production World. But **the production World App offers no Selfie Check
  option at all** today (`docs/world-feedback.md`, "Test users and Sandbox App
  states"), so the free lane cannot be completed against the production URL.
  The verified end-to-end proof, and the Selfie Check segment of the demo video,
  therefore run on a **sandbox-configured preview deploy** — which
  `next.config.ts:24` deliberately exempts from that build check, preview builds
  being the case it is written to allow. One code path, two configurations: the
  environment value is validated rather than coerced, precisely so a sandbox
  World App cannot silently request a production-targeted Selfie Check
  (`world-id.ts:166-181`). Everything else in the product — classification,
  pricing, payment, release — behaves identically on both.
- **The project's own subgraph is on Subgraph Studio**, at
  `https://api.studio.thegraph.com/query/1758667/usepostage/v0.4.0`, and is not
  published to the decentralized network. Queries for **public** data — ENS
  ownership and age — do go through the decentralized network gateway
  (`web/src/lib/graph.ts:35`).
- **Held mail is stored, for up to 24 hours.** For a sender to answer one
  question and have their original message arrive without writing it twice,
  Postage has to still have it. It lives in Cloudflare KV, in the worker that
  received it and nowhere else, written with its deadline attached so Cloudflare
  drops an unanswered one at exactly that moment
  (`ARCHITECTURE.md:304-316`). Nothing anyone wrote is in the database at all.
- **Not stored is not the same as not seen.** Five parties see a message in
  plaintext, and that is what SMTP is rather than a shortcut here
  (`ARCHITECTURE.md:382-408`). The claim "nobody can read your mail" is not made
  in this repo, because it would not be true until the MTA itself runs in an
  enclave.
- **`worker`'s `npm run typecheck` does not run in a fresh checkout.** `tsc` is
  not installed in `worker/node_modules` despite `typescript` being listed as a
  devDependency, so the script exits with `tsc: not found`. `worker`'s tests run
  and pass; only its typecheck script is affected.
- **The classifier is not attested.** See technical highlight 1 — the registered
  measurement names this state rather than hiding it.

## AI tool disclosure

The event requires that a submission "clearly document in your submission where
and how AI tools were used in the project." Two different things get conflated
under that heading, so they are separated here.

### 1. AI as a product dependency

Claude is a **runtime component of the product**, not a development tool. The
classifier at `web/src/lib/classify.ts` calls the Anthropic API with model
`claude-opus-5` (`classify.ts:143-151`; the model id at `:147`) and returns the
tier that decides whether a message is delivered free, held for personhood, held
for payment, or blocked. Its input is the mail only — from, to, subject, body,
SPF/DKIM/DMARC results and the URLs in the body (`MailFacts`,
`classify.ts:6-16`).

This is a hard dependency with a defined failure mode. If the model is
unreachable, the gateway falls back to `classifyFromHeaders`
(`classify.ts:168-192`), which reads the DMARC/SPF/DKIM results the MTA computed
plus keyword patterns in the subject and body — not the sender's domain. That
degraded path is **barred from returning the `dangerous` tier**: every branch
returns `important` or `commercial`, including the one that has just matched
phishing language (`classify.ts:165-167`, the refusal made explicit at `:181`).
Pricing then declines to charge punitively on a degraded verdict wherever one
still arrives (`web/src/lib/pricing.ts:64-68`, `ARCHITECTURE.md:68-71`) — a
wrong verdict there would both block real mail and bill for it.

The classifier does not read Graph data or any onchain history. See "How it
works" above.

### 2. AI tools used to build the project

Claude Code was used during development, selectively. Scaffolding, the test
suites and the documentation were written with its assistance. The core
architecture and the Solidity contracts were written by hand.

That is a separate statement from the one above, and the two are not
interchangeable. The classifier in section 1 is a component the product does not
run without. Claude Code is a tool that helped write the repository, and nothing
in the deployed system depends on it.

## Team

One member. The event's team cap is five and solo entries are permitted.

| | |
| --- | --- |
| Mattéo Pecher | sole author — every commit in the repository |

## Submission checklist

| Event requirement | Status |
| --- | --- |
| Submitted before 2026-09-13, 12:00 EDT | Pending |
| At most 3 partner prizes selected — The Graph, World, Privy | Done |
| From Scratch / Classic: all work begun after the Sep 4 start | Done — first commit `920b477`, 2026-09-05 11:23:45 +0200 |
| Granular commit history, no large single commit | Done — 80-plus commits across the event week, one author, nothing squashed |
| Demo video: 2–4 min, ≥720p, no phone recording, no AI voiceover | **Outstanding** — script at [`docs/demo-script.md`](docs/demo-script.md) |
| AI tool disclosure in the submission | Done — see above |
| Public repository | Done — https://github.com/Matgothmog/postage |

## Per-prize requirement matrix

One row per requirement, in the words each prize page uses, with the evidence
against it. The argument behind each row is in the track sections above; this is
the box-ticking view.

### The Graph — Best AI Tooling or AI Use Case with The Graph (From Scratch)

| Stated requirement | Evidence | Status |
| --- | --- | --- |
| The Graph is a load-bearing part of the project — the app uses Subgraphs as its source of blockchain data | Every sender with a wallet on file is priced from subgraph-indexed history, and the path refuses to quote without it (`web/src/app/api/mail/inbound/challenge.ts:34`, `:57`; `web/src/lib/reputation.ts:48-68`) | **Met**, with the scope stated in the track section: a first-time sender with no wallet on file is priced with `signals: null` and does not reach The Graph |
| Live data from a Graph provider; "mocked, local-only, or static datasets do not qualify" | Two live endpoints, every query `cache: "no-store"` (`web/src/lib/graph.ts:12-18`): Subgraph Studio for the project's own subgraph (`:29-31`), the decentralized network gateway for ENS (`:33-36`, id pinned at `:5`) | **Met** |
| Meaningful work with the data — "reasoning, decisions, automation … not just printing a raw query result" | The indexed signals drive deterministic markup/discount arithmetic into a price (`web/src/lib/pricing.ts:70-94`), which is signed EIP-712 and then enforced onchain by `PostageEscrow.payToSend`. Observed live: `paidCount 1, spamReports 1` quoted at 5× the floor (`DEPLOYMENTS.md:94-98`) | **Met** |
| Open-source with a clear README or SKILL.md so judges can run it | MIT [`LICENSE`](LICENSE); [`README.md`](README.md) carries a per-tree "Run it" section and a "What you can run without credentials" section | **Met** |
| Public repository | https://github.com/Matgothmog/postage | **Met** |
| Demo video, two to four minutes | Script at [`docs/demo-script.md`](docs/demo-script.md), 3:40 | **Outstanding — not yet recorded** |
| "Select the pool that matches how you built" | **Start Fresh** (net-new). First commit 2026-09-05, one day after the event opened; no pre-existing project-specific code — see the From Scratch declaration below | **Met** |

### World — Selfie Check

| Stated requirement | Evidence | Status |
| --- | --- | --- |
| Uses Selfie Check, or a compatible World ID credential flow, in a meaningful way | The free lane is gated on a Selfie Check proof bound to the challenge token as its signal; exactly one `selfie` credential is accepted, checked on the incoming proof and again on World's response (`web/src/app/api/world/verify/route.ts:137-143`, `:161-166`, `:967-975`) | **Met** |
| Treats Selfie Check as a risk, eligibility, fairness, continuity, or abuse-prevention signal | Abuse prevention, structurally rather than decoratively: without a sound proof the free lane would be the cheapest bulk-mail channel in the product. The proof is bound to a person (nullifier → identity, `HumanRegistry.sol:33`, `:67-68`), to one message (signal hash, `verify/route.ts:161-166`), and in time (15-minute pass, 90-day credential) | **Met** |
| Includes a feedback document — Selfie Check docs and integration flow, Developer Portal navigation/search/discovery/debugging, Sandbox App states, proof flows, test users, errors and edge cases, and what was confusing, missing, broken or hard to test | [`docs/world-feedback.md`](docs/world-feedback.md) | **Met** |
| Shows a working app | https://postage-seven.vercel.app, plus a real-phone proof that succeeded 2026-09-09 and attested onchain in [`0x48c7b5cd…37ee`](https://testnet.arcscan.app/tx/0x48c7b5cd756cdd017d1aa0dc83e4bcdee1ee86c7ec0a8ea47eda27fff34537ee) | **Met, with one caveat stated in Known limitations**: the Selfie Check lane runs on the sandbox-configured preview deploy, because the production World App offers no Selfie Check today |

### Privy — Best financial flow

| Stated requirement | Evidence | Status |
| --- | --- | --- |
| Integrate Privy as a core part of the product | The only authentication path in the product — no second login and no injected-wallet fallback. `PrivyProvider` is the root provider and the app renders a configuration error rather than a UI without `NEXT_PUBLIC_PRIVY_APP_ID` (`web/src/app/providers.tsx:8-20`) | **Met** |
| Create or use at least one Privy wallet | An embedded wallet is minted on login for anyone without one (`providers.tsx:27-28`). The wallet that settled the live payment is **`0xdd769553802be81d4eb1f8588de4c120d318b38e`** — the `from` of the `payToSend` transaction the ledger shows, [`0xd6a36eae…b7b5`](https://testnet.arcscan.app/tx/0xd6a36eae30aafc30d13a0d8c80563077c875c25e4a9bfcf8e32ed2ec2149b7b5), 0.03 USDC into `PostageEscrow`. See [`docs/privy-notes.md`](docs/privy-notes.md) | **Met** |
| Complete at least one functional financial flow using a generally available Privy feature | A value transfer, not a signature: Privy's `useSendTransaction` sends USDC value into `PostageEscrow.payToSend` (`web/src/app/c/[token]/ChallengeActions.tsx:274`, `:342-357`) — a supported wallet action, settled onchain and indexed. Nothing about this flow is mocked | **Met** |
| Provide a working demo and access to the project's source code | https://postage-seven.vercel.app and https://github.com/Matgothmog/postage | **Source met; video outstanding** — script at [`docs/demo-script.md`](docs/demo-script.md) |
| Clearly explain how Privy improves the user experience | The Privy section above, and [`docs/privy-notes.md`](docs/privy-notes.md) | **Met** |

### From Scratch declaration

The event's Classic-track rule is that "all work on your project must begin
after the hackathon officially starts. Any prior project-specific code, designs,
or assets are not allowed unless they're from public libraries or starter kits."

Postage complies. The repository's first commit is `920b477` at
**2026-09-05 11:23:45 +0200**, one day after the event opened on 2026-09-04.
Everything after it is by the same **single author** — 80-plus commits across
the event week, with the exact count inspectable at the public repository rather
than asserted here. No commit predates the event start, no code was imported
from a prior project, and the history was not squashed — the granularity the
event's version-control rule asks for is intact and inspectable.

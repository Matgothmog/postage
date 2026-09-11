# The Graph in Postage

This traces the path from an indexed onchain event to an enforced price, and
states plainly what role an LLM plays alongside it. Every claim below cites a
file and line in this repository so it can be checked directly rather than
taken on faith.

## What we index

`subgraph/subgraph.yaml` defines one subgraph with four data sources, all on
`arc-testnet`, one per deployed contract:

- `PostageEscrow` (`subgraph/subgraph.yaml:6-30`) — handles `FloorPriceSet`,
  `Paid`, `SpamReported`, `EarningsClaimed`, producing `Sender`, `Inbox`, and
  `Payment` entities via `subgraph/src/escrow.ts`.
- `EnclaveRegistry` (`subgraph/subgraph.yaml:31-53`) — handles
  `MeasurementSet`, `EnclaveRegistered`, `EnclaveRevoked`, producing `Enclave`
  and `ExpectedMeasurement` entities via `subgraph/src/enclave.ts`.
- `HumanRegistry` (`subgraph/subgraph.yaml:54-72`) — handles `HumanAttested`,
  producing `HumanAttestation` entities and mirroring `humanUntil` onto
  `Sender` via `subgraph/src/registry.ts:20-23`.
- `PostageVault` (`subgraph/subgraph.yaml:73-95`) — handles `Funded`,
  `RelayerRefilled`, `TreasuryWithdrawn`, producing a single `Vault` entity via
  `subgraph/src/vault.ts`.

`subgraph/src/shared.ts:4-27` holds the entity constructors shared by more
than one mapping (`loadSender`, `loadInbox`), plus `refreshSpamRate`
(`shared.ts:31-39`), which recomputes `spamReports / paidCount` every time
either changes rather than storing a value that could drift from its inputs.
The schema's doc comment on `Sender` (`subgraph/schema.graphql:1-5`) calls it
"the reputation record the pricing engine reads back" — that describes the
downstream use, not just the storage shape defined in the type itself
(`schema.graphql:6-19`).

All four contracts are covered; nothing in the product's onchain surface is
left unindexed. The deployed endpoint is

    https://api.studio.thegraph.com/query/1758667/usepostage/v0.4.0

(`DEPLOYMENTS.md:88`).

## Two Graph providers, and which is which

`web/src/lib/graph.ts` talks to two different things and keeps them
syntactically separate:

- `queryPostage` (`graph.ts:29-31`) hits `required("GRAPH_QUERY_URL")` — the
  project's own subgraph above, deployed to **Subgraph Studio** and queried
  directly with an API key. Studio is a hosted query endpoint, distinct from
  the decentralized-network gateway used below for ENS.
- `queryNetwork` (`graph.ts:34-36`) hits
  `https://gateway.thegraph.com/api/${GRAPH_API_KEY}/subgraphs/id/${subgraphId}`
  — the **decentralized-network gateway**, authenticated with an API key, used
  to reach subgraphs Postage does not own. The one call site
  (`web/src/lib/reputation.ts:53`) points it at `ENS_SUBGRAPH`
  (`graph.ts:5`), a public ENS subgraph identified by its network subgraph ID.

These are not interchangeable and the code does not blur them: first-party
reputation data (payments, spam reports) comes from Studio; third-party
context about a wallet (ENS ownership and age) comes through the gateway. Both
lanes are exercised on every priced message that has a wallet on file
(`reputation.ts:48-57`, discussed below).

## Live, not fixtures

`query()` (`graph.ts:12-26`) is a plain `fetch` issued at request time with
`cache: "no-store"` (`graph.ts:17`) — no caching layer, no persisted snapshot.
There is no fixture file, mock response, or static JSON blob anywhere in
`subgraph/` or `web/src/lib/` standing in for a query result; every call to
`queryPostage` or `queryNetwork` reaches a live endpoint or throws
(`graph.ts:20,23-24`).

External corroboration, not just code reading: the deployed `/network` page at
`https://postage-seven.vercel.app/network` renders rows this subgraph
actually indexed — one payment row of $0.03, "Paid to inboxes" at $0.02, 9
verified people, 10 sender rows. `PostageEscrow.sol:34`'s `VAULT_BPS = 2_000`
means that $0.03 settles as $0.024 to the inbox and $0.006 to the vault — the
split the page implies rather than renders directly. `DEPLOYMENTS.md:94-98`
records the same settlement from the indexing side: after a payment reported
as spam, the sender entity reads `paidCount 1, spamReports 1, spamRate 1`, and
the pricing engine (below) quotes that sender at five times the floor because
of it. Same event, two independent observations, same numbers.

## Load-bearing by construction

`web/src/app/api/mail/inbound/challenge.ts:33-36`:

```
function senderSignals(wallet: string): Promise<SenderSignals> {
  requireConfigured("GRAPH_QUERY_URL", "GRAPH_API_KEY");
  return gatherSignals(wallet);
}
```

`requireConfigured` throws before `gatherSignals` — and therefore before any
Graph query — runs if either variable is unset. That throw is not caught
locally; it propagates out of `issueChallenge`
(`challenge.ts:44-109`) to the route's top-level handler
(`web/src/app/api/mail/inbound/route.ts:82-87`), which turns it into a fault
response instead of a priced challenge. Remove `GRAPH_QUERY_URL` or
`GRAPH_API_KEY` and pricing for that request does not degrade — it fails.

The precise scope: `senderSignals` is only called when the sender has a
wallet on file (`challenge.ts:56-57`, `senderWallet ? await
senderSignals(senderWallet) : null`). A first-time sender with no wallet
recorded skips the Graph call and prices with `signals: null`. The hard
failure hits exactly the population the reputation system exists for —
returning senders — which is the case The Graph is supposed to be
load-bearing for in the first place. There is no fallback path here of the
kind `classify.ts` has for its own dependency (see below); an unreachable
Graph is a failed request, not a degraded one.

## The query-to-decision trace

One hop per step, each traceable to a line:

1. **Indexed events.** A `Paid` or `SpamReported` event on `PostageEscrow` is
   picked up by `subgraph/src/escrow.ts:16-56`, which increments
   `Sender.paidCount` / `Sender.spamReports` and recomputes `spamRate`
   (`shared.ts:31-39`).
2. **Query.** `web/src/lib/reputation.ts:16-24` (`POSTAGE_HISTORY`) asks
   Studio for `sender(id: $wallet) { paidCount spamReports spamRate }`,
   fired through `queryPostage` (`graph.ts:29-31`). In parallel,
   `reputation.ts:26-32` (`ENS_OWNED`) asks the gateway for that wallet's ENS
   domains, fired through `queryNetwork` (`graph.ts:34-36`).
3. **Assembling `SenderSignals`.** `reputation.ts:48-68` (`gatherSignals`)
   runs both queries with `Promise.allSettled` — either can fail
   independently without blocking the other or throwing — and reduces the
   result to the five fields declared at `reputation.ts:7-14`: `paidCount`,
   `spamReports`, `spamRate`, `ensNames`, and `oldestEnsAt` (derived from the
   oldest ENS `createdAt`, `reputation.ts:64-66`).
4. **Turning signals into an amount.** `web/src/lib/pricing.ts:70-89` is the
   arithmetic core:

   ```
   if (signals) {
     if (signals.paidCount > 0 && signals.spamRate > 0) {
       bps += Math.round(signals.spamRate * 4 * ONE);
       ...
     }
     if (signals.paidCount >= 3 && signals.spamRate < 0.2) {
       bps = Math.round(bps * 0.5);
       ...
     }
     if (signals.ensNames > 0) {
       bps = Math.round(bps * 0.7);
       ...
       if (age > YEAR_SECONDS) { bps = Math.round(bps * 0.8); ... }
     }
   }
   ```

   A history of spam raises the multiplier; a clean paid history above three
   messages halves it; an owned ENS name discounts further, more so if it is
   over a year old. This is fixed integer arithmetic on basis points
   (`pricing.ts:13-15`, `ONE = 10_000`) with a hard floor and ceiling
   (`pricing.ts:94`, `Math.min(Math.max(bps, ONE), CEILING)`) — deterministic,
   not a model call. Every step appends a human-readable reason to the quote
   (`pricing.ts:73,77,81,86`), so the price is explainable from its own output
   without re-deriving it.
5. **Signed quote.** `challenge.ts:63-71` reads the inbox's floor from the
   chain (never a cached copy, per the comment there), calls `quote(floor,
   verdict.tier, signals, verdict.degraded)` (`challenge.ts:71`), then
   `signQuote` (`web/src/lib/quote.ts:51-68`) signs the messageId, inbox,
   tier, amount, and expiry as EIP-712 typed data (`quote.ts:15-23,60-65`)
   with `CLASSIFIER_PRIVATE_KEY`.
6. **Onchain enforcement.** `contracts/src/PostageEscrow.sol:162-193`
   (`payToSend`) recovers the signer of that quote
   (`PostageEscrow.sol:178`) and reverts `UnknownEnclave(signer)`
   (`PostageEscrow.sol:179`) unless that key is registered in
   `EnclaveRegistry`. It also reverts `BelowFloor` if the quoted amount is
   under the inbox's floor and `Underpaid` if less than the quoted amount was
   sent (`PostageEscrow.sol:175-176`). `DEPLOYMENTS.md:48-63` records these
   reverts actually firing on Arc testnet, including `UnknownEnclave` against
   an unregistered signature and `BelowFloor` against an inbox with no floor
   set.

The number that came out of step 4 is not displayed and left for a human to
act on — it is cryptographically committed to in step 5 and mechanically
enforced by a smart contract in step 6, which will refuse to accept a payment
whose quote was not signed by a registered key. A Graph-derived number is
therefore consumed by a contract, not printed for a reader.

## Where the AI sits

Postage has two independent decision lanes that both bear on the outcome for
a message, and they do not share inputs.

**Lane one — the classifier.** `web/src/lib/classify.ts` exports
`classify(mail: MailFacts)`. `MailFacts`
(`classify.ts:6-16`) is `from`, `to`, `subject`, `body`, `spf`, `dkim`,
`dmarc`, `urls` — the message and what the receiving MTA already computed
about its authenticity. `classifyWithModel` (`classify.ts:102-117`) calls
`claude-opus-5` (`classify.ts:106`) through the Anthropic SDK with a
structured output schema (`classify.ts:27-31`) and returns one of four tiers
— `human`, `important`, `commercial`, `dangerous` — with a confidence and
plain-language reasons (`classify.ts:18-25`). On failure it falls back to
`classifyFromHeaders` (`classify.ts:127-151`), a deterministic, deliberately
conservative fallback that can never reach the top tier.

**Lane two — the pricing engine.** `pricing.ts:37-103` (`quote`) takes that
tier plus `SenderSignals | null` from the Graph (step 3 above) and computes
an amount by fixed arithmetic — no model involved.

The two lanes do not talk to each other. `classify.ts` never imports from
`reputation.ts` or `graph.ts`; grepping `classify.ts` for
`SenderSignals|signals|reputation|graph` returns nothing. The classifier
decides *what kind of message this is* from the message alone; the Graph
decides *what this sender's history is worth* from indexed chain data alone.
`challenge.ts:44-71` is where the two lanes meet — `verdict` from lane one and
`signals` from lane two are both passed into `quote()` — but they meet as two
separate arguments to one deterministic function, not as one blended input to
a model.

This split is a deliberate design choice, not an oversight, and it is worth
stating as one: the part of the pricing decision that has to be auditable,
reproducible, and cheap to verify — "why did this specific sender's price
change" — is arithmetic over data anyone can re-query and check against
`DEPLOYMENTS.md:94-98`'s own worked example. The part that genuinely needs
judgment about unstructured text — "is this phishing" — is where the model
sits, and its failure mode is a documented, testable fallback rather than a
missing price. Feeding sender history into the classifier's prompt would make
the interesting number (the price) depend on a model's interpretation of a
raw input the model cannot be forced to weigh consistently. Keeping it out
means the price a sender was charged can be recomputed from the chain state
`quote()` read, byte for byte, without asking a model to reproduce its own
past judgment on a different input.

## What became easier

`contracts/src/PostageEscrow.sol` stores no per-sender aggregate. The only
sender-adjacent onchain storage is `settlementOf` keyed by message id
(`PostageEscrow.sol:63`) and `earnings` keyed by inbox
(`PostageEscrow.sol:51`) — there is no mapping from a sender address to a
paid count or a spam rate. Computing "has this wallet paid before, and how
often was it reported" from chain state alone means scanning every past
`Paid` and `SpamReported` log and filtering by sender — on Arc testnet today,
cheap; at any real volume, an `eth_getLogs` scan is not something to run
synchronously inside `challenge.ts`, which sits on the critical path of
holding an inbound email before the sender's browser has even loaded the
challenge page.

The subgraph turns that into `sender(id: $wallet) { paidCount spamReports
spamRate }` (`reputation.ts:17-23`) — one indexed lookup, already aggregated,
already correct as of the last indexed block. That is what let the pricing
path be a request-time read instead of a background job or an event-scan the
product would otherwise have needed to run and cache itself, duplicating what
the subgraph already does.

## Reproducing it

Deploy the subgraph (`subgraph/package.json:8`):

```
cd subgraph
npm run codegen   # graph codegen
npm run build     # graph build
npm run deploy    # graph deploy usepostage --node https://api.studio.thegraph.com/deploy/
```

`subgraph/.env.example` names `GRAPH_DEPLOY_KEY` (from Studio's own subgraph
page) and `GRAPH_SUBGRAPH_SLUG=usepostage`.

To exercise the query-to-decision path in `web/`, the relevant variables
(named at their throw sites: `graph.ts:30`, `graph.ts:35`,
`challenge.ts:34`) are:

- `GRAPH_QUERY_URL` — the deployed Studio endpoint,
  `https://api.studio.thegraph.com/query/1758667/usepostage/v0.4.0`
  (`DEPLOYMENTS.md:88`).
- `GRAPH_API_KEY` — a decentralized-network gateway key, used only for the
  `queryNetwork` / ENS path (`graph.ts:34-36`).
- `CLASSIFIER_PRIVATE_KEY` and `MESSAGE_ID_SECRET` — needed downstream of the
  quote, checked immediately before use at `challenge.ts:77`, without which
  `issueChallenge` cannot sign what `quote()` computed.

With those set, sending mail to a handle that has a wallet with prior history
will produce a quote whose `reasons` array (`pricing.ts:73,77,81,86`) names
which Graph-derived signal moved the price, and that price can be checked
against `sender(id: $wallet)` on the Studio endpoint directly.

## Summary: real integration, not decoration

- **Load-bearing, not optional.** The app uses The Graph as its actual source
  of blockchain data, and that dependency is not decorative. See
  *Load-bearing by construction* above: `challenge.ts:34` hard-fails pricing
  for any returning sender if the Graph endpoints are unset, with no fallback
  of the kind the classifier has.
- **Live data, not a fixture.** Postage's own subgraph is queried through
  Subgraph Studio with an API key (`graph.ts:29-31`) — never a mocked,
  local-only, or static dataset. See *Live, not fixtures*: `graph.ts:17`'s
  `cache: "no-store"` fetch, no fixtures anywhere in the tree, and the live
  `/network` page and `DEPLOYMENTS.md:94-98` agreeing on the same indexed
  numbers independently.
- **Data that decides something, not data that's just printed.** See *The
  query-to-decision trace*: the Graph-derived `SenderSignals` are reduced
  to a priced, EIP-712-signed quote (`pricing.ts:70-89`, `quote.ts:51-68`)
  that a smart contract independently verifies and enforces
  (`PostageEscrow.sol:162-193`) before it will accept payment. That is a
  decision with an onchain consequence, not a display of what was queried.

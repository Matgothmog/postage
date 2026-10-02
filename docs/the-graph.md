# The Graph in Postage

This traces the path from an indexed onchain event to an enforced price, and
states plainly what role an LLM plays alongside it. Every claim below cites a
file and line in this repository so it can be checked directly rather than
taken on faith.

For the endpoints, entities, and example queries an agent would actually run
against this data, see `../SKILL.md`.

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

`crates/server/src/graph.rs` talks to two different things and keeps them
syntactically separate:

- `Graph::query_postage` (`graph.rs:114`) hits the URL in `GRAPH_QUERY_URL` — the
  project's own subgraph above, deployed to **Subgraph Studio** and queried
  directly, with no key in the request. Studio is a hosted query endpoint,
  distinct from the decentralized-network gateway used below for ENS.
- `Graph::query_network` (`graph.rs:127`) hits
  `https://gateway.thegraph.com/api/{GRAPH_API_KEY}/subgraphs/id/{subgraph_id}`
  — the **decentralized-network gateway**, authenticated with an API key, used
  to reach subgraphs Postage does not own. The one call site
  (`crates/server/src/reputation.rs:19`) points it at `ENS_SUBGRAPH`
  (`crates/core/src/reputation.rs:12`), a public ENS subgraph identified by its
  network subgraph ID.

These are not interchangeable and the code does not blur them: first-party
reputation data (payments, spam reports) comes from Studio; third-party
context about a wallet (ENS ownership and age) comes through the gateway. Both
lanes are exercised on every priced message that has a wallet on file
(`crates/server/src/reputation.rs:13-25`, discussed below).

## Live, not fixtures

`Graph::query` (`graph.rs:144`) is a plain HTTP POST issued at request time
with a 10-second timeout (`graph.rs:23`) — no caching layer, no persisted
snapshot. There is no fixture file, mock response, or static JSON blob anywhere
in `subgraph/` or in the server's production code standing in for a query
result; every call to `query_postage` or `query_network` reaches a live
endpoint or returns an error (`graph.rs:144-178`). The stub servers that exist
are test code and are never compiled into the API.

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

`crates/server/src/routes/mail_inbound/challenge.rs:164-167`:

```rust
async fn sender_signals(state: &AppState, wallet: &str) -> Result<SenderSignals, BoxError> {
    require_configured(state.env(), &["GRAPH_QUERY_URL", "GRAPH_API_KEY"])?;
    Ok(gather_signals(state.graph(), wallet).await)
}
```

`require_configured` returns an error before `gather_signals` — and therefore
before any Graph query — runs if either variable is unset. That error is not
handled locally; it propagates out of `issue_challenge`
(`challenge.rs:61-110`) to the route's top-level handler
(`crates/server/src/routes/mail_inbound.rs:138`), which turns it into a fault
response instead of a priced challenge. Remove `GRAPH_QUERY_URL` or
`GRAPH_API_KEY` and pricing for that request does not degrade — it fails.

The precise scope: `sender_signals` is only called when the sender has a
wallet on file (`challenge.rs:116-121`). A first-time sender with no wallet
recorded skips the Graph call and prices with no signals. The hard failure on
missing configuration hits exactly the population the reputation system
exists for — returning senders — which is the case The Graph is supposed to be
load-bearing for in the first place. There is no fallback for a missing
configuration of the kind `classify_from_headers` has for the classifier (see
below). Once both variables are set, a Graph that is unreachable or slow
(past the 10-second timeout) is softened inside `gather_signals`, and the
sender is priced as if the lookup had come back empty.

## The query-to-decision trace

One hop per step, each traceable to a line:

1. **Indexed events.** A `Paid` or `SpamReported` event on `PostageEscrow` is
   picked up by `subgraph/src/escrow.ts:16-56`, which increments
   `Sender.paidCount` / `Sender.spamReports` and recomputes `spamRate`
   (`shared.ts:31-39`).
2. **Query.** `crates/core/src/reputation.rs:15-23` (`POSTAGE_HISTORY`) asks
   Studio for `sender(id: $wallet) { paidCount spamReports spamRate }`,
   fired through `query_postage` (`graph.rs:114`). In parallel,
   `reputation.rs:25-31` (`ENS_OWNED`) asks the gateway for that wallet's ENS
   domains, fired through `query_network` (`graph.rs:127`).
3. **Assembling `SenderSignals`.** `gather_signals`
   (`crates/server/src/reputation.rs:13-25`) runs both queries concurrently
   with `tokio::join!` and keeps each answer or an empty one — either can fail
   independently without blocking the other or failing the request — and
   `signals_from` (`crates/core/src/reputation.rs:63`) reduces the result to the
   five fields declared at `crates/core/src/pricing.rs:12-19`: `paid_count`,
   `spam_reports`, `spam_rate`, `ens_names`, and `oldest_ens_at` (derived from
   the oldest ENS `createdAt`, `reputation.rs:78`).
4. **Turning signals into an amount.** `crates/core/src/pricing.rs:116-149`
   (`apply_signals`) is the arithmetic core:

   ```rust
   if signals.paid_count > 0 && signals.spam_rate > 0.0 {
       *bps += js_round(signals.spam_rate * 4.0 * ONE);
       ...
   }
   if signals.paid_count >= 3 && signals.spam_rate < 0.2 {
       *bps = js_round(*bps * 0.5);
       ...
   }
   if signals.ens_names > 0 {
       *bps = js_round(*bps * 0.7);
       ...
       if age > YEAR_SECONDS { *bps = js_round(*bps * 0.8); ... }
   }
   ```

   A history of spam raises the multiplier; a clean paid history above three
   messages halves it; an owned ENS name discounts further, more so if it is
   over a year old. This is fixed arithmetic on basis points
   (`pricing.rs:4-5`, `ONE = 10_000`) with a hard floor and ceiling
   (`pricing.rs:104`, `bps.clamp(ONE, CEILING)`) — deterministic, not a model
   call. Every step appends a human-readable reason to the quote
   (`pricing.rs:119,126,134,141`), so the price is explainable from its own
   output without re-deriving it.
5. **Signed quote.** `price` (`challenge.rs:114-135`) reads the inbox's floor
   from the chain (never a cached copy, per the comment there) and calls
   `quote(floor, tier, signals, degraded, now)` (`challenge.rs:128`); then
   `sign_quote` (`crates/core/src/quote.rs:210`, called from `challenge.rs:148`)
   signs the messageId, inbox, tier, amount, and expiry as EIP-712 typed data
   (the `Quote` struct at `quote.rs:34-46`) with `CLASSIFIER_PRIVATE_KEY`.
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

**Lane one — the classifier.** `Classifier::classify`
(`crates/server/src/classify.rs:120`) takes a `MailFacts`
(`crates/core/src/classify.rs:15-24`): `from`, `to`, `subject`, `body`, `spf`,
`dkim`, `dmarc`, `urls` — the message and what the receiving MTA already
computed about its authenticity. `classify_with_model` (`classify.rs:142`)
calls `claude-opus-5` (`classify.rs:26`) over HTTP with a structured output
schema whose `tier` is a real enum (`classify.rs:233-250`), a 20-second timeout
per attempt and one retry (`classify.rs:33-35`), and returns one of four tiers
— `human`, `important`, `commercial`, `dangerous` — with a confidence and
plain-language reasons (`ModelVerdict`, `crates/core/src/classify.rs:40-46`). On
failure it falls back to `classify_from_headers`
(`crates/core/src/classify.rs:244`), a deterministic, deliberately conservative
fallback that can never reach the top tier.

**Lane two — the pricing engine.** `quote` (`pricing.rs:62`) takes that tier
plus `Option<&SenderSignals>` from the Graph (step 3 above) and computes an
amount by fixed arithmetic — no model involved.

The two lanes do not talk to each other. Neither `classify.rs` (the one in
`crates/server` or the one in `crates/core`) mentions `SenderSignals`,
`reputation` or `graph`; grepping them for those words returns nothing. The classifier
decides *what kind of message this is* from the message alone; the Graph
decides *what this sender's history is worth* from indexed chain data alone.
`challenge.rs:61-135` is where the two lanes meet — the verdict from lane one and
the signals from lane two are both passed into `quote()` — but they meet as two
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
synchronously inside `challenge.rs`, which sits on the critical path of
holding an inbound email before the sender's browser has even loaded the
challenge page.

The subgraph turns that into `sender(id: $wallet) { paidCount spamReports
spamRate }` (`crates/core/src/reputation.rs:15-23`) — one indexed lookup, already aggregated,
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

To exercise the query-to-decision path in `crates/server`, the relevant
variables (named where they are read: `graph.rs:122`, `graph.rs:136`,
`challenge.rs:165`) are:

- `GRAPH_QUERY_URL` — the deployed Studio endpoint,
  `https://api.studio.thegraph.com/query/1758667/usepostage/v0.4.0`
  (`DEPLOYMENTS.md:88`).
- `GRAPH_API_KEY` — a decentralized-network gateway key, used only for the
  `query_network` / ENS path (`graph.rs:127`).
- `CLASSIFIER_PRIVATE_KEY` and `MESSAGE_ID_SECRET` — needed downstream of the
  quote, checked immediately before use at `challenge.rs:145-147`, without which
  `issue_challenge` cannot sign what `quote()` computed.

With those set, sending mail to a handle that has a wallet with prior history
will produce a quote whose `reasons` array (`pricing.rs:119,126,134,141`) names
which Graph-derived signal moved the price, and that price can be checked
against `sender(id: $wallet)` on the Studio endpoint directly.

## Summary: real integration, not decoration

- **Load-bearing, not optional.** The app uses The Graph as its actual source
  of blockchain data, and that dependency is not decorative. See
  *Load-bearing by construction* above: `challenge.rs:165` hard-fails pricing
  for any returning sender if the Graph endpoints are unset, with no fallback
  of the kind the classifier has.
- **Live data, not a fixture.** Postage's own subgraph is queried through
  Subgraph Studio (`graph.rs:114`) — never a mocked,
  local-only, or static dataset. See *Live, not fixtures*: `graph.rs:144`'s
  request-time POST, no fixtures in any production code path, and the live
  `/network` page and `DEPLOYMENTS.md:94-98` agreeing on the same indexed
  numbers independently.
- **Data that decides something, not data that's just printed.** See *The
  query-to-decision trace*: the Graph-derived `SenderSignals` are reduced
  to a priced, EIP-712-signed quote (`pricing.rs:116-149`, `quote.rs:210`)
  that a smart contract independently verifies and enforces
  (`PostageEscrow.sol:162-193`) before it will accept payment. That is a
  decision with an onchain consequence, not a display of what was queried.

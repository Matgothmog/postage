---
name: postage-graph-signals
description: Query Postage's Graph-indexed onchain data (sender payment history, spam reports, ENS ownership) and turn it into the same price a message would actually be charged. Use this when an agent needs a wallet's reputation inside Postage, the current pricing inputs for an inbox, network-wide totals, or an ENS lookup through the decentralized network gateway.
---

# Postage: Graph-indexed sender signals

Postage is an email-toll service: mail to a `you@usepostage.com` address is held
until the sender pays a small USDC-denominated toll, unless a classifier clears
it free or World ID proves a human. The price a sender pays is not fixed — it
moves on their history, and that history lives entirely in a subgraph indexed
by The Graph, not in any database Postage runs itself.

## Endpoints and credentials

Two separate Graph providers, two separate env vars. Both are read server-side
only (never `NEXT_PUBLIC_*`) via `required()` (`web/src/lib/env.ts:2-6`), which
throws rather than passing `undefined` through.

| Provider | Env var | What it's for | Used by |
|---|---|---|---|
| Postage's own subgraph, on **Subgraph Studio** | `GRAPH_QUERY_URL` | Sender history, inbox pricing state, payments, vault totals — Postage's own data | `queryPostage()` (`web/src/lib/graph.ts:29-31`) |
| Decentralized network **gateway** | `GRAPH_API_KEY` | Third-party subgraphs Postage doesn't own — today, just ENS | `queryNetwork()` (`web/src/lib/graph.ts:34-37`) |

`GRAPH_QUERY_URL` is queried directly with no key in the request — Studio query
URLs are public reads. `GRAPH_API_KEY` is never sent to Studio; it's only
spent on gateway calls, embedded in the URL path
(`https://gateway.thegraph.com/api/$GRAPH_API_KEY/subgraphs/id/$subgraphId`).
Deployed Studio endpoint (public, not a secret):
`https://api.studio.thegraph.com/query/1758667/usepostage/v0.4.0`
(`DEPLOYMENTS.md:88`). ENS subgraph ID on the network:
`5XqPmWe6gjyrJtFn9cLy237i4cWw2j9HcUJEXsP5qGtH` (`web/src/lib/graph.ts:5`).

Set both in `.env`/`.env.local` from your own Studio project and gateway API
key — never hardcode a key, never commit one.

## Entity model (`subgraph/schema.graphql`)

Seven entities across four contracts on **Arc testnet** (chain id `5042002`):

- `Sender` — one row per wallet that ever paid: `paidCount`, `totalPaid`,
  `spamReports`, `spamRate` (= `spamReports / paidCount`), `firstSeenAt`,
  `humanUntil`. The reputation record pricing reads back.
- `Inbox` — one row per `you@usepostage.com` address: `floorPrice`,
  `receivedCount`, `earned`, `claimed`.
- `Payment` — one row per settled message: `tier` (`Human | Important |
  Commercial | Dangerous`), `amount` (inbox's net share), `toVault`, `paidAt`,
  `reportedAsSpam`.
- `Enclave` / `ExpectedMeasurement` — signer attestation for the pricing
  enclave; not sender-facing.
- `HumanAttestation` — a World ID result recorded on Arc.
- `Vault` — one singleton row, protocol-wide totals: `totalFunded`,
  `toTreasury`, `toSponsorship`, `refilledToRelayer`, `withdrawnByTreasury`,
  `fundingEvents`.

## Example queries (all run live against the deployed endpoints, 2026-09-12)

**1. Sender history / reputation signals** — the exact query `reputation.ts`
sends (`web/src/lib/reputation.ts:16-24`):

```graphql
query SenderHistory($wallet: ID!) {
  sender(id: $wallet) { paidCount spamReports spamRate }
}
```
Verified live, e.g. `{"wallet":"0x01d01288a964c0d6efbdcaf4bb8d6e528dc0bbf3"}` →
`{"sender":{"paidCount":6,"spamReports":0,"spamRate":"0"}}`. Lowercase the
wallet first — entity IDs are stored lowercase (`reputation.ts:49`).

**2. Current pricing inputs for an inbox** — what an inbox charges before
reputation moves it:

```graphql
query { inboxes(first: 5) { id floorPrice receivedCount earned claimed } }
```
Verified live, returns real `floorPrice`/`earned` figures for deployed
inboxes.

**3. Recent network activity**:

```graphql
query {
  payments(first: 5, orderBy: paidAt, orderDirection: desc) {
    id tier amount toVault paidAt reportedAsSpam
    sender { id }
    inbox { id }
  }
}
```
Verified live against real settled payments.

**4. Network totals** (singleton `Vault`):

```graphql
query { vaults(first: 1) { id totalFunded toTreasury toSponsorship refilledToRelayer withdrawnByTreasury fundingEvents } }
```
Verified live: `totalFunded` and `fundingEvents` both nonzero. See Pitfalls —
don't query `vault(id: "vault")` directly.

**5. ENS ownership, through the decentralized network gateway** — the exact
query `reputation.ts` sends (`web/src/lib/reputation.ts:26-32`), against
`ENS_SUBGRAPH`:

```graphql
query NamesOwned($wallet: String!) {
  domains(first: 5, where: { owner: $wallet }) { createdAt }
}
```
Verified live via the gateway with a well-known ENS-holding address, returned
five `createdAt` timestamps. This is real mainnet ENS data — unlike Postage's
own subgraph, it has nothing to do with Arc testnet.

## How signals become a price

`gatherSignals(wallet)` (`web/src/lib/reputation.ts:48-67`) runs query 1 and
query 5 concurrently via `Promise.allSettled`, so a failure in either softens
the price rather than blocking the message, and returns a `SenderSignals`:
`paidCount`, `spamReports`, `spamRate`, `ensNames`, `oldestEnsAt`.

`quote()` (`web/src/lib/pricing.ts:37-103`) turns that into a price, starting
from a per-tier base (`TIER_BPS`, `pricing.ts:19-31`) and adjusting in order:

- spam history raises it (`pricing.ts:71-74`: `spamRate * 4×` added, only if
  `paidCount > 0`)
- a clean paid history lowers it (`pricing.ts:75-78`: 3+ payments and
  `spamRate < 0.2` halves it)
- holding an ENS name lowers it further (`pricing.ts:79-82`: ×0.7), and an
  ENS name registered over a year ago lowers it again (`pricing.ts:83-87`:
  ×0.8)

The result is clamped to `[1×, 10×]` of the inbox's floor
(`pricing.ts:91-94`) and returned with a human-readable `reasons[]` trail
naming exactly which signal moved it. This is arithmetic over Graph data, not
a model's judgment — see `docs/the-graph.md` for why that split is
deliberate.

## Live vs. testnet-only

Postage's own subgraph indexes real transactions, but on **Arc testnet** —
every `Sender`/`Payment`/`Vault` row above is genuinely live-indexed, not a
fixture, but it isn't mainnet money. The ENS subgraph reached through the
gateway is real mainnet data with no testnet relationship to Postage at all.

## Pitfalls

- **`vault(id: "vault")` fails** with `Store error: Odd number of digits`.
  `Vault.id` is `Bytes`, and the singleton's ID is the ASCII bytes of
  `"vault"`, not the literal string — the store expects a hex string. Query
  the plural `vaults(first: 1)` instead (shown above), or pass the hex form
  `0x7661756c74` if you need the singular field.
- **No caching layer.** `query()` (`web/src/lib/graph.ts:12-26`) fetches with
  `cache: "no-store"` every call — nothing memoizes repeated lookups of the
  same wallet, so an agent polling this in a loop should throttle itself.
- **The subgraph lives on Subgraph Studio, not the decentralized network** —
  `GRAPH_QUERY_URL` is a direct Studio query URL with no API key attached.
  Only the gateway calls (ENS, or any other public subgraph) spend
  `GRAPH_API_KEY`; that key does nothing for Postage's own data.
- **Both env vars are required together, even for gateway-only reads** — the
  pricing path checks `GRAPH_QUERY_URL` and `GRAPH_API_KEY` are both set
  before running either query (`web/src/app/api/mail/inbound/challenge.ts:33-36`)
  and throws before querying anything if either is missing.
- **Rate limits apply** on both the Studio endpoint and the gateway key —
  this is a free-tier setup, not built for high query volume.

# Postage

A filter in front of the inbox you already have, and a price on the mail that
wastes your time.

Give out `you@usepostage.com`. Mail sent there is read, judged, and forwarded to
the address you actually use. You do not change email provider, and you do not
learn a new inbox.

**Live at [postage-seven.vercel.app](https://postage-seven.vercel.app).**
[`/network`](https://postage-seven.vercel.app/network) shows live,
subgraph-indexed data — 1 settled payment, 9 verified people, as of this
writing — and is the fastest way to see the system has actually run.

## The idea

Spam is cheap to send and expensive to receive. Every filter ever built has tried
to fix that by guessing better. Postage does something else: it makes the sender
carry the cost. Marketing that wants your attention pays for it, and the money is
yours.

Everything below exists to make a one-cent charge on a stranger's email actually
work.

## What happens to a message

Every stranger is held. The classifier does not decide whether to hold you — it
decides **who pays to get through**.

| What it is | What happens |
| --- | --- |
| Something you are waiting for — a login code, a receipt, a delivery update | Delivered at once, free. Never held. |
| Written by a person | Held. Say a person wrote it and it clears, for nothing. |
| Ordinary automated mail — newsletters, marketing | Held. Nobody proved a person is behind it, so it pays. |
| Trying to deceive you | Never delivered. Being a person does not clear it, and paying is a penalty. |

A held sender gets a **reply to the message they just sent**, threaded to it,
asking one question: did a person write this, or a machine? Two links, one
answer. Nothing asks them to write the message again, because it is still here.

Say a person wrote it and the sender is routed into a World ID Selfie Check.
Passing it binds a nullifier to their wallet in `HumanRegistry` onchain, so the
same credential cannot clear a second identity — proven end to end by a
real-phone Selfie Check on 2026-09-09, attested
[onchain](https://testnet.arcscan.app/tx/0x48c7b5cd756cdd017d1aa0dc83e4bcdee1ee86c7ec0a8ea47eda27fff34537ee).
That check only runs in live identity mode; the default is mock, where the
claim clears on a sender-keyed stand-in and nothing is actually verified. **No
wallet, no account, nothing to sign up for** — the free lane should not charge
a toll in setup. A machine pays instead, and only then is there anything to
create an account for, because only then is there money to move.

## What arrives is what was sent

A released message is the bytes that arrived. The sender's own DKIM signature
still covers it, their address is still in `From:`, and nothing of ours has been
added — no footer, no subject tag, no rewritten links.

That is harder than it sounds, and it is the constraint that shaped the
architecture. See [ARCHITECTURE.md](ARCHITECTURE.md#carrying-a-release).

## A pass runs out

Proving personhood opens a fifteen minute window; paying buys one delivery.
Writing again tomorrow means answering again. World ID is a check that someone
was there a moment ago, not a badge an address keeps.

The hold outlives the pass on purpose. A pass measures how recently somebody
proved they were there. A hold measures how long a person takes to read their
mail, so it lasts a day.

## Why it needs each piece

**Arc** is where the money is. USDC is its native gas token, so a one-cent price
and the fraction of a cent of gas that moves it are quoted in the same unit. On a
chain with a volatile gas token, a one-cent price is not a coherent idea.

**World ID** is the free lane, meant to ask nothing of anybody's wallet. The
attestation goes onchain against an address derived from the nullifier — an
identity nobody holds a key to — and a real nullifier is what would stop one
person minting themselves unlimited free senders. World App's Selfie Check
supplies that proof through IDKit; the server verifies it against World's
Developer Portal, and only a verified proof puts the attestation onchain. The
flow is proven against World's Sandbox App on a sandbox-configured preview
deploy — the production deploy is built for production World, where Selfie
Check is not offered.

**The Graph** decides what a sender pays. Every payment, every verdict, and every
time a recipient contradicted the classifier is indexed, and that history prices
the next message.

**Privy** gives a wallet to people who do not have one, and its identity token is
what makes signing up a single click.

**Mailgun** carries a released message without touching it, which is the one
thing Cloudflare's own sending cannot do.

## The price cannot be made up

A price is only valid if it carries a signature from a key listed in
`EnclaveRegistry`, and the escrow rejects anything else. The classifier signs
what it decided; the chain refuses to charge you on anyone else's say-so.

Today that key belongs to an ordinary server process, and the registered
measurement says exactly that:
`keccak256("stage1-plain-classifier-not-attested")`. The next step is a Nitro
enclave, where the measurement becomes a hash of the running image anyone can
recompute from source. The interface does not change, only who may sign.

## Signing up

Sign in, pick a handle, click the link Cloudflare emails you. That is all of it.

Signing in already proves you can read the address the handle points at, so
nothing asks you to prove it twice. An inbox charges one cent before its owner
has picked a price, so the first message is charged for correctly without a
transaction, a balance, or a decision.

## This is a proof of concept

It runs, it charges real testnet USDC, and the parts do what this file says they
do. It is not a service to point your real mail at yet.

**A held message is kept, and that is a real cost.** For a sender to answer one
question and have their mail arrive without writing it twice, Postage has to
still have it. So it is stored for at most a day, in the worker that received it
and nowhere else, and deleted the moment it is released or its deadline passes.

**Not stored is not the same as not seen.** Cloudflare receives the message, the
worker parses it, the gateway is handed the parsed fields, and the classifier
reads them. That is not a gap in the implementation, it is what SMTP is — mail
arrives in plaintext, so whatever terminates the connection holds it. Closing
that properly means running the MTA itself inside an enclave. See
[what privacy would take](ARCHITECTURE.md#what-privacy-would-actually-take).

## Layout

    contracts/   escrow, enclave registry, identity registry, vault
    shared/      the verdict shape worker and web agree on, nothing else
    subgraph/    indexes all four on Arc
    web/         signup, dashboard, challenge page, and the API behind them
    worker/      the Cloudflare mail worker

Four independent trees. `web/`, `worker/`, and `subgraph/` each have their own
`package.json`; `contracts/` is Foundry, keyed off `foundry.toml`. There is no
root workspace and no root `package.json` — running `npm install` at the repo
root has nothing to install against.

[ARCHITECTURE.md](ARCHITECTURE.md) explains how the parts fit and why each is
there. Addresses and endpoints are in [DEPLOYMENTS.md](DEPLOYMENTS.md).

## Run it

Or skip all of this and use the live demo linked at the top. Otherwise, each
tree below installs, tests, and builds on its own.

### `web/`

```bash
cd web
npm install
cp .env.local.example .env.local
npm run dev         # next dev -p 3210 → http://localhost:3210
```

```bash
npm test            # node --test, src/**/*.test.ts
npm run lint         # eslint
npm run typecheck    # tsc --noEmit
npm run build        # next build
```

`.env.local` has about two dozen variables. Grouped by what they actually gate
(source: the comments in `web/.env.local.example` itself):

- **Ship with a working local default already in the example file**, nothing
  to fill in: `APP_URL` (`http://localhost:3210`), `DATABASE_URL`
  (`file:.data/postage.db`, local SQLite), `MAIL_FROM`.
- **Only matter in live identity mode.** `IDENTITY_MODE` unset/blank defaults
  to `mock`, which the file calls "safe for local development." Live mode
  additionally needs `NEXT_PUBLIC_WORLD_APP_ID`,
  `NEXT_PUBLIC_WORLD_ENVIRONMENT`, `WORLD_RP_ID`, `WORLD_RP_SIGNING_KEY`,
  `WORLD_ACTION`.
- **Gate one integration each, blank otherwise:** `NEXT_PUBLIC_PRIVY_APP_ID`
  (Privy wallet/signup), `ATTESTER_PRIVATE_KEY` (onchain attestations to
  `HumanRegistry`), `MAIL_WEBHOOK_SECRET` / `MAIL_WORKER_URL` (talking to the
  deployed mail worker), `MESSAGE_ID_SECRET` (keys the onchain message id),
  `GRAPH_QUERY_URL` / `GRAPH_API_KEY` (subgraph-priced holds),
  `RELAYER_PRIVATE_KEY` / `CLASSIFIER_PRIVATE_KEY` / `ANTHROPIC_API_KEY`
  (classifier signing and AI classification), `CLOUDFLARE_ACCOUNT_ID` /
  `CLOUDFLARE_API_TOKEN` (registering forwarding addresses), `RESEND_API_KEY`
  (verification and release email). `ARC_RPC_URL` is optional even for
  onchain features — unset, viem falls back to Arc's public RPC.
- Whether `npm run dev` tolerates every gated variable staying blank, or
  throws on one of them, was not exercised live for this README — the
  grouping above is drawn from the file's own comments, not a run of the
  server.

### `worker/`

```bash
cd worker
npm install
npm test             # node --test, src/**/*.test.ts — 51 tests
npm run typecheck     # tsc --noEmit
```

**`npm run typecheck` currently fails on a fresh checkout**, with `sh: 1:
tsc: not found` — `typescript` is a declared devDependency, resolved in the
lockfile, but not actually present under `worker/node_modules` as checked
out. Not fixed here; running the command above will hit it.

There is no lint script and no local build or dev script in
`worker/package.json`. Runtime config (Mailgun, the Cloudflare KV namespace
holding messages) lives in `worker/wrangler.toml`; secrets are set with
`wrangler secret put POSTAGE_API_URL` / `POSTAGE_SECRET` / `MAILGUN_API_KEY`
(`worker/wrangler.toml`, bottom). `npm run deploy` runs `wrangler deploy` and
needs Cloudflare credentials this repo does not ship.

### `subgraph/`

```bash
cd subgraph
npm install
npm run codegen      # graph codegen
npm run build        # graph build
```

Both run fully locally against `subgraph/schema.graphql` and
`subgraph/subgraph.yaml`. There is no test, lint, or dev script. `npm run
deploy` runs `graph deploy usepostage --node
https://api.studio.thegraph.com/deploy/` and needs a Studio deploy key
(`subgraph/.env.example`: `GRAPH_DEPLOY_KEY`) this repo does not ship.

### `contracts/`

`contracts/lib/` (forge-std, OpenZeppelin) is gitignored and not vendored in
a fresh checkout. `contracts/foundry.toml`'s remappings expect `lib/forge-std`
and `lib/openzeppelin-contracts` on disk — fetch them before building. Their
exact source repos aren't pinned anywhere in this checkout (no `.gitmodules`,
no lockfile), but these two paths are the standard Foundry vendoring of
`foundry-rs/forge-std` and `OpenZeppelin/openzeppelin-contracts`:

```bash
cd contracts
forge install foundry-rs/forge-std --no-git
forge install OpenZeppelin/openzeppelin-contracts --no-git
forge build
forge test            # 68 tests, fully offline
```

Use `--no-git` — plain `forge install` stages `.gitmodules` and gitlinks over
directories that are meant to stay plain vendored and gitignored here.

Foundry has no separate typecheck step; `forge fmt` (`contracts/foundry.toml`
`[fmt]`, line length 100) is the closest thing to lint. Deploy config — RPC,
deployer key, live contract addresses to reuse — is in
`contracts/.env.example`, not needed for `forge build` or `forge test`.

## What you can run without credentials

No World ID sandbox build, no Mailgun key, no Graph API key, no Privy app —
here is what still works:

- **All three test suites, unconditionally.** `web` (`cd web && npm test`,
  492 tests), `worker` (`cd worker && npm test`, 51 tests), and `contracts`
  (`cd contracts && forge test`, 68 tests, once `lib/` is fetched — see
  "Run it"). None of the three reach out to a live service.
- **`subgraph/`** — `codegen` and `build` compile the mappings locally; only
  `deploy` needs Studio auth.
- **`web/`** — the dev server starts in mock identity mode (`IDENTITY_MODE`
  unset) against a local SQLite file; neither needs a key. What will not work
  without the matching credential: wallet signup, live World ID verification,
  subgraph-priced holds, onchain writes, talking to a deployed mail worker,
  outbound email — see the breakdown under "Run it" → `web/`.
- **`worker/`** — the test suite runs standalone; real inbound mail and
  `wrangler deploy` need Cloudflare and Mailgun credentials this repo does
  not ship.

## Submission documents

- [SUBMISSION.md](SUBMISSION.md) — the ETHOnline submission: prize tracks
  entered, AI-tool disclosure.
- [docs/the-graph.md](docs/the-graph.md) — how The Graph drives pricing.
- [docs/world-feedback.md](docs/world-feedback.md) — feedback on integrating
  World ID Selfie Check.
- [docs/privy-notes.md](docs/privy-notes.md) — how Privy carries the payment
  UX.
- [docs/demo-script.md](docs/demo-script.md) — the demo video script.
- [ARCHITECTURE.md](ARCHITECTURE.md) — how the parts fit together and why
  each is there.
- [DEPLOYMENTS.md](DEPLOYMENTS.md) — contract addresses and endpoints.

## Licence

MIT. See [LICENSE](LICENSE).

Built for ETHOnline 2026. All work began after the event's start date: first
commit 2026-09-05, no pre-existing code.

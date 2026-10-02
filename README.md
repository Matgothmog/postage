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
That deployment now runs the Rust build in this repository, deployed on
2026-10-02 as a prebuilt preview deployment aliased to postage-seven.vercel.app
(live identity mode, World sandbox). The ETHOnline submission was the
TypeScript build, and the end-to-end proofs cited below were made on it.

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

Say a person wrote it and the sender is routed into a World ID Selfie Check —
live identity mode, which is the default. Passing it binds a nullifier to
their wallet in `HumanRegistry` onchain, so the same credential cannot clear a
second identity — proven end to end by a real-phone Selfie Check on
2026-09-09, attested
[onchain](https://testnet.arcscan.app/tx/0x48c7b5cd756cdd017d1aa0dc83e4bcdee1ee86c7ec0a8ea47eda27fff34537ee).
Set `IDENTITY_MODE=mock` and the claim clears on a sender-keyed stand-in
instead, with nothing actually verified — the escape hatch for running this
without World credentials, not the default. **No wallet, no account, nothing
to sign up for** — the free lane should not charge a toll in setup. A machine
pays instead, and only then is there anything to create an account for,
because only then is there money to move.

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
deploy — the TypeScript production deploy was built for production World, where
Selfie Check is not offered. The TypeScript production site submitted to
ETHOnline also set `IDENTITY_MODE=mock` explicitly, so it cleared every claim on
the sender-keyed stand-in regardless of that env var's own default, which is
live. The Rust build now serving postage-seven.vercel.app runs in live mode
against World's sandbox. The Rust API refuses mock in a Vercel production
environment unless `POSTAGE_ALLOW_MOCK_IN_PRODUCTION=1` is set as well: the
stand-in lets anyone through the free lane, and the relayer pays for each
attestation. A Rust production deployment therefore needs either
`POSTAGE_ALLOW_MOCK_IN_PRODUCTION=1` or live mode.

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
do. It is not a service to point your real mail at yet. Built during ETHOnline
2026 — first commit 2026-09-05, no code carried in from before the event —
which is a statement about how young this is, not a claim about what it is.

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

    contracts/           escrow, enclave registry, identity registry, vault (Solidity)
    subgraph/            indexes all four on Arc (AssemblyScript)
    crates/shared/       wire types the API and the mail worker agree on
    crates/core/         pricing, quote signing, sender authentication, the
                         classifier's header fallback: pure logic, native or wasm
    crates/server/       the API (Axum, libSQL, alloy)
    crates/web/          the browser app (Leptos, compiled to WebAssembly)
    crates/web/js/       a small vendor bridge for Privy and World IDKit
    crates/mail-worker/  the Cloudflare email worker (workers-rs)
    api/index.rs         the Vercel function that serves the API
    fixtures/golden/     frozen test vectors from the original TypeScript
    e2e/                 a local end-to-end run
    scripts/             the web build and a check of vercel.json

The Rust crates are one Cargo workspace rooted at the top-level `Cargo.toml`,
pinned to Rust 1.95.0 with the `wasm32-unknown-unknown` target
(`rust-toolchain.toml`). `contracts/` is Foundry, keyed off `foundry.toml`;
`subgraph/` has its own `package.json`. There is no root `package.json`: the
only npm package besides the subgraph is `crates/web/js`, which bundles the
two SDKs that have no Rust version. No TypeScript application code remains.

[ARCHITECTURE.md](ARCHITECTURE.md) explains how the parts fit and why each is
there. Addresses and endpoints are in [DEPLOYMENTS.md](DEPLOYMENTS.md).

## Run it

Or skip all of this and use the live demo linked at the top. Otherwise, each
part below installs, tests, and builds on its own.

### The Rust workspace

You need the pinned toolchain (`rustup` reads `rust-toolchain.toml`) and
[`cargo-nextest`](https://nexte.st).

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
```

Nothing in the test suite reaches a live service: the database tests use local
libSQL files, and everything outside the process is a loopback stub.

The browser app's component tests run in a real browser. They need headless
Firefox, `geckodriver` and `wasm-bindgen-test-runner` on the path:

```bash
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
  cargo test -p postage-web --target wasm32-unknown-unknown
```

The browser app is built into `crates/web/dist` by `sh scripts/build-web.sh`,
which installs the bridge's npm dependencies (`npm ci` in `crates/web/js`) and
runs `trunk build --release`. It needs `cargo`, `trunk` and `npm` on the path,
and the public ids are read when it runs and compiled into the WebAssembly:

```bash
NEXT_PUBLIC_PRIVY_APP_ID=<privy app id> \
NEXT_PUBLIC_WORLD_APP_ID=<world app id> \
sh scripts/build-web.sh
```

`NEXT_PUBLIC_WORLD_ENVIRONMENT` is optional. Unset, it is `production`; set it
to `sandbox` to target a Sandbox World App build (`staging` is also accepted);
any other value fails the build, and so does anything but `production` in a
Vercel production build.
In a debug build, `trunk serve` included, the app renders only a configuration
notice without `NEXT_PUBLIC_PRIVY_APP_ID`; a release build refuses to compile
without it. `trunk serve` in `crates/web` serves the app on port 3210, but
without an API behind it.

The repository has no standalone API dev server. To see the whole app running
locally with no credentials, use the end-to-end harness, which serves the real
router and the built bundle over a local libSQL file with every outside service
stubbed:

```bash
node e2e/run.mjs        # needs Node 23+, trunk, geckodriver and Firefox
```

See [e2e/README.md](e2e/README.md) for what it covers.

### The API's configuration

The API reads its settings from the process environment, set per project in
Vercel. Grouped by what each gates:

- **Have a working default.** `DATABASE_URL` defaults to
  `file:.data/postage.db`, a local SQLite file; use a `libsql://…` URL with
  `DATABASE_AUTH_TOKEN` for Turso.
- **Only matter in live identity mode, which is the default.** `IDENTITY_MODE`
  unset, blank or `live` means live. Live mode needs `WORLD_RP_ID`,
  `WORLD_RP_SIGNING_KEY` and `WORLD_ACTION`, and `/api/world/context` and
  `/api/world/verify` refuse without them; the web build needs
  `NEXT_PUBLIC_WORLD_APP_ID` as well. Set
  `IDENTITY_MODE=mock` to skip all of it and clear the check on a sender-keyed
  stand-in, which the API refuses when `VERCEL_ENV` is `production` unless
  `POSTAGE_ALLOW_MOCK_IN_PRODUCTION=1` is also set. Any other value is a
  configuration error.
- **Gate one integration each.** `NEXT_PUBLIC_PRIVY_APP_ID` (sign-in; also read
  by the server to check identity tokens), `ATTESTER_PRIVATE_KEY` (onchain
  attestations to `HumanRegistry`), `MAIL_WEBHOOK_SECRET` / `MAIL_WORKER_URL`
  (talking to the deployed mail worker), `MESSAGE_ID_SECRET` (keys the onchain
  message id), `GRAPH_QUERY_URL` / `GRAPH_API_KEY` (subgraph-priced holds),
  `RELAYER_PRIVATE_KEY` / `CLASSIFIER_PRIVATE_KEY` / `ANTHROPIC_API_KEY`
  (relaying, quote signing and classification), `CLOUDFLARE_ACCOUNT_ID` /
  `CLOUDFLARE_API_TOKEN` (registering forwarding addresses), `RESEND_API_KEY` /
  `MAIL_FROM` (verification and release email), `APP_URL` (the origin that
  challenge links point at). `ARC_RPC_URL` is optional even for onchain
  features: unset, the API uses Arc's public RPC.

A missing setting does not take the rest of the API down. Most fail only the
route that needs them, with a 500 whose server log line names the variable. A
few degrade instead: without `ANTHROPIC_API_KEY` mail is classified on its
headers alone, without `NEXT_PUBLIC_PRIVY_APP_ID` the server accepts no
identity token, and `/api/network` answers 502 naming `GRAPH_QUERY_URL`.

### The mail worker

Needs `worker-build` (`cargo install worker-build`) and, to deploy, `wrangler`.

```bash
cd crates/mail-worker
worker-build --release        # builds build/worker/shim.mjs
```

The worker's tests run with the rest of the workspace. Runtime config (Mailgun,
the Cloudflare KV namespace holding messages) is in
`crates/mail-worker/wrangler.toml`; secrets are set with
`wrangler secret put POSTAGE_API_URL` / `POSTAGE_SECRET` / `MAILGUN_API_KEY`.
`npx wrangler deploy` from that directory publishes it as `postage-mail` (it
runs `worker-build --release` first) and needs Cloudflare credentials this repo
does not ship.

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

## Deploying the Rust stack

The browser app and the API deploy together to the existing `postage` Vercel
project, which serves postage-seven.vercel.app; link this checkout to it with
`vercel link` before anything else. The project's ignored build step is
`exit 0`, so pushes to Git build nothing: every deploy is built locally and
uploaded with `--prebuilt`.

`vercel.json` serves `crates/web/dist` as static files, routes `/api/*` to the
single Rust function `api/index.rs`, falls back to `index.html` for every other
path so the client router can answer it, allows the function up to 120
seconds, and sets the security headers, including the Content Security Policy.
`node scripts/check-vercel-routing.mjs` checks those rules without contacting
Vercel.

Build on a machine with the Rust toolchain and a C compiler (libSQL compiles
SQLite from C) and upload the output, rather than relying on Vercel's build
image to compile it. Set the project's environment variables, including the
three `NEXT_PUBLIC_*` ones the web build reads. `vercel pull` writes
`[SENSITIVE]` in place of every sensitive variable, and those pulled values
override the shell, so the `NEXT_PUBLIC_*` values the build bakes in have to
come from a local env file. Then deploy a preview:

```bash
vercel pull --environment=preview
vercel build
vercel deploy --prebuilt
```

Production is a preview deployment like that one, aliased to the domain once
the checks below pass:

```bash
vercel alias set <deployment-url> postage-seven.vercel.app
```

The mail worker deploys separately with `npx wrangler deploy` in
`crates/mail-worker` (see above). It is the `postage-mail` worker, deployed in
place of the TypeScript build, and keeps the KV namespace and binding names that
build used, so held mail survived the switch.
`npx wrangler rollback 73edd33b-700a-4cd8-80a5-c32c8126e49a` returns to the last
TypeScript version.

Before pointing real traffic at a new deployment:

1. Done (2026-10-02): real mail through the deployed `postage-mail` worker
   showed Cloudflare stamping its own `Authentication-Results`, with the
   authserv-id `mx.cloudflare.net`, topmost, above any the sender wrote. The
   worker trusts only headers with that id and reads the topmost; if
   Cloudflare ever stamped only the ARC header, a sender's own
   `Authentication-Results` carrying that id would be read first.
2. On the first preview deployment, check that `GET /api/health` and a real API
   route both reach the Axum router with the path they were sent.
3. Sign in with Privy, pay a quote, and run a Selfie Check with real ids,
   watching the browser console for Content Security Policy refusals.
4. Run a smoke test against a disposable Turso database before the production
   one.
5. Decide the production identity mode. With `IDENTITY_MODE=mock` and no
   `POSTAGE_ALLOW_MOCK_IN_PRODUCTION=1`, a Vercel production deployment answers
   `POST /api/world/verify` and the view of an open challenge
   (`GET /api/challenge/{token}`) with a 500, so no held sender can load their
   challenge page at all.

## What you can run without credentials

No World ID sandbox build, no Mailgun key, no Graph API key, no Privy app —
here is what still works:

- **The Rust test suites, unconditionally.** `cargo nextest run --workspace`
  (1,362 tests across the five crates) and the browser component tests
  (124 tests), plus `contracts` (`cd contracts && forge test`, 68 tests,
  once `lib/` is fetched — see "Run it"). None of them reach out to a live
  service.
- **The whole app, locally.** `node e2e/run.mjs` drives the real browser bundle
  against the real router with every outside service stubbed.
- **`subgraph/`** — `codegen` and `build` compile the mappings locally; only
  `deploy` needs Studio auth.
- **A release build of the web app** needs only placeholder ids to compile. It
  will not sign anyone in: wallet signup needs a real Privy app, and the
  challenge page needs `IDENTITY_MODE=mock` or World credentials. What still
  will not work without its own credential: subgraph-priced holds, onchain
  writes, talking to a deployed mail worker, outbound email — see "The API's
  configuration".
- **The mail worker** — its tests run with the workspace; real inbound mail and
  `wrangler deploy` need Cloudflare and Mailgun credentials this repo does not
  ship.

## Changes from the TypeScript version

The original app was TypeScript on Next.js, with a TypeScript Cloudflare worker.
It was ported to Rust and WebAssembly with the same routes, screens and
contracts. Where the port fixed a defect or tightened a rule, the behaviour
changed:

- **Sender authentication.** A sender counts as authenticated when SPF and DKIM
  both pass, or when DMARC passes and the `From:` domain equals the envelope
  domain. Some mailing-list and forwarded mail that used to pass is now
  challenged. The worker reads results only from a header whose authserv-id is
  `mx.cloudflare.net` (the topmost `Authentication-Results`, or failing that an
  `ARC-Authentication-Results` with `i=1`) and sends the `From:` address to the
  gateway as `header_from`. That is safe only if Cloudflare stamps its own
  `Authentication-Results` above any the sender wrote, which real mail
  confirmed on 2026-10-02 (check 1 under "Deploying the Rust stack").
- **Pasted messages** from a sender who could not be verified go out without a
  `Reply-To` and with a footer saying the sender address could not be verified.
- **Payments.** A payment on an expired pass that still has uses left adds a
  use, and concurrent first payments from one sender are all counted.
- **Classifier.** Each attempt times out after 20 seconds, with one retry, and
  the tier is constrained to the four allowed values in the request.
- **Mock identity mode** is refused in a Vercel production environment unless
  `POSTAGE_ALLOW_MOCK_IN_PRODUCTION=1` is set.
- **Challenge page.** A World ID poll that fails unexpectedly reads
  "Verification failed. Try again, or pay instead." The server enforces 200 and
  20,000 characters for subject and body, as before.
- **Sign-out and races.** A second wallet on the same browser no longer sees the
  previous wallet's inbox, concurrent claims on one handle resolve to a single
  owner, and a slow upstream (the subgraph, Cloudflare, Resend, World) times out
  instead of hanging the request.

## Documentation

Everything above is the pitch. For the parts that need more depth:

- [ARCHITECTURE.md](ARCHITECTURE.md) — how the four tiers, the
  parts of the system, and the trust boundaries fit together, including what is
  deliberately not built yet. For anyone changing the system, or checking
  what's real versus aspirational.
- [DEPLOYMENTS.md](DEPLOYMENTS.md) — contract addresses, deploy blocks, and
  explorer links on Arc Testnet. For anyone verifying a transaction or
  pointing a client at a specific address.
- [docs/the-graph.md](docs/the-graph.md) — how an indexed onchain event
  becomes the price a sender pays, cited against the subgraph mappings line
  by line. For anyone auditing the pricing logic or extending the subgraph.
- [SKILL.md](SKILL.md) — agent-executable queries against the Graph and how signals
  map to prices. For an AI agent or tool automating decisions with Postage's subgraph data.
- [docs/privy-notes.md](docs/privy-notes.md) — why Privy is what lets a
  stranger clear a hold without ever installing a wallet. For anyone
  evaluating the signup and identity UX.
- [docs/world-feedback.md](docs/world-feedback.md) — friction found
  integrating World ID Selfie Check, written for World's own team. For anyone
  building against World ID, or World engineers themselves.

`docs/demo-script.md` is the shot list for the submission demo video —
stagecraft for a camera, not documentation about the system — and by its own
recommendation isn't linked from here.

[SUBMISSION.md](SUBMISSION.md) is the ETHOnline 2026 submission as filed:
prize-track writeups and an AI-tool disclosure. It is kept as an archival
record of what was submitted and is not maintained as product documentation
— read the sections above for how Postage actually works today.

## Licence

MIT. See [LICENSE](LICENSE).

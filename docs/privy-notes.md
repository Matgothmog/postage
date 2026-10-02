# Privy: how it improves the user experience

Every path through Postage that ends with someone holding a wallet — the
inbox owner claiming an address, and a challenged stranger who pays instead
of proving personhood — depends on Privy removing wallet setup from the
critical path. The free lane is the exception: proving personhood through
World ID Selfie Check needs no wallet and never touches Privy at all
(`crates/web/src/challenge_actions.rs:366-372`, `README.md:53-54`). The
material for the paths that do depend on it exists scattered across the
codebase and in `ARCHITECTURE.md:273-301`; this document is the one place it
is stated as an argument.

## The user who matters

Postage has two kinds of user. The inbox owner is crypto-native by
construction — they claimed a wallet-backed address on purpose. The user this
document is about is the other one: a stranger who wrote a normal email, got
held, and received a reply containing an unlock link (`ARCHITECTURE.md:275-276`,
"The person clicking an unlock link is a stranger with no wallet and no reason
to install one"). They did not choose to interact with a blockchain. To get
their message through by paying roughly one cent, they now have to make an
on-chain payment.

That is the hard case, because every part of the standard crypto onboarding
path is disproportionate to a one-cent transaction: installing a browser
extension, funding a wallet, holding a gas token, backing up a seed phrase.
Anyone who bounces off that flow is a real email that did not get delivered.
The UX question this document answers is what Privy removes from that path.

## What Privy does here, concretely

The product depends on `@privy-io/react-auth` 3.40.0 (pinned at
`crates/web/js/package.json:11`). The app itself is Rust, and Privy's login
modal, embedded wallet and signing prompts ship only as React, so the SDK runs
in one hidden React root with `PrivyProvider` and nothing else
(`crates/web/js/src/entry.js:65-78`). A small bridge reports the hook state to
the Rust code and hands it the login, signing and transaction calls
(`crates/web/js/src/bridge-core.js`, typed on the Rust side in
`crates/web/src/bridge/privy.rs`). The app refuses to render past a
configuration notice if `NEXT_PUBLIC_PRIVY_APP_ID` was not set at build time
(`crates/web/src/app.rs:21-22`; a release build fails outright without it,
`crates/web/src/config.rs:63-64`) — there is no code path that runs without
Privy configured.

The provider config, built in Rust and passed to the island
(`crates/web/src/bridge/privy.rs:49-66`), sets:

- `loginMethods: ["email", "passkey"]` (`:54`) — no wallet connector is
  offered. The only way in is a credential a stranger already has.
- `embeddedWallets: { ethereum: { createOnLogin: "users-without-wallets" } }`
  (`:55-57`) — Privy mints a wallet automatically for anyone who logs in
  without one. The user never sees a "create wallet" step; it happens as a side
  effect of logging in.
- `defaultChain` / `supportedChains: [chain]` (`:59-60`), where `chain` is
  Arc testnet (`crates/core/src/contracts.rs:54`) — the embedded wallet is
  provisioned directly on the chain the app actually uses, so there is no
  network-switching step either.

Privy is the only authentication in the product — there is no non-Privy
login anywhere in the tree. The sign-in control for claiming an inbox is
`SignedOutActions`, a component in `crates/web/src/account.rs:611-621`: its
button calls `start_login` (`crates/web/src/privy_context.rs:19`), which opens
the Privy modal. It is not the only place a login starts: the pay lane in
`crates/web/src/challenge_actions.rs` logs the sender in when an
unauthenticated sender tries to pay instead of proving personhood (`:659-660`)
— both paths go through Privy, consistent with the "only authentication" claim
above. `account.rs` gates the whole signed-in view on Privy's
`ready` and `authenticated` state (`account.rs:594`). There is no
separate account system, password, or session cookie to design around —
signing in and having a funded wallet are the same action.

## The functional financial flow

Beyond onboarding, Privy also carries the product's money-moving side: at
least one functional financial flow built on a generally available Privy
feature, not a bespoke integration. Postage has two, and both go through
`useSendTransaction` (`crates/web/js/src/entry.js:44`), a generally available
hook that needs no commercial Privy onboarding.

**1. Pay-to-send.** When a held message is challenged as automated mail, the
sender's embedded wallet sends the quoted amount as the transaction's `value`
(`U256::from(self.amount)`) directly to `PostageEscrow.payToSend`
(`crates/web/src/challenge_actions.rs:567-576`, with the call data built by
`pay_to_send_data` at `:160`). This is not a gas-token transfer dressed up as a
payment: on Arc, native `msg.value` *is* USDC, at 18 decimals
(`crates/core/src/format.rs:5` and `contracts.rs:60`, 18 decimals; the same
point is made in `ARCHITECTURE.md` under "Arc — where the money is": "native
USDC is 18 decimals, but the ERC-20 view of the same balance is 6"). So the one
transaction the sender signs is simultaneously the gas payment and the
stablecoin payment — there is no separate `approve` step, and no second asset
to hold.

**2. Claim earnings / set a floor price.** On the recipient side, the same
hook drives two more state-changing calls against the same contract:
`claimEarnings` withdraws what strangers have paid into the inbox, and
`setFloorPrice` changes what the inbox charges going forward
(`crates/web/src/inbox_panel.rs:77` and `:70` for the two calls, sent at
`:128`).

External evidence that flow 1 has actually settled on-chain: the live
`/network` page (`https://postage-seven.vercel.app/network`, fetched
2026-09-10) shows one settled payment of $0.03 on Arc testnet. The wallet
`0xdd769553802be81d4eb1f8588de4c120d318b38e` sent this payment and is verified
in the project's subgraph with `paidCount: 1`. This wallet executed the payment
via `useSendTransaction`, Privy's transaction hook — an embedded Privy wallet
initiating a real onchain transaction, not just holding a balance. (That
payment was made through the app's first, TypeScript implementation; the Rust
app sends the same transaction through the same hook, at
`crates/web/src/challenge_actions.rs:567-576`.) `PostageEscrow`
splits every payment `VAULT_BPS = 2_000` (20%) to the vault and the remainder to
the inbox (`contracts/src/PostageEscrow.sol:34`), which is exactly the $0.024
inbox / $0.006 vault split the page implies for a $0.03 payment.

## Server-side: the identity token

Privy's identity token is what lets a signed-in user claim an inbox without a
second proof step. The claims route accepts it as one of two ways to prove
wallet ownership: `session()` reads the header via `read_identity` and checks
the claimed wallet against the token's linked wallets
(`crates/server/src/routes/inbox.rs:273-280`); the alternative is the "long way"
— a wallet signature plus an emailed code — for anyone without a live token
(`inbox.rs:282-300`, and the Privy section of `ARCHITECTURE.md`).

Verification is hand-rolled: `read_identity` in `crates/server/src/privy.rs:219`
fetches Privy's public JWKS (`https://auth.privy.io/api/v1/apps/{app_id}/jwks.json`,
`privy.rs:40`) and `crates/core/src/privy.rs` parses the JWT, rejects any
algorithm but ES256 (`:27`, `:86`), and checks the signature against the
matching key locally (`:119-143`). This is not Privy's server SDK — nothing in
the workspace depends on it — and no Privy server secret is held or referenced;
`ARCHITECTURE.md` makes the same point about this code path ("No app secret, no
call out to Privy, no library: one signature check against a key anyone can
fetch"). The verification depends only on Privy's publicly fetchable signing
key.

## What this replaces

Without Privy, the stranger clicking the unlock link would need, before they
could pay one cent: a browser extension installed, a wallet created and
backed up (seed phrase written down and not lost), that wallet funded with a
gas token they'd have to acquire first, and — on most chains — a second asset
approved before the actual payment could go through. All of that is
disproportionate to the transaction it gates. Privy collapses it to a login
with a credential (email or passkey) the person already has, and the wallet
that results is already funded in the one unit the app needs, because Arc's
gas token is the payment token. The size of that gap — extension, seed
phrase, funded gas token, approval, vs. an email login — is the actual UX
improvement, not a claim about Privy in the abstract.

## Boundaries

What the app does not use: no Privy policies, signers, key quorums, or
intents, and no Privy Cards or Bridge — nothing in `crates/`,
`crates/web/js/package.json` or `ARCHITECTURE.md` uses them, and nothing
depends on `@privy-io/server-auth`. Every money-moving action in the product
goes through the client-side `useSendTransaction` hook against a raw contract
call (`challenge_actions.rs:567-576`, `inbox_panel.rs:128`); there is no
session-key or spending-policy layer between the user's approval and the
transaction. The deployment target throughout is Arc **testnet**, not
mainnet (`crates/core/src/contracts.rs:54`, `ARC_TESTNET`) — this is a
proof-of-concept, not a production financial product.

# Privy: how it improves the user experience

This is the answer to the prize requirement "Clearly explain how Privy improves
the user experience." The material exists scattered across the codebase and in
`ARCHITECTURE.md:237-265`; this document is the one place it is stated as an
argument.

## The user who matters

Postage has two kinds of user. The inbox owner is crypto-native by
construction — they claimed a wallet-backed address on purpose. The user this
document is about is the other one: a stranger who wrote a normal email, got
held, and received a reply containing an unlock link (`ARCHITECTURE.md:239`,
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

The product depends on `@privy-io/react-auth ^3.40.0`
(`web/src/app/providers.tsx` imports it; version pinned at
`web/package.json:16`). `PrivyProvider` wraps the entire app in
`web/src/app/providers.tsx:22-33`, and the app refuses to render past a
configuration notice if `NEXT_PUBLIC_PRIVY_APP_ID` is unset
(`web/src/app/providers.tsx:10-20`) — there is no code path that runs without
Privy configured.

The provider config (`web/src/app/providers.tsx:25-30`) sets:

- `loginMethods: ["email", "passkey"]` (`:26`) — no wallet connector is
  offered. The only way in is a credential a stranger already has.
- `embeddedWallets: { ethereum: { createOnLogin: "users-without-wallets" } }`
  (`:27`) — Privy mints a wallet automatically for anyone who logs in without
  one. The user never sees a "create wallet" step; it happens as a side effect
  of logging in.
- `defaultChain` / `supportedChains: [chain]` (`:28-29`), where `chain` is
  `arcTestnet` (`web/src/lib/contracts.ts:1,3`) — the embedded wallet is
  provisioned directly on the chain the app actually uses, so there is no
  network-switching step either.

Privy is the only authentication in the product. `SignInButton.tsx` is the
entire sign-in control: it destructures `login` from `usePrivy()` and wires it
straight to a button's `onClick` (`web/src/app/SignInButton.tsx:9,11`) — no
other login path exists in the component tree. `Account.tsx` gates the whole
signed-in view on `usePrivy()`'s `ready`/`authenticated`/`user`
(`web/src/app/Account.tsx:16`). There is no separate account system, password,
or session cookie to design around — signing in and having a funded wallet are
the same action.

## The functional financial flow

The prize's other requirement is "Complete at least one functional financial
flow using a generally available Privy feature." Postage has two, and both go
through `useSendTransaction`, a generally available hook that needs no
commercial Privy onboarding.

**1. Pay-to-send.** When a held message is challenged as automated mail, the
sender's embedded wallet sends
`value: BigInt(quote.amount)` directly to `PostageEscrow.payToSend`
(`web/src/app/c/[token]/ChallengeActions.tsx:191` for the hook,
`:232-247` for the call). This is not a gas-token transfer dressed up as a
payment: on Arc, native `msg.value` *is* USDC, at 18 decimals
(`web/src/lib/contracts.ts:10-13`, "Arc's native USDC is 18 decimals"; the same
point is made in `ARCHITECTURE.md:157-158`, "native USDC is 18 decimals, but
the ERC-20 view of the same balance is 6"). So the one transaction the sender
signs is simultaneously the gas payment and the stablecoin payment — there is
no separate `approve` step, and no second asset to hold.

**2. Claim earnings / set a floor price.** On the recipient side, the same
hook drives two more state-changing calls against the same contract:
`claimEarnings` withdraws what strangers have paid into the inbox, and
`setFloorPrice` changes what the inbox charges going forward
(`web/src/app/InboxPanel.tsx:43` for the hook, `:69-77` for both calls).

External evidence that flow 1 has actually settled on-chain: the live
`/network` page (`https://postage-seven.vercel.app/network`, fetched
2026-09-10) shows one settled payment of $0.03 on Arc testnet. The wallet
`0xdd769553802be81d4eb1f8588de4c120d318b38e` sent this payment and is verified
in the project's subgraph with `paidCount: 1`. This wallet executed the payment
via `useSendTransaction` (`web/src/app/c/[token]/ChallengeActions.tsx:191,232-247`),
Privy's transaction hook, routing it through Privy's infrastructure and
satisfying the requirement to "create or use at least one Privy wallet". `PostageEscrow`
splits every payment `VAULT_BPS = 2_000` (20%) to the vault and the remainder to
the inbox (`contracts/src/PostageEscrow.sol:34`), which is exactly the $0.024
inbox / $0.006 vault split the page implies for a $0.03 payment.

## Server-side: the identity token

Privy's identity token is what lets a signed-in user claim an inbox without a
second proof step. The claims route accepts it as one of two ways to prove
wallet ownership: `session()` reads the header via `readIdentity` and checks
the claimed wallet against the token's linked wallets
(`web/src/app/api/inbox/route.ts:55-58`); the alternative is the "long way" — a
wallet signature plus an emailed code — for anyone without a live token
(`web/src/app/api/inbox/route.ts:126-144`, `ARCHITECTURE.md:254-258`).

Verification is hand-rolled: `readIdentity` in `web/src/lib/privy.ts` decodes
the JWT, fetches Privy's public JWKS (`https://auth.privy.io/api/v1/apps/
<app-id>/jwks.json`, `web/src/lib/privy.ts:63`), and checks the ES256
signature against the matching key locally (`web/src/lib/privy.ts:149-211`,
in particular the `verify(...)` call at `:178-183` and the algorithm pin at
`:173`). This is not `@privy-io/server-auth` — a repo-wide search finds no
such import anywhere in `web/` — and no Privy server secret is held or
referenced; `ARCHITECTURE.md:250-252` makes the same point about this code
path ("No app secret, no call out to Privy, no library: one signature check
against a key anyone can fetch"). The verification depends only on Privy's
publicly fetchable signing key.

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
intents, and no Privy Cards or Bridge — a search of `web/src`,
`web/package.json`, and `ARCHITECTURE.md` for those terms and for
`@privy-io/server-auth` returns nothing. Every money-moving action in the
product goes through the client-side `useSendTransaction` hook against a raw
contract call (`ChallengeActions.tsx:232-247`, `InboxPanel.tsx:74-77`); there
is no session-key or spending-policy layer between the user's approval and
the transaction. The deployment target throughout is Arc **testnet**, not
mainnet (`web/src/lib/contracts.ts:3`, `arcTestnet`) — this is a
proof-of-concept, not a production financial product.

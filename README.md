# Postage

Email where attention has a price, and being human makes it free.

Sending email costs nothing, which is exactly why spam works. Postage puts a
price on unsolicited mail from strangers and waives it for anyone willing to
prove they are a real person.

## How it works

Every inbox sets its own postage price. The default is $0.01.

Mail from a stranger is held at the gateway until the sender does one of two
things:

- passes a World ID Selfie Check, and the message is delivered free
- escrows the postage in USDC, and the message is delivered

Once it lands, the recipient decides what the stamp was worth:

- **Release** — the message was legitimate, and the postage goes back to the
  sender
- **Claim** — it was spam, and the postage belongs to the recipient

Stamps nobody resolves expire after 14 days and refund the sender.

The result is asymmetric. A stranger acting in good faith pays nothing in the
end; they post a bond and get it back. A spammer pays every single person who
marks them.

## Why it needs all three

Postage is built on Arc, The Graph and World ID, and it does not work without
any one of them.

World ID's Selfie Check is the free lane. It is a liveness and uniqueness
signal built for exactly this — sign-up and bot defense where speed matters —
and its per-action nullifier means one person cannot farm unlimited free-send
identities. The credential lasts 90 days, so the free pass genuinely lapses.

Arc is the settlement layer. USDC is its native gas token, so postage and the
gas to move it are both denominated in dollars. A $0.01 stamp costs about
$0.009 to send. On a chain with a volatile gas token, a one-cent stamp is not a
coherent idea.

The Graph turns behaviour into a price. Every stamp, refund, claim and identity
attestation lands onchain and gets indexed, which gives every sender a public
spam rate. Combined with their history on other chains, that sets what they pay
next time: an established sender who has never been flagged sends for almost
nothing, and a wallet funded ten minutes ago pays full price.

Identity is proven off-chain, recorded onchain, indexed, and priced. Each piece
feeds the next.

## Status

Built for ETHOnline 2026. Work in progress.

## Layout

    contracts/   escrow and identity registry (Foundry)
    subgraph/    indexes both contracts on Arc
    web/         inbox, compose, unlock page
    gateway/     inbound mail webhook and hold/release logic

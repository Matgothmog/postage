# Postage

A filter in front of the inbox you already have.

Give out `you@usepostage.com`. Mail sent there is read, judged, and forwarded to
the address you actually use. You do not change email provider, and you do not
learn a new inbox.

## What the filter does

Every message is classified as one of four things, and that decides what
happens to it.

| What it is | What happens |
| --- | --- |
| Written by a person | Delivered free. The sender never sees the gate again. |
| Something you are waiting for — a login code, a receipt, a delivery update | Delivered free, immediately |
| Ordinary automated mail — newsletters, marketing | Held. The sender pays your price, and it goes to you |
| Trying to deceive you | Never delivered. Charged if a wallet is attached |

A stranger is refused once, inside SMTP, with a link. They prove they are a
person and it costs nothing, or they pay. Either way they join your allowlist
and never hit the gate again. **One refusal per sender, ever.**

The money is yours. Marketing that wants your attention pays for it, and you
withdraw it to a wallet.

## Why it needs all of this

**World ID** is the free lane. A liveness and uniqueness check built for exactly
this — bot defence where speed matters — and its per-action nullifier means one
person cannot mint themselves unlimited free senders.

**Arc** is where the money is. USDC is its native gas token, so a one-cent price
and the half-cent of gas that moves it are quoted in the same unit. On a chain
with a volatile gas token, a one-cent price is not a coherent idea.

**The Graph** decides what a sender pays. Every payment, every verdict, and
every time a recipient contradicted the classifier is indexed, and that history
prices the next message. A sender who has never been reported pays the minimum;
one who has pays several times it.

**Privy** gives a wallet to people who do not have one. The person clicking the
unlock link is a stranger with no wallet and no reason to install one.

## The price cannot be made up

A price is only valid if it carries a signature from a key listed in
`EnclaveRegistry`, and the escrow rejects anything else. The classifier signs
what it decided; the chain refuses to charge you on anyone else's say-so.

Today that key belongs to an ordinary server process, and the registered
measurement says so:
`keccak256("stage1-plain-classifier-not-attested")`. The next step is a Nitro
enclave, where the measurement becomes a hash of the running image that anyone
can recompute from source — the interface does not change, only who is allowed
to sign.

## What is not stored

No message is ever written down. A held message leaves a row recording who
wrote to whom and what it would cost — never the subject, never the body. The
mail itself is refused at the door and lives only in the sender's outbox until
they resend it.

## Layout

    contracts/   escrow, enclave registry, identity registry, vault
    subgraph/    indexes all four on Arc
    web/         signup, challenge page, and the API behind them
    worker/      the Cloudflare mail worker

[ARCHITECTURE.md](ARCHITECTURE.md) explains how the parts fit together and why
each is there. Addresses are in [DEPLOYMENTS.md](DEPLOYMENTS.md).

Built for ETHOnline 2026.

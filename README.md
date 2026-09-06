# Postage

A filter in front of the inbox you already have, and a price on the mail that
wastes your time.

Give out `you@usepostage.com`. Mail sent there is read, judged, and forwarded to
the address you actually use. You do not change email provider, and you do not
learn a new inbox.

## What Postage is for

Spam is cheap to send and expensive to receive, and every filter ever built has
tried to fix that by getting better at guessing. Postage does something else:
it makes the sender carry the cost. Marketing that wants your attention pays
for it, and the money is yours.

That is the whole idea. Everything below is in service of making a one-cent
charge on a stranger's email actually work.

Every stranger is held. The classifier does not decide whether to hold you, it
decides **who pays to get through**.

| What it is | What happens |
| --- | --- |
| Something you are waiting for — a login code, a receipt, a delivery update | Delivered at once, free. Never held. |
| Written by a person | Held. Proving you are a person clears it for nothing. |
| Ordinary automated mail — newsletters, marketing | Held. Nobody proved a person is behind it, so it pays. |
| Trying to deceive you | Never delivered. Being a person does not clear it, and paying is a penalty. |

A stranger is refused inside SMTP with a link. They prove they are a person and
it costs nothing, or they pay, and their message goes through. The refusal is
not a bounce into nowhere — their mail is still in their outbox, and the
challenge page will deliver it for them if they paste it back in.

**A pass runs out.** Proving personhood opens a fifteen minute window; paying
buys one delivery. Writing again tomorrow means proving it again. World ID is a
check that someone was there a moment ago, not a badge an address keeps, and
until liveness detection is good enough to say otherwise this treats it that
way.

## Why it needs all of this

**Arc** is where the money is. USDC is its native gas token, so a one-cent price
and the half-cent of gas that moves it are quoted in the same unit. On a chain
with a volatile gas token, a one-cent price is not a coherent idea.

**World ID** is the free lane, and it is asked every time. A liveness and uniqueness check built for exactly
this — bot defence where speed matters — and its per-action nullifier means one
person cannot mint themselves unlimited free senders.

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

## Nothing to set up

Claim a handle and you are done. An inbox charges one cent before its owner has
picked a price, so the first message is charged for without a transaction, a
balance, or a decision. Changing the price later is one call.

## This is a proof of concept

It runs, it charges real testnet USDC, and the parts do what this README says
they do. It is not a service you should point your real mail at yet.

**Message bodies are never stored.** A held message leaves a row recording who
wrote to whom and what it would cost — never the subject, never the body. The
mail is refused at the door and lives only in the sender's outbox until they
resend it.

**Not stored is not the same as not seen.** Cloudflare receives the message, the
gateway parses it, and the classifier reads it. That is not a gap in the
implementation; it is what SMTP is. Mail arrives in plaintext, so whatever
terminates the connection holds it. Closing that properly means running the MTA
itself inside an enclave — see [ARCHITECTURE.md](ARCHITECTURE.md#what-privacy-would-actually-take).

## Layout

    contracts/   escrow, enclave registry, identity registry, vault
    subgraph/    indexes all four on Arc
    web/         signup, challenge page, and the API behind them
    worker/      the Cloudflare mail worker

[ARCHITECTURE.md](ARCHITECTURE.md) explains how the parts fit together and why
each is there. Addresses are in [DEPLOYMENTS.md](DEPLOYMENTS.md). What is known
to be wrong with it is in [KNOWN_ISSUES.md](KNOWN_ISSUES.md).

Built for ETHOnline 2026.

# Postage

A filter in front of the inbox you already have, and a price on the mail that
wastes your time.

Give out `you@usepostage.com`. Mail sent there is read, judged, and forwarded to
the address you actually use. You do not change email provider, and you do not
learn a new inbox.

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

Say a person wrote it and the message goes — nothing today actually checks that
one did. **No wallet, no account, nothing to sign up for** — the free lane
should not charge a toll in setup. A machine pays instead, and only then is
there anything to create an account for, because only then is there money to
move.

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
Developer Portal, and only a verified proof puts the attestation onchain —
today that runs against World's sandbox environment, not production.

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

[ARCHITECTURE.md](ARCHITECTURE.md) explains how the parts fit and why each is
there. Addresses and endpoints are in [DEPLOYMENTS.md](DEPLOYMENTS.md).

Built for ETHOnline 2026.

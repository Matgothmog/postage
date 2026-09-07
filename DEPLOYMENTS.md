# Deployments

## Arc Testnet (5042002)

| Contract | Address | Deployed at block |
| --- | --- | --- |
| PostageVault | `0xd488a385529e9eec44a17b686f3b9372071f22dc` | 60731753 |
| EnclaveRegistry | `0xf6afced17443571c79036f9d543ddbc2c5a645f9` | 60731753 |
| HumanRegistry | `0x0f9a1c7e971df81adc1b0335a527b30b6f136d05` | 60731753 |
| PostageEscrow | `0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7` | 60744660 |

Explorer: https://testnet.arcscan.app

### The escrow was redeployed

It moved from `0x5dcf371c3959de729ca637628dc574133ab42795` in
[`0x94882e9a`](https://testnet.arcscan.app/tx/0x94882e9aa3dc066fc0cf6249ef4f4be74a96923bea1a555285db4379e38e7d20).
Three ABI-breaking changes:

- `effectiveFloor(address)` — an inbox that never set a price charges
  `DEFAULT_FLOOR`, one cent, rather than nothing, and `payToSend` enforces it.
- `reportSpam(bytes32)` — takes only the message id. The escrow records who paid
  in `settlementOf`, so a report is checked against the payment rather than
  trusting an address the caller passed. Only the receiving inbox may report, and
  only once.
- `settled(bytes32)` is a view over `settlementOf` rather than its own mapping.
  Same signature, same meaning.

`script/DeployEscrow.s.sol` deploys the escrow alone against the registry and
vault already live, which `Deploy.s.sol` would have replaced — orphaning the
registered signing key and the vault's balance. Nothing was stranded: the old
escrow held a zero balance, because earnings are claimed to the inbox owner
rather than accumulated. The old address is dead; anything still pointing at it
reverts on `effectiveFloor`.

### Registered signer

The escrow accepts prices only from keys listed in `EnclaveRegistry`. The
measurement currently registered is

    keccak256("stage1-plain-classifier-not-attested")

which names what it is: **the classifier runs as an ordinary process, so this is
not a hardware measurement.** Stage 2 replaces it with a real PCR0 from a Nitro
enclave and revokes this key. The interface does not change between the two, only
who is allowed to sign.

### Verified onchain

Run against `0x4469e8` in
[`0xd6a36eae`](https://testnet.arcscan.app/tx/0xd6a36eae30aafc30d13a0d8c80563077c875c25e4a9bfcf8e32ed2ec2149b7b5)
and the calls around it.

| | |
| --- | --- |
| Payment with a registered signature | accepted, 0.03 USDC |
| Split | 0.024 to inbox earnings, 0.006 to vault |
| Payment with an unregistered signature | reverted `UnknownEnclave` |
| Quote under the floor, for an inbox that never set one | reverted `BelowFloor` |
| Spam reported by anyone but the recipient | reverted `NotTheRecipient` |
| Recipient reporting the message | accepted |
| The same message reported twice | reverted `AlreadyReported` |
| The same quote spent twice | reverted `AlreadySettled` |

### Measured gas at 25 gwei

| Call | Gas | Cost |
| --- | --- | --- |
| setFloorPrice | 45,147 | 0.0011 USDC |
| payToSend | 128,851 | 0.0032 USDC |
| reportSpam | 31,425 | 0.0008 USDC |
| attest (HumanRegistry) | 75,395 | 0.0019 USDC |

`payToSend` costs more than the old escrow's ~120,000 because it records who paid
rather than a single bit. That is the storage a spam report is checked against,
and it is what stops reputation being writable by anyone.

### Vault economics

A payment gives 20% to the vault, split 30/70 between treasury and the pool that
pays gas for people who verify. Sponsoring one attestation costs 0.00188 USDC, so
a single commercial message priced at 0.03 funds roughly two verifications.

## Subgraph

Studio: https://thegraph.com/studio/subgraph/usepostage

    https://api.studio.thegraph.com/query/1758667/usepostage/v0.4.0

Indexes all four contracts, including which signing keys may price mail and under
what measurement. v0.4.0 follows the escrow to `0x4469e8` from block 60744660;
v0.3.0 still points at the dead one and returns nothing for any sender.

Confirmed indexing the settlement made against the new escrow: the payment splits
0.024 to the inbox and 0.006 to the vault, and carries `reportedAsSpam: true`
attributed to the wallet the escrow recorded as having paid. The sender then
reads `paidCount 1, spamReports 1, spamRate 1`, so the pricing engine quotes them
at five times the floor rather than the minimum.

## The mail path

    worker     https://postage-mail.postage-worker.workers.dev
    held mail  KV namespace 332d86ab54b3480492e8241c0e18b9cf, binding HELD
    gateway    postage-seven.vercel.app

The worker serves exactly one route, `POST /release`. Everything else answers
404, and 401 without the shared secret. `MAIL_WORKER_URL` on the gateway points
at it.

Held mail is in KV rather than R2 because R2 is not enabled on the account, and
it turned out to be the better fit: each value is written with the hold's
deadline attached, so Cloudflare drops one nobody answered at exactly that
moment.

### The relay carries a message without touching it

`usepostage.com` is active on Mailgun for sending. Three records, all valid:

| Record | Name | Note |
| --- | --- | --- |
| TXT | `usepostage.com` | SPF, **merged** into the Cloudflare record rather than added beside it |
| TXT | `mailo._domainkey.usepostage.com` | DKIM |
| CNAME | `email.usepostage.com` | tracking; unused, we send with tracking off |

**Mailgun's MX records are deliberately absent**, which is why the domain reads
`valid=unknown` against them. The MX for `usepostage.com` is Cloudflare Email
Routing, and that is what delivers mail to the worker — adding Mailgun's would
stop every message reaching Postage.

Open, click and unsubscribe tracking are off at the domain level as well as
per-message, so link rewriting cannot come back by someone dropping a flag.
Rewriting a link changes the body, and the body is what the sender's signature
covers.

Verified against the live API rather than assumed: a raw message with
`From: Sara Müller <sarah@example.org>` — a domain this account does not own,
carrying its own DKIM signature — was accepted on `/messages.mime` and logged
`accepted` then `delivered`, in test mode so nothing left the building. Mailgun
returned the message's own `Message-ID` rather than minting one, and logged the
sender and subject as written.

Cloudflare's own `send_email` cannot do this. It rejects raw MIME whose envelope
sender does not match the `From:` header, and that address must be on a domain
the account owns — `From: header does not match mail from`. Rewriting `From:` is
the one edit a forward must never make, so the release goes through Mailgun.

### Credentials

The worker sends as key `55613b82-53c4b5eb` — `kind=domain`, `role=sending`,
scoped to `usepostage.com`. It can send and read domains and nothing else; it
cannot list the account's keys.

Creating one through the API needs `kind=domain` alongside `role=sending`. Pass
`role=sending` on its own and Mailgun quietly issues an **admin** key instead.

### Proven end to end

A message from Gmail to a live handle, on the deployed stack: held in KV, replied
to inside its own SMTP session, and released through Mailgun when the sender said
a person wrote it. The worker logged the inbound and the `POST /release`
thirty-three seconds apart, KV was empty afterwards, and the challenge row read
resolved with `held_until` null.

## Arc Mainnet (5042)

Not deployed yet.

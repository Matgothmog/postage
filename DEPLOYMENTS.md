# Deployments

## Arc Testnet (5042002)

Deployed at block 60731753.

| Contract | Address |
| --- | --- |
| PostageVault | `0xd488a385529e9eec44a17b686f3b9372071f22dc` |
| EnclaveRegistry | `0xf6afced17443571c79036f9d543ddbc2c5a645f9` |
| PostageEscrow | `0x5dcf371c3959de729ca637628dc574133ab42795` |
| HumanRegistry | `0x0f9a1c7e971df81adc1b0335a527b30b6f136d05` |

Explorer: https://testnet.arcscan.app

### Registered signer

The escrow accepts prices only from keys listed in `EnclaveRegistry`. The
measurement currently registered is

    keccak256("stage1-plain-classifier-not-attested")

which names what it is: **Stage 1 runs the classifier as an ordinary process,
so this is not a hardware measurement.** Stage 2 replaces it with a real PCR0
from a Nitro enclave and revokes this key. The contract interface does not
change between the two — only who is allowed to sign.

### Verified onchain

| | |
| --- | --- |
| Payment with a registered signature | accepted, 0.03 USDC |
| Split | 0.024 to inbox earnings, 0.006 to vault |
| Payment with an unregistered signature | reverted `UnknownEnclave` |

### Measured gas at 25 gwei

| Call | Gas | Cost |
| --- | --- | --- |
| setFloorPrice | ~45,000 | 0.0011 USDC |
| payToSend | ~120,000 | 0.0030 USDC |
| attest (HumanRegistry) | 75,395 | 0.0019 USDC |

### Vault economics

A payment gives 20% to the vault, split 30/70 between treasury and the pool
that pays gas for people who verify. Sponsoring one attestation costs
0.00188 USDC, so a single commercial message priced at 0.03 funds roughly
two verifications.

## Subgraph

Studio: https://thegraph.com/studio/subgraph/usepostage

    https://api.studio.thegraph.com/query/1758667/usepostage/v0.2.0

Currently indexes the previous contracts; repointing at the addresses above is
day 2 work.

## Arc Mainnet (5042)

Not deployed yet.

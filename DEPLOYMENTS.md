# Deployments

## Arc Testnet (5042002)

Deployed at block 60568030.

| Contract | Address |
| --- | --- |
| PostageVault | `0x771f3da6d0d05f904fd28a782bfafdf18393aa90` |
| PostageEscrow | `0x164f432fd08dd4611172aa077882859b7c3ead7f` |
| HumanRegistry | `0x1b83c30c4138ca29a942f1a14237881daa7320d9` |

Explorer: https://testnet.arcscan.app

Measured gas at the 25 gwei the network was charging:

| Call | Gas | Cost |
| --- | --- | --- |
| setPrice | 45,154 | 0.0011 USDC |
| postStamp | 116,016 | 0.0029 USDC |
| claim | 40,201 | 0.0010 USDC |
| attest | 75,395 | 0.0019 USDC |

### Vault economics

A claimed stamp gives 20% to the vault, split 30/70 between the treasury and
the pool that pays gas for people who verify.

Sponsoring one attestation costs 0.00188 USDC, so a stamp has to be worth
about 0.0135 USDC before it funds one. The base rate of 0.01 does not reach
that, but base rate is what well-behaved senders pay and their mail gets
released rather than claimed, so it never funds the vault anyway. Spam is
priced at five times base, and one claimed spam stamp at 0.05 pays for
roughly 3.7 verifications.

## Subgraph

Studio: https://thegraph.com/studio/subgraph/usepostage

    https://api.studio.thegraph.com/query/1758667/usepostage/v0.2.0

Indexes all three contracts on arc-testnet from block 60568030.

## Arc Mainnet (5042)

Not deployed yet.

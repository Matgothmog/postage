# Deployments

## Arc Testnet (5042002)

Deployed at block 60558189.

| Contract | Address |
| --- | --- |
| PostageEscrow | `0xa17f3d284ce462e049c54b98aee55b7739b06c16` |
| HumanRegistry | `0xfcdca9fbd11dd826e459eddccf8f0581de77ab46` |

Explorer: https://testnet.arcscan.app

Measured gas, at the 25 gwei the network was charging:

| Call | Gas | Cost |
| --- | --- | --- |
| setPrice | 45,154 | 0.0011 USDC |
| postStamp | 116,016 | 0.0029 USDC |
| claim | 40,201 | 0.0010 USDC |

A full stamp cycle costs about half a cent to move a one cent stamp.

## Subgraph

Studio: https://thegraph.com/studio/subgraph/usepostage

    https://api.studio.thegraph.com/query/1758667/usepostage/v0.1.0

Indexes both contracts on arc-testnet from block 60558189.

## Arc Mainnet (5042)

Not deployed yet.

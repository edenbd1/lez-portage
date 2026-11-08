wrap_bridge gas, LEZ v0.3 (lee db66590a), anvil vectors at block 0xc

| transaction | gas | share of the 10,000,000 cap |
|---|---:|---:|
| sextant_lc Init (devnet checkpoint) | 1831160 | 18.31% |
| Init (bridge configuration) | 27049 | 0.27% |
| Register (entry + chained token NewFungibleDefinition) | 90958 | 0.91% |
| Claim, 1 ETH (path + chained Attest + chained token Mint) | 297457 | 2.97% |
| Claim, 100 USDT | 297529 | 2.98% |
|   of which: the same Attest sent alone | 111122 | 1.11% |
| Burn, 1 ETH (burn-tree append + chained ATA Burn -> token Burn) | 228728 | 2.29% |
| Freeze (one token) | 34614 | 0.35% |

Claim instruction: 2723 bytes (the storage proof is 3 account nodes, 4 storage nodes; the path is 24 x 32 bytes)
Nullifier account: 1 byte; token entry: 150 bytes; burn tree frontier: 812 bytes

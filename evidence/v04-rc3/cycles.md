| transaction (bridge.rs, ETH; the claim into an existing ATA holding) | cycles | applied |
|---|---:|---|
| Init (sextant_lc) | 1815646 | true |
| Init | 64456 | true |
| Register | 86985 | true |
| Register | 86491 | true |
| ATA Create | 45776 | true |
| Claim | 295139 | true |
| Burn | 188520 | true |

| transaction (erc20.rs, USDT, 6 decimals) | cycles | applied |
|---|---:|---|
| Init (sextant_lc) | 1815646 | true |
| Init | 64456 | true |
| Register | 86985 | true |
| Register | 86491 | true |
| ATA Create | 45780 | true |
| Claim | 294776 | true |
| Burn | 188308 | true |

On the local rc3 sequencer (evidence/v04-rc3/e2e-seq, pre-check cycles): a claim whose destination has no holding yet (the claim creates the ATA holding): 317,923 (1 ETH), 317,565 (1,000 USDT); into an existing holding: 295,456 (100 USDT), 295,821 (1 ETH); burns 188,257 to 188,520.
The private claim and burn: evidence/v04-rc3/e2e-seq/private-claim.json and private-burn.json (docs/V04-PORT.md section 0.4).

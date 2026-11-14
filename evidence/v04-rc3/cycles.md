| transaction (bridge.rs, ETH; the claim into an existing ATA holding) | cycles | applied |
|---|---:|---|
| Init (sextant_lc) | 1815646 | true |
| Init | 64554 | true |
| Register | 87164 | true |
| Register | 86670 | true |
| ATA Create | 45776 | true |
| Claim | 294856 | true |
| Burn | 187671 | true |

| transaction (erc20.rs, USDT, 6 decimals) | cycles | applied |
|---|---:|---|
| Init (sextant_lc) | 1815646 | true |
| Init | 64554 | true |
| Register | 87164 | true |
| Register | 86670 | true |
| ATA Create | 45780 | true |
| Claim | 294491 | true |
| Burn | 187459 | true |

On the local rc3 sequencer (evidence/v04-rc3/e2e-seq, pre-check cycles): a claim whose destination has no holding yet (the claim creates the ATA holding): 311,465 (1 ETH), 311,103 (1,000 USDT); into an existing holding: 288,996 (100 USDT), 289,361 (1 ETH).

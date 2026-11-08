| input | blocks | bytes | burns counted | user cycles | total cycles | segments | user cycles/byte |
|---|---:|---:|---:|---:|---:|---:|---:|
| real v0.4.0-rc1 blocks 18-28 (sequencer) | 11 | 28186 | 2 | 283753 | 524288 | 1 | 10.07 |
| 1 x (18 public burns, 1 reverted) | 1 | 23227 | 17 | 1241142 | 1572864 | 2 | 53.44 |
| 10 x (18 public burns, 1 reverted) | 10 | 232270 | 170 | 11951298 | 12845056 | 13 | 51.45 |
| 50 x (18 public burns, 1 reverted) | 50 | 1161110 | 850 | 59504775 | 63438848 | 61 | 51.25 |
| 1 x (4 private txs, 223,551-byte proofs) | 1 | 915857 | 0 | 1445810 | 3145728 | 3 | 1.58 |
| 5 x (4 private txs, 223,551-byte proofs) | 5 | 4579285 | 0 | 5803178 | 10747904 | 11 | 1.27 |
| 10 x empty (fee + clock only) | 10 | 4850 | 0 | 92533 | 131072 | 1 | 19.08 |

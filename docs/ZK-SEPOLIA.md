# Zero-knowledge release verified on Sepolia (LEZ and Bedrock local, the public LEZ testnet halted)

The outbound release that `docs/OUTBOUND.md` §5 runs on anvil, run with its
Ethereum side on the **public Sepolia testnet**: a vault whose release
verifier is `ZkBedrockBurnVerifier` pays out carol's private burn on one
Groth16 seal that proves, together, that the burn is a leaf of the burn tree
rebuilt from LEZ blocks, that those blocks run from the anchor the verifier
pins, and that the last of them is inscribed on Bedrock, signed by the pinned
sequencer key, with `MIN_DEPTH` Bedrock headers above it. The release carries
no leaf, secret, commitment or Merkle path.

**What is public, and what is local.** The public LEZ testnet stopped
producing blocks on 2026-11-17 (tip 83005, 23:26 UTC; only 3 blocks after
03:14 UTC that day) and its blocks stopped being inscribed on the public
Bedrock testnet (LEZ 83002 inscribed a day late, 83003 to 83005 not at all),
so a new burn could not be anchored on public Bedrock. Here:

| part | where | how to check it |
|---|---|---|
| the RISC Zero Groth16 verifier, `ZkBedrockBurnVerifier`, the vault | **Sepolia (public)**, source verified on Sourcify | `scripts/verify-zk-sepolia.py` reads them back and checks every pinned value |
| the two deposits, carol's release, the refusals | **Sepolia (public)** | the same script: receipts, the calldata's journal decoded, the seal re-verified today by the deployed verifier (`eth_call`), the committee's two signatures recovered |
| the Sepolia checkpoint Sextant was initialised from, the deposit's storage proof | **Sepolia (public)**, read into `evidence/zk-sepolia/` | as in `docs/TESTNET.md` |
| the LEZ v0.3.1 sequencer (Sextant `Init`, the bridge's `Init`, the claims, carol's private burn) | **local**, built without `standalone`, inscribing every block on the devnet | `evidence/zk-sepolia/run.tsv`, `outbound.tsv`; the chain proof's blocks |
| the Bedrock devnet (LEZ v0.3.1's devnet configuration, one validator, k = 5, proofs of leadership in dev mode) | **local** | the fetched blocks were checked against the node's own types (`bedrock-tools check`); the anchor is committed |

**Deployed source.** The three contracts were compiled from commit `f274338`
(Sourcify `exact_match`): a verifier pinning one sequencer key and the vault
before its per-token escrow bound and release delay (`docs/FAILSAFE.md`). The
current sources pin the channel's key set and add the escrow bound and the
release delay; a redeploy carries them. `scripts/verify-zk-sepolia.py` reads
the deployed contracts, so it re-checks this run against what is on chain.

**What is still trusted** (as on anvil, `docs/OUTBOUND.md` §5):
- the **committee** (2 of 3 keys, one operator here) attests only that the
  top Bedrock header the journal names is on its node's canonical chain;
  nothing about LEZ or the burn;
- the **sequencer key**: the verifier deployed here pins one key (the devnet
  sequencer's). On the public testnet the LEZ channel now has **four
  accredited keys**, posting in turns (a12da31c, 06abcb81, 0d27b709,
  30cfd05a; the threshold of three is the channel's configuration threshold,
  not a posting rule). The current `ZkBedrockBurnVerifier` source **pins the
  channel's key set** instead (`docs/BEDROCK-FINALITY.md`, "The channel's key
  set"); a redeploy carries it;
- LEZ blocks' **validity** rests on the sequencer and LEZ nodes; only
  **private** burns are derivable from blocks.

Forging a release needs the committee threshold and the sequencer's key
together. F.7 is still not met as the RFP words it: a full Bedrock light
client is the remaining step (`docs/BEDROCK-FINALITY.md`, `docs/POL-MEASURED.md`).

## The run

2026-11-18, `evidence/zk-sepolia/` (Apple M4 Pro, 24 GiB; real proving, RISC0
dev mode off; the Groth16 step in RISC Zero's x86_64 image under Docker).

**Sepolia (public; contracts source-verified on Sourcify, exact match):**

| what | address / transaction |
|---|---|
| RISC Zero `RiscZeroGroth16Verifier` (risc0-ethereum v3.0.1, as vendored) | [`0x033bEF27626546b6e47914A2E59B4f44f7BDCc7f`](https://sepolia.etherscan.io/address/0x033bEF27626546b6e47914A2E59B4f44f7BDCc7f) |
| `ZkBedrockBurnVerifier` (2-of-3 header committee, `MIN_DEPTH` 5) | [`0xf02D0456A0E5e1B023081D01686C67d506E94F44`](https://sepolia.etherscan.io/address/0xf02D0456A0E5e1B023081D01686C67d506E94F44) |
| `WrapVault` | [`0x1b2aBe9Ebe3769Ff0615fe7Cae313F8389189223`](https://sepolia.etherscan.io/address/0x1b2aBe9Ebe3769Ff0615fe7Cae313F8389189223) |
| deposits, 0.001 ETH each (alice, carol) | [`0x08eb0fff…`](https://sepolia.etherscan.io/tx/0x08eb0fff4d466abc83c960bc5cad0cf5d5ad1db0208a41b1e2420133294d5859), [`0x6b7dbe3a…`](https://sepolia.etherscan.io/tx/0x6b7dbe3ab2545e8640265ba3770c7581d6b6bca16abf407c3a4498a154197c32) |
| **carol's release, applied**: 0.001 ETH to the address her burn committed to, 717,185 gas, calldata 1,924 bytes, seal 260 bytes; no leaf, secret or commitment in it | [`0xf6ac3411…`](https://sepolia.etherscan.io/tx/0xf6ac341150d27a5db9470d8f204c91186c9429a79fccfafda75bb5e0373dfc26) (block 12,160,799) |
| refused: one member signs (`HeaderNotAttested`) | [`0x694db597…`](https://sepolia.etherscan.io/tx/0x694db5973840a0b6eb90286851823c7436d17e8c045197d381c1e6481aed3c6d) |
| refused: the recipient changed in the journal (`VerificationFailed`: the seal no longer proves it) | [`0x213d4961…`](https://sepolia.etherscan.io/tx/0x213d4961b534c9c078f29b61b0787b4ea9afd1d30da4670315c8529484de378f) |
| refused: the replay (`AlreadyReleased`) | [`0x141e4bbd…`](https://sepolia.etherscan.io/tx/0x141e4bbd89b2b1cd6e17708b3937cc51ff3f8f2a6fc9f228da6ea7ae4b0ac2e0) |

The vault paid out exactly carol's 0.001 ETH and holds alice's (a public
burn cannot be released this way: only private burns are derivable from
blocks). Anyone may resend the release's calldata; it pays the relayer the
proof names, not the sender.

**Local LEZ and Bedrock.** Sextant `Init` from the Sepolia checkpoint at
slot 11,605,088 (execution block 12,160,623, finalized after both
deposits), the bridge's `Init` and `Register`, alice's public claim (432,666
gas) and two refusals (`E7402` replay, `E7208` unknown root), carol's
private claim (proven in 311 s, 257,199 bytes) and private burn (215 s,
248,358 bytes, LEZ block 645); the burn tree rebuilt from blocks 323 to 701
holds carol's burn only.

| proof | blocks | user cycles | segments | STARK | Groth16 | host peak |
|---|---:|---:|---:|---:|---:|---:|
| chain, step 1 (blocks 323 to 644, 538,625 bytes) | 322 | 2,134,643 | 3 | 270 s | | 8.2 GB |
| chain, step 2 (645 to 701, its prior verified inside) | 57 | 753,503 | 1 | 93 s | | 8.0 GB |
| anchor (Bedrock block `3da10107…`, slot 9,503,328, LEZ 701 inscribed, 5 headers above) | | 1,154,059 | 2 | 141 s | | 7.0 GB |
| release (the anchor receipt verified inside), one seal for the three | | 347,157 | | 58 s | 60 s (emulated), 7.2 GiB container | |

The members' Bedrock node had the journal's top header (`c9b3e8a1…`) on its
canonical chain; 485 Bedrock blocks down to the inscription were decoded
with the node's own types and agreed with lez-bedrock.

`scripts/verify-zk-sepolia.py`: **PASS, 0 failed** from Sepolia's public RPC
(`evidence/zk-sepolia/verify.txt`) and again from the recording
`evidence/archive/zk-sepolia.jsonl.gz` (`verify-archived.txt`). Its seal
check fails, as it should, on another digest, another image id and a
flipped seal byte.

A first vault (`0xf845847f…`) and Sextant instance were abandoned before
any LEZ claim: the Sextant client read the committed checkpoint instead of
the fresh one, so that instance's header predated the deposits; its 0.002
ETH stays locked there. See `evidence/zk-sepolia/run.tsv`.

## Re-run it

```console
scripts/verify-zk-sepolia.py                         # the Sepolia side, from public RPC, and the committed local evidence
python3 scripts/rpc-archive.py serve evidence/archive/zk-sepolia.jsonl.gz 8646 &
scripts/verify-zk-sepolia.py --eth-rpc http://127.0.0.1:8646   # the same, from the recording (offline)
```

To make it again: a Bedrock devnet (`scripts/bedrock-devnet.sh`), then
`MODE=local BEDROCK_NODE=… scripts/testnet-inbound.sh` stages `local-up`,
`lez-deploy`; `wrap chain-params` (the anchor, and the burn tree of the
configuration that names the vault's predicted address); `sepolia_vault.py
deploy-zk` and `deposit`; stages `fetch`, `sextant`, `claims`;
`PRIVATE=1 scripts/testnet-outbound.sh burn`; `scripts/zk-sepolia.sh`.
`SEXTANT_REPO` must be the checkout the `sextant` binary was built in: the
client reads `vectors/sepolia-portage` from its build directory, which is
how the first instance came to be initialised from the committed checkpoint.

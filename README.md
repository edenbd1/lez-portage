# Wrapped ERC-20 and Ether bridge for LEZ (RFP-022 prototype)

A lock-and-mint bridge between an Ethereum vault and a LEZ v0.3 program,
whose inbound leg is verified by Sextant, the Ethereum light client for LEZ
(RFP-021, [edenbd1/lez-sextant](https://github.com/edenbd1/lez-sextant) at
`main`, `8153272`), and never by a signer or federation.

- `vault/`: `WrapVault.sol` (Foundry). Escrows registered ERC-20s and native
  ETH in fixed denominations, against a hiding commitment: the LEZ destination
  appears in no argument, event or state (F.2). Each deposit appends a leaf to
  a depth-24 SHA-256 tree, and every root the tree has had is recorded at
  `knownRoot[root]`, storage slot 0. Transfers work with tokens that return
  nothing (USDT, F.17); fee-on-transfer tokens and `false` returns are refused.
  Per-token caps per window, per-token and global freezes for each direction,
  releases paid on a proof checked by a verifier fixed at construction
  (Security #8), each nullifier once. There are two verifiers.
  `ZkBurnVerifier.sol` checks a Groth16 proof that the burn is in the LEZ
  burn tree, without the leaf, its index or its path. RISC Zero's own
  `RiscZeroGroth16Verifier` (risc0-ethereum v3.0.1, vendored) checks the proof
  against the pinned guest ImageID, and the guest's journal must hash to the
  vault's release digest. `CommitteeBurnVerifier.sol` checks the burn leaf's
  opening in public instead. Both check the burn tree's root against a fixed
  k-of-n committee's attestation that the root is final
  (`BurnRootCommittee.sol`). A third, `ZkChainBurnVerifier.sol`, takes no
  root from anyone. One Groth16 seal proves the burn under a tree rebuilt
  from LEZ blocks, and its committee (`LezBlockCommittee.sol`) only attests
  that the last block is final. A fourth, `ZkBedrockBurnVerifier.sol`, also
  proves that last block inscribed on Bedrock by the pinned sequencer key, k
  headers deep, and its committee (`BedrockHeaderCommittee.sol`) only attests
  one Bedrock header canonical. **That attestation is still a trust
  assumption, so F.7 is not met** ([docs/OUTBOUND.md](docs/OUTBOUND.md),
  [docs/BEDROCK-FINALITY.md](docs/BEDROCK-FINALITY.md)).
- `zk/burn_membership/`: the RISC Zero guest of the zero-knowledge release. It
  proves knowledge of a burn (token, amount, recipient, fee, secret, index,
  path) under the root its path leads to, and commits the vault's release
  preimage of that root and the burn's nullifier. It is built reproducibly in
  RISC Zero's pinned builder into `artifacts/zk/burn_membership.bin` (ImageID
  `855dfc80…`).
- `bedrock/`, `bedrock/tools/`, `zk/bedrock_anchor/`, `zk/bedrock_release/`:
  the Bedrock anchor. `lez-bedrock` (`no_std`) checks a Bedrock header's id,
  its body root over a transaction's Merkle path, the LEZ block inscribed by
  that transaction's first operation and its Ed25519 signature, and a chain
  of headers above it; `bedrock-tools` checks it against the Bedrock node's
  own types on real blocks. Run with `scripts/bedrock-devnet.sh` and
  `scripts/e2e-bedrock.sh` (evidence/local-bedrock/; docs/BEDROCK-FINALITY.md
  section 8).
- `chain/`, `zk/lez_chain/`, `zk/burn_release/`: the chain-derived release.
  `lez-chain` decodes LEZ v0.3.1 blocks strictly and recomputes their hashes
  and links. It rebuilds the tree of the bridge's private burns: those of
  privacy-preserving transactions, which on v0.3.1 cannot be included and
  fail. The chain guest proves that from a pinned anchor, or from an
  earlier receipt of itself. The release guest proves a burn under its root,
  with the chain receipt verified inside. Both are built reproducibly
  (`scripts/build-chain-guests.sh`; ImageIDs `68718ab0…` and `fd3b52ce…`).
  [docs/BEDROCK-FINALITY.md](docs/BEDROCK-FINALITY.md) covers what Bedrock
  finality would take on top.
- `core/`: `wrap-core`, every hash both halves must agree on (leaf, tree,
  commitments, nullifiers, the `knownRoot` slot, the burn tree, the release
  digest), with shared test vectors asserted on both sides; feature `zk`, the
  release guest's statement (`src/zk.rs`).
- `programs/wrap_bridge/`: the LEZ v0.3 program (plan/apply on `lee_core` at
  v0.3.0, `db66590ab`). `Claim` checks the deposit's Merkle path to `root`,
  calls Sextant's `Attest` as a chained call to prove `knownRoot[root] == 1`
  under a header the light client finalized, spends the deposit's nullifier,
  and mints through LEZ v0.3.0's token program, which it calls with the seed of
  the definition PDA it owns: the mint authority is the program itself, with no
  stored authority field (LP-0013's authorities are not in the runtime).
  Wrapped tokens are held in associated token accounts, one per owner and token,
  or, on the private path, in the private account the deposit committed to.
  `Burn` burns through the ATA program and appends a leaf hiding the Ethereum
  recipient and the relayer fee to the program's burn tree. `BurnFrom` does
  the same from the account that holds the token. On the private path that
  account is a private one, authorized by its owner's key inside a
  privacy-preserving transaction. `Register`, `SetPolicy`, `Freeze`.
- `executor-tests/`: the four committed binaries (`sextant_lc` from Sextant,
  `token` and `associated_token_account` from LEZ v0.3.0, `wrap_bridge`)
  through `lee::V03State`, the state machine the v0.3 sequencer applies, under
  the 10,000,000 gas cap, on deposits made on anvil; and the claim as a
  privacy-preserving transaction to a private account, proven by LEZ's privacy
  circuit and settled through the same checks the sequencer runs.
- `cli/`: `wrap`, the client on a LEZ v0.3 sequencer (v0.3.0 `wallet` crate,
  v0.3.1 block format): loads the four binaries through program_loader,
  initialises Sextant and the bridge, sends public claims and burns with an
  explicit gas limit after a local pre-check, proves the private claim into
  a wallet-held shielded account and the private burn out of it, and builds
  a release's opening (`release-proof`). With `release-proof --zk` (and
  `zk-prove`) it proves the release through r0vm and RISC Zero's Groth16
  prover image (`src/zk.rs`). `scripts/e2e-local.sh` drives the
  round trip against a local v0.3.1 sequencer and anvil
  ([docs/LOCAL-E2E.md](docs/LOCAL-E2E.md)). `scripts/anvil_release.py` is
  the committee and the release on anvil.
- `vectors/anvil/`: those deposits (`scripts/anvil_vectors.py`): the vault on a
  local anvil chain, five deposits by three seeds (one to a private LEZ
  account), the Deposit events, the block,
  and `eth_getProof` of `knownRoot[r]` for every root, plus one never-known root.

Anvil has no beacon chain, so the tests wrap its block in a beacon header and
initialise a Sextant instance from it as the trusted checkpoint, the one input
a light client takes on trust. Everything after that is verified as on a real
network: the storage proof against the block's state root (by Sextant), the
deposit's path, the nullifier, the mint.

## Run

```console
forge test --root vault                                            # 86 tests (47 on real Groth16 seals)
scripts/mutation-test-vault.sh         # each check of the release path removed must fail them (42)
cargo test --manifest-path chain/Cargo.toml --features release     # 10 tests: the chain guest's statement
scripts/build-chain-guests.sh          # rebuild artifacts/zk/lez_chain.bin and burn_release.bin (reproducible, Docker)
cargo test --release --manifest-path core/Cargo.toml --features zk # 9 tests
scripts/build-zk-guest.sh              # rebuild artifacts/zk/burn_membership.bin (reproducible, Docker)
scripts/zk-fixture.sh                  # re-prove the Foundry fixture (x86_64 Docker image; ~75 s emulated)
cargo test --release --manifest-path executor-tests/Cargo.toml     # 15 tests (r0vm 3.0.5)
RISC0_DEV_MODE=1 cargo test --release --manifest-path executor-tests/Cargo.toml --test private -- --ignored   # 3 tests
scripts/measure-private-claim.sh       # the private claim with real proving (~7 GB, minutes)
cargo run --release --manifest-path executor-tests/Cargo.toml --bin measure
DOCKER=1 scripts/build-guest.sh        # rebuild artifacts/programs/v0.3/wrap_bridge.bin (reproducible)
scripts/mutation-test.sh               # each guard removed in turn must fail the tests (21)
python3 scripts/anvil_vectors.py       # regenerate vectors/anvil (local anvil, nothing spent)
cargo build --release --manifest-path cli/Cargo.toml
SEQUENCER=… WALLET=… LEZ_SRC=… scripts/e2e-local.sh    # local v0.3.1 sequencer + anvil, end to end, zero-knowledge releases (docs/LOCAL-E2E.md)
RUN=<a KEEP=1 run dir> BASE=<its e2e.tsv> scripts/e2e-chain.sh   # then the chain-derived release on it (evidence/local-chain/)
# the run ends with both against its own sequencer; with KEEP=1 it stays up to re-run them:
scripts/verify-onchain.sh --manifest evidence/local/e2e.tsv --rpc http://127.0.0.1:3051 --eth-rpc http://127.0.0.1:8551
scripts/mutation-test-verifier.sh --manifest evidence/local/e2e.tsv --rpc http://127.0.0.1:3051 --eth-rpc http://127.0.0.1:8551
```

`artifacts/programs/v0.3/wrap_bridge.bin` is the `DOCKER=1` build, in RISC
Zero's pinned builder (`risczero/risc0-guest-builder:r0.1.91.1`): 446,832
bytes, sha256 `6975794938335ab3715a9acdc783bc1e6eb5203e9fe411c5e7efd9f78cca1903`,
ImageID `073f046ce2a6fffd6adec655f0e65e91fd1ab7e47774e2685c40f2d51ea1c670`
(two builds from scratch, identical).
CI rebuilds it from a clean checkout and `cmp`s it with the committed bytes
(`.github/workflows/ci.yml`, job `reproduce`). The local build (no `DOCKER`)
is for iterating and for the mutation test; its bytes differ.

## Measured (docs/raw/)

| | gas |
|---|---:|
| LEZ `Claim` (path, chained `Attest`, chained token `Mint`) | 297,457 (2.97% of the cap) |
| of which Sextant's `Attest` of the storage slot | 111,122 |
| LEZ `Burn` (burn-tree append, chained ATA `Burn`) | 228,728 |
| LEZ `Register` (entry, chained `NewFungibleDefinition`) | 90,577 |
| Ethereum `deposit` (median, max) | 257,854, 336,694 |
| Ethereum `release`, `ZkBurnVerifier` (Groth16 + 2 signatures, local run, gas used) | 334,215 (fee 0), 343,550 (paying a relayer) |
| of which RISC Zero's Groth16 `verify` (Foundry, execution) | 253,318 |
| Ethereum `release`, `CommitteeBurnVerifier`, 2 signatures (local run, gas used) | 143,096 (fee 0), 152,400 (paying a relayer) |

The zero-knowledge release proof, for each release
([docs/OUTBOUND.md](docs/OUTBOUND.md), section 3). The STARK takes 19 to 21 s
(165,109 user cycles). The Groth16 wrap takes 52 to 56 s, in RISC Zero's
x86_64 image under emulation on Apple Silicon. Peak memory is 2.5 GB on the
host and about 7.6 GiB in the Groth16 container. The seal is 260 bytes, and
the release calldata about 1,540 bytes.

The private claim, as a privacy-preserving transaction to a private account
with real proving ([docs/PRIVATE-CLAIM-MEASURED.md](docs/PRIVATE-CLAIM-MEASURED.md)):
252 to 254 s, 6.7 to 8.3 GB peak RAM, 230,510 bytes (M4 Pro, 24 GiB). Through
the client, the wallet and a local v0.3.1 sequencer
([docs/LOCAL-E2E.md](docs/LOCAL-E2E.md)): 263 s, 6.87 GB, 251,527 bytes on
chain (the wallet pads to seven private inputs and 512-byte notes), decrypted
by carol's wallet. The
destination, secret, commitment, leaf, index and path stay private; the root,
token, amount and nullifier account are public, so a claim hides among the
unclaimed deposits of its token and amount under its root.

The private burn, a `BurnFrom` out of carol's private account through the
same client and sequencer: 202.9 s, 7.22 GB, 248,358 bytes. Her account id,
her Ethereum recipient and the burn secret are absent from it, and the burn
leaf is public ([docs/OUTBOUND.md](docs/OUTBOUND.md)). The claim in the same
run took 247.6 s, 6.17 GB and 251,335 bytes. The machine was busier during
the burn.

The anvil account proof has 3 nodes; a mainnet one has 8 to 9, which Sextant
measured at 304,267 gas for a storage `Attest`: a mainnet claim is about
490,000 gas, still under 5% of the cap.

## What this prototype is not yet

A public claim carries the deposit's opening, so it links the claim to its
deposit. The private claim and the private burn run through a local v0.3.1
sequencer and the wallet ([docs/LOCAL-E2E.md](docs/LOCAL-E2E.md)), not yet on
a public network.

**The release (F.7) is not cryptographically verified.** Its root rests on a
k-of-n committee, because nothing on Ethereum can check LEZ state today: LEZ
v0.3 block headers carry no state root, and there is no Bedrock light client
for Ethereum. In the local run, "final" is the standalone sequencer's tip.
The zero-knowledge release hides which burn it pays out. Its journal (token,
amount, recipient, relayer, fee, root, nullifier) is public, so it hides only
among the burns of the same token and denomination under its root. Not built:
the Cryptarchia light client that would replace the committee for the root
([docs/OUTBOUND.md](docs/OUTBOUND.md)). The Groth16 wrap was measured
emulated, not on native x86_64. No protocol fee, no rolling-window cap on LEZ (an outstanding-
supply cap instead), no relayer, no Logos modules, no SPEL IDL (SPEL does not
build v0.3 programs), no deployment on a public network (the local run's
Sextant instance is initialised from a beacon header built around the anvil
block, a checkpoint nothing signs).

MIT OR Apache-2.0.

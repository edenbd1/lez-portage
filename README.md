# Portage

Portage is a prototype of a privacy-preserving bridge for wrapped ERC-20 tokens
and Ether between Ethereum and LEZ, the Logos Execution Zone (Logos RFP-022).
Assets are locked in an Ethereum vault and minted on LEZ by a LEZ v0.3
program; they go back by a burn on LEZ and a release on Ethereum.

- **Inbound (Ethereum to LEZ)** is verified on LEZ by Sextant, the Ethereum
  light client for LEZ (RFP-021,
  [edenbd1/lez-sextant](https://github.com/edenbd1/lez-sextant), `main` at
  `8153272`), and never by a signer or a federation.
- **Outbound (LEZ to Ethereum)** is released on a RISC Zero proof, verified on
  Ethereum as one Groth16 seal, that the burn is in a burn tree rebuilt from
  LEZ blocks and that the last of those blocks is inscribed on Bedrock by the
  sequencer, k headers deep. One fact is still attested rather than proven: a
  2-of-3 committee says that the top Bedrock header is canonical. **F.7 of
  the RFP is therefore not met**; see [What is still trusted](#what-is-still-trusted).

**The inbound path ran on public networks** on 2026-11-08
([docs/TESTNET.md](docs/TESTNET.md)). Two deposits were made into a vault on
Sepolia. A Sextant instance on the public LEZ testnet was initialised from a
Sepolia checkpoint finalized after them. `wrap_bridge` on the same testnet then
minted both deposits, one by a public claim and one by a privacy-preserving
claim into a shielded account. Each claim was attested against that header,
by a storage proof checked on chain. `scripts/verify-testnet.sh` re-checks it.
Everything else ran locally: anvil, a LEZ v0.3.1 sequencer, the v0.3.0 wallet,
and a single-node Bedrock devnet. The release path has not run on a public
network.

## How it works

**Deposit and claim.** `WrapVault.sol` escrows a registered ERC-20 or ETH in a
fixed denomination against a hiding commitment, so the LEZ destination appears
in no argument, event or storage slot (F.2). Each deposit appends a leaf to a
depth-24 SHA-256 tree, and every root it has had is recorded at
`knownRoot[root]` (storage slot 0). On LEZ, the program's `Claim` checks the
deposit's Merkle path to `root`, calls Sextant's `Attest` as a chained call to
prove `knownRoot[root] == 1` under an Ethereum header the light client
finalized, spends the deposit's nullifier, and mints through LEZ v0.3.0's
token program. The program owns the token definition as a PDA, so it is the
mint authority without any stored authority field (LP-0013's authorities are
not in the runtime). A claim can be public, or a privacy-preserving
transaction into a private account, in which the destination, secret,
commitment, leaf and path stay hidden.

**Burn and release.** `Burn` and `BurnFrom` burn the wrapped token and append a
leaf, which hides the Ethereum recipient and the relayer fee, to the program's
burn tree. `BurnFrom` from a private account is authorized inside the privacy
circuit, so the account appears nowhere. The vault releases on a verifier
fixed at construction, each nullifier once. Four verifiers were built, each
one trusting less than the one before:

| verifier | what the proof shows | what the committee attests |
|---|---|---|
| `CommitteeBurnVerifier` | nothing: the leaf is opened in public | the burn-tree root is final |
| `ZkBurnVerifier` | the burn is under a root (zero knowledge) | the burn-tree root is final |
| `ZkChainBurnVerifier` | the root is rebuilt from LEZ blocks | the last LEZ block is final |
| `ZkBedrockBurnVerifier` | that block is inscribed on Bedrock, k deep | one Bedrock header is canonical |

The zero-knowledge releases carry no leaf, secret, commitment or path. Their
journal (token, amount, recipient, relayer, fee, root, nullifier) is public, so
a release hides among the burns of the same token and denomination.

Details: [docs/OUTBOUND.md](docs/OUTBOUND.md) (burn and the four releases),
[docs/BEDROCK-FINALITY.md](docs/BEDROCK-FINALITY.md) (how LEZ sits on Bedrock,
what a light client would need, what was built),
[docs/PRIVATE-CLAIM-MEASURED.md](docs/PRIVATE-CLAIM-MEASURED.md),
[docs/LOCAL-E2E.md](docs/LOCAL-E2E.md).

## What is proven and measured

Machine: Apple M4 Pro (14 cores, 24 GiB), r0vm 3.0.5, RISC Zero dev mode off.
The Groth16 wrap ran in RISC Zero's x86_64 prover image under emulation; it
was not measured on native x86_64.

**Local end-to-end runs** (records in [`evidence/`](evidence/)):

| run | result |
|---|---|
| [`evidence/testnet/`](evidence/testnet/), `scripts/testnet-inbound.sh`, **Sepolia and the public LEZ testnet** | 2 deposits on Sepolia; Sextant `Init` from a Sepolia checkpoint finalized after them; the public claim (451,107 gas) and the private claim (proven in 264.6 s, 257,739 B) applied; 2 refusals (a replay, a root the vault never had); `scripts/verify-testnet.sh`: 29 LEZ checks and 14 Sepolia checks, 0 fail; verifier mutation test 16 of 16; 35.4M of the testnet token |
| [`evidence/local/`](evidence/local/), `scripts/e2e-local.sh` | PASS: 14 LEZ transactions (9 applied, 5 refused as expected), 11 zero-knowledge releases on anvil (2 applied, 9 refused as expected); verifier 81 checks, 0 fail; its mutation test 32 of 32 |
| [`evidence/local-committee/`](evidence/local-committee/), `RELEASE_VERIFIER=committee` | PASS, releases opening the leaf: verifier 73 checks, mutation test 28 of 28 |
| [`evidence/local-chain/`](evidence/local-chain/), `scripts/e2e-chain.sh` | PASS: 12 releases through `ZkChainBurnVerifier` (1 applied, 11 refused) |
| [`evidence/local-bedrock/`](evidence/local-bedrock/), `scripts/e2e-bedrock.sh` | PASS: 15 releases through `ZkBedrockBurnVerifier` (1 applied, 14 refused), after e2e-local (PASS) and e2e-chain (PASS) on a sequencer inscribing every block on the devnet; 2,579 Bedrock blocks checked against the node's own types |

The refusals include replays, a redirected recipient, another amount, a
front-run, a tampered journal, another anchor or channel, an anchor one header
short of `MIN_DEPTH`, outsiders and a single committee member signing, and a
real proof over a made-up Bedrock block signed by the real sequencer key,
which only the committee members' node refuses.

**Proving** (one proof each, real proving):

| proof | time | memory | size |
|---|---:|---:|---:|
| private claim, through the client and the sequencer | 263 s | 6.87 GB | 251,527 B on chain |
| private burn (`BurnFrom` out of a private account) | 202.9 s | 7.22 GB | 248,358 B on chain |
| zero-knowledge release: STARK, then Groth16 (emulated) | 19-21 s + 52-56 s | 2.5 GB host, ~7.6 GiB container | 260 B seal |
| chain proof, LEZ blocks 28-120 then 121-317 (recursive) | 87 s, 141 s | | 1.31M, 2.10M cycles |
| Bedrock anchor, 5 headers deep (devnet) | 111 s STARK | 5.9 GB | 1.14M user cycles |
| Bedrock-anchored release, three guests in one seal | 52 s STARK + 62 s Groth16 | 5.3 GB, container 7.5 GiB | 260 B seal |

A Bedrock anchor 30 headers deep (k on the public testnet), on a real testnet
block, is 1.80M user cycles in the executor (24.3k per header). The chain
proof costs about 1.5 cycles per block byte plus 4.3k per block.

**Gas:**

| | gas |
|---|---:|
| LEZ `Claim` (path, chained `Attest`, chained token `Mint`) | 297,457 (2.97% of the 10M cap) |
| of which Sextant's `Attest` of the storage slot | 111,122 |
| LEZ `Burn` (burn-tree append, chained ATA `Burn`) | 228,728 |
| LEZ `Register` (entry, chained `NewFungibleDefinition`) | 90,577 |
| Ethereum `deposit` (median, max) | 257,854, 336,694 |
| Ethereum `release`, `CommitteeBurnVerifier` | 143,096 (fee 0), 152,400 (relayer paid) |
| Ethereum `release`, `ZkBurnVerifier` | 334,215 (fee 0), 343,550 (relayer paid) |
| Ethereum `release`, `ZkChainBurnVerifier` | 347,250 |
| Ethereum `release`, `ZkBedrockBurnVerifier` | 349,997 |
| of which RISC Zero's Groth16 `verify` | 253,318 |

The anvil storage proof has 3 nodes; a mainnet one has 8 to 9, which Sextant
measured at 304,267 gas for an `Attest`, so a mainnet claim is about 490,000
gas, under 5% of the cap.

**Tests:** 105 Foundry tests (`vault/`, the 66 of the three zero-knowledge verifiers on real Groth16 seals), the
vault's mutation test 66 of 66; 15 executor tests running the committed
binaries through `lee::V03State`, plus 3 private-path tests in dev mode; the
program's mutation test 21 of 21; `wrap-core` 9, `lez-chain` 10, `lez-bedrock`
14 tests and its mutation test 30 of 30. Raw outputs: [`docs/raw/`](docs/raw/).

**Reproducible binaries.** Every committed program and guest is built in RISC
Zero's pinned builder (`risczero/risc0-guest-builder:r0.1.91.1`); CI rebuilds
each from a clean checkout and `cmp`s it with the committed bytes.

| binary | bytes | ImageID |
|---|---:|---|
| `artifacts/programs/v0.3/wrap_bridge.bin` | 446,832 | `073f046ce2a6fffd6adec655f0e65e91fd1ab7e47774e2685c40f2d51ea1c670` |
| `artifacts/zk/burn_membership.bin` | 318,720 | `855dfc80…` |
| `artifacts/zk/lez_chain.bin` | 294,832 | `68718ab0…` |
| `artifacts/zk/burn_release.bin` | 330,048 | `fd3b52ce…` |
| `artifacts/zk/bedrock_anchor.bin` | 431,932 | `cebe7a27…` |
| `artifacts/zk/bedrock_release.bin` | 335,088 | `a73cc984…` |

`sextant_lc.bin` is Sextant's; `token.bin` and `associated_token_account.bin`
are LEZ v0.3.0's.

## What is still trusted

On the release path, in the strongest verifier (`ZkBedrockBurnVerifier`):

1. **The top Bedrock header is canonical.** The 2-of-3 `BedrockHeaderCommittee`
   says so. A header chain can be made up: checking Cryptarchia's proof of
   leadership needs note-set roots that no header commits to, so a light client
   would have to replay the note ledger
   ([docs/BEDROCK-FINALITY.md](docs/BEDROCK-FINALITY.md), section 3). The
   committee's claim is checkable by anyone with a Bedrock node in one request.
2. **Bedrock's own checks.** The leader's block signature, and the pinned key
   being the channel's accredited key at that slot, are checked by Bedrock,
   not by the proof.
3. **LEZ block validity.** LEZ v0.3.1 headers carry no state root; the proof
   rebuilds the burn tree from block contents, and an invalid block can still
   be inscribed. It also does not prove the inscribed block is the only LEZ
   block on the channel at its height.
4. **Only private burns release this way.** A reverted public transaction stays
   in its block unmarked, so public burns cannot be derived from blocks;
   privacy-preserving transactions are fee-exempt on v0.3.1, so a failing one
   invalidates its block.

Forging a release for a burn that never happened needs the committee threshold
and the sequencer's key together.

Inbound, the trusted input is the checkpoint a light client starts from. Anvil
has no beacon chain, so the local runs wrap its block in a beacon header and
initialise Sextant from it; everything after that (the storage proof against
the state root, the deposit's path, the nullifier, the mint) is verified as on
a real network. On Sepolia the checkpoint is a finalized header served,
byte for byte the same, by two beacon implementations.

Not built yet: the release path on a public network, a relayer, a
protocol fee, a rolling-window cap on LEZ (an outstanding-supply cap instead),
Logos modules, and a SPEL IDL (SPEL does not build v0.3 programs). A public
claim carries the deposit's opening, so it links the claim to its deposit.

## Run

Tests and reproducible builds (as in CI):

```console
forge test --root vault                                            # 105 tests
scripts/mutation-test-vault.sh                                     # 66 mutations, each must fail them
cargo test --release --manifest-path core/Cargo.toml --features zk
cargo test --release --manifest-path chain/Cargo.toml --features release
cargo test --release --manifest-path bedrock/Cargo.toml --features release
scripts/mutation-test-bedrock.sh
cargo test --release --manifest-path executor-tests/Cargo.toml     # r0vm 3.0.5
RISC0_DEV_MODE=1 cargo test --release --manifest-path executor-tests/Cargo.toml --test private -- --ignored
scripts/mutation-test.sh                                           # wrap_bridge's guards
DOCKER=1 scripts/build-guest.sh        # artifacts/programs/v0.3/wrap_bridge.bin
scripts/build-zk-guest.sh              # artifacts/zk/burn_membership.bin
scripts/build-chain-guests.sh          # artifacts/zk/lez_chain.bin, burn_release.bin
scripts/build-bedrock-guests.sh        # artifacts/zk/bedrock_anchor.bin, bedrock_release.bin
```

The end-to-end runs need a LEZ checkout at `db66590a` (v0.3.0, for the wallet)
and `1a003bfb` (v0.3.1, for the sequencer), anvil, Docker with RISC Zero's
x86_64 Groth16 prover image (`risczero/risc0-groth16-prover:v2025-04-03.1`,
emulated on Apple Silicon by `DOCKER_DEFAULT_PLATFORM=linux/amd64`), and about
8 GB of free memory per proof. Set `HEAVY` to a wrapper command to serialize
the provers with other work on the machine.

```console
cargo build --release --manifest-path cli/Cargo.toml
# 1. claim, burn and zero-knowledge release on a standalone sequencer and anvil
SEQUENCER=<lez@1a003bfb>/target/release/sequencer_service \
WALLET=<lez@db66590a>/target/release/wallet \
LEZ_SRC=<lez@1a003bfb> scripts/e2e-local.sh
# 2. the Bedrock-anchored release: a separate single-node Bedrock devnet,
#    a sequencer built without `standalone` inscribing on it, then the chain
#    and Bedrock steps on the kept run
BEDROCK_BIN=<logos-blockchain-node 0.3.1> LEZ_SRC=<lez@1a003bfb> DIR=<new dir> scripts/bedrock-devnet.sh
cargo build --release --manifest-path bedrock/tools/Cargo.toml
KEEP=1 BEDROCK_NODE=http://127.0.0.1:18480 BEDROCK_CHANNEL=<32 bytes hex> RUN=<run dir> \
  OUT=evidence/local-bedrock/e2e.tsv SEQUENCER=… WALLET=… LEZ_SRC=… scripts/e2e-local.sh
RUN=<run dir> BASE=evidence/local-bedrock/e2e.tsv OUT=evidence/local-bedrock/e2e-chain.tsv scripts/e2e-chain.sh
RUN=<run dir> BASE=evidence/local-bedrock/e2e.tsv BEDROCK_NODE=http://127.0.0.1:18480 scripts/e2e-bedrock.sh
# re-check a kept run's manifest against the chains, and mutation-test that check
scripts/verify-onchain.sh --manifest evidence/local/e2e.tsv --rpc http://127.0.0.1:3051 --eth-rpc http://127.0.0.1:8551
scripts/mutation-test-verifier.sh --manifest evidence/local/e2e.tsv --rpc http://127.0.0.1:3051 --eth-rpc http://127.0.0.1:8551
```

[docs/LOCAL-E2E.md](docs/LOCAL-E2E.md) describes each step, its ports and its
limits; [docs/OUTBOUND.md](docs/OUTBOUND.md), section 5, the Bedrock step.

The public-network inbound run is resumable by stage. Its commands are in
[docs/TESTNET.md](docs/TESTNET.md#reproduce). To re-check the recorded run
against the public LEZ testnet and Sepolia, with JSON-RPC only:

```console
scripts/verify-testnet.sh
```

## Layout

| path | what it is |
|---|---|
| [`vault/`](vault/) | Foundry: `WrapVault.sol`, the four release verifiers and their committees, tests on real Groth16 seals (`test/fixtures/`); vendored forge-std, OpenZeppelin, risc0-ethereum v3.0.1 (`RiscZeroGroth16Verifier`, unchanged) |
| [`programs/wrap_bridge/`](programs/wrap_bridge/) | the LEZ v0.3 program (`Register`, `SetPolicy`, `Freeze`, `Claim`, `Burn`, `BurnFrom`) |
| [`core/`](core/) | `wrap-core`: every hash both chains must agree on, with shared vectors; feature `zk`, the release statement |
| [`chain/`](chain/) | `lez-chain`: strict LEZ v0.3.1 block decoding, block hashes and links, the burn tree rebuilt from blocks |
| [`bedrock/`](bedrock/) | `lez-bedrock` (`no_std`): Bedrock header id, body root, Merkle path, inscription, Ed25519 signature, header chain; [`bedrock/tools/`](bedrock/tools/) checks it against the node's own types |
| [`zk/`](zk/) | RISC Zero guests: `burn_membership`, `lez_chain`, `burn_release`, `bedrock_anchor`, `bedrock_release` |
| [`artifacts/`](artifacts/) | the committed program and guest binaries |
| [`executor-tests/`](executor-tests/) | the committed binaries through `lee::V03State` under the gas cap, public and private paths |
| [`cli/`](cli/) | `wrap`, the client on a LEZ v0.3 sequencer (v0.3.0 wallet crate, a vendored v0.3.1 `common` for the block format) and the release prover |
| [`scripts/`](scripts/) | builds, end-to-end runs, the on-chain verifier, mutation tests, anvil helpers |
| [`vectors/`](vectors/) | anvil deposits and proofs, LEZ sequencer blocks, a public-testnet Bedrock anchor |
| [`evidence/`](evidence/) | manifests, transcripts and raw measurements of each run (`testnet/`: Sepolia and the public LEZ testnet) |
| [`docs/`](docs/) | the design and measurement write-ups; [`docs/raw/`](docs/raw/) raw outputs |

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at
your option.

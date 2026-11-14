# Portage

Portage is a prototype of a privacy-preserving bridge for wrapped ERC-20 tokens
and Ether between Ethereum and LEZ, the Logos Execution Zone (Logos RFP-022).
Assets are locked in an Ethereum vault and minted on LEZ by a LEZ v0.3
program; they go back by a burn on LEZ and a release on Ethereum.

- **Inbound (Ethereum to LEZ)** is verified on LEZ by Sextant, the Ethereum
  light client for LEZ (RFP-021,
  [edenbd1/lez-sextant](https://github.com/edenbd1/lez-sextant)), and never
  by a signer or a federation. Two Sextant builds are involved: the local runs
  and executor tests use the committed `sextant_lc.bin`, Sextant at
  `8153272` (ImageID `fa229edd…`); the public run used Sextant v2, program
  `Ebe8rifvjyvwWUXnk7bsKeTwuyCAFSRMFuPMVHE8CbUB` (ImageID `ad6fbedb…`), the
  binary of lez-sextant `main` at `0338eb3`.
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

**Branch `v0.4-spike`** (this branch; not merged into `main`) ports the LEZ
side to LEZ v0.4.0-rc3: the actor model, the canonical token and associated
token account programs' actor-model ports (one account holds several wrapped
tokens), Sextant's light client on v0.4, blocks that list reverted
transactions (so public burns are derivable from blocks), ETH and an ERC-20
claimed and burned, a v0.4 client (`wrap`, pre-checking every transaction on
rc3's state machine), reproducible Docker builds of every v0.4 binary, and a
run on a local v0.4.0-rc3 sequencer whose applied burns are then released from
WrapVault on anvil through the committee verifier, including a private claim
and a private burn proven for real, their fee paid inside the proof by a
funded private account. Nothing of v0.4 is on a public network
(there is no public 0.4 network yet). See [docs/V04-PORT.md](docs/V04-PORT.md)
and [`v04/`](v04/); how to run it is under "Run" below.

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

**Caps, pause and roles.** Each deposit and release passes two rate limits
over a rolling window (`WINDOW`, 1 hour to 30 days, fixed at deployment; 1 day
in the scripts): one per token and direction, in token units, and one global
per direction, in a value unit the admin sets per token (`setTokenValue`). The
global caps are off until `setGlobalCaps` turns them on; a token without a
value is then refused, so leaving one unpriced is no way around them. Each
limit is a bucket that fills with every transfer and drains linearly, one full
cap per window: no burst exceeds the cap, and any interval of length T moves
at most cap × (1 + T / `WINDOW`). The fixed window it replaces let two caps
through in one second across its reset
(`test_the_boundary_burst_of_a_fixed_window_is_closed`; the bound is fuzzed in
`testFuzz_rolling_bound_holds_over_every_interval`).

The freezer (guardian) pauses deposits and/or releases, of one token or all,
at once and with no delay; it can do nothing else, not even unpause. The
admin registers a new token and tightens at once (a lower cap, a higher
value, a delisting); everything that loosens (a higher cap, a lower value, a
relisting, a new denomination, a shorter delay) and everything that touches a
pause or a role runs only as an operation scheduled with its exact calldata,
runnable from `delay` later (2 days by default, bounded to 1 to 30 days) and
for 14 days after that. Rotation rules:

- an unpause is always timelocked and names the pause count it lifts, so an
  unpause scheduled before the freezer's latest pause cannot lift it: the
  admin cannot bypass a pause, even with an operation kept ready; re-pausing
  what is already paused does not count, so the freezer cannot void unpauses
  without end;
- the freezer is replaced only by the admin, through the timelock; the
  current freezer keeps pausing until the replacement runs, and cannot block it;
- the admin is replaced in two steps, a timelocked proposal and the new
  admin's acceptance; the acceptance voids every operation the old admin had
  pending;
- no rotation can make the admin and the freezer one account (the constructor
  allows it only for test rigs, where the separation is then void).

Tests: [`vault/test/WrapVaultGuard.t.sol`](vault/test/WrapVaultGuard.t.sol),
each role acting adversarially. The `WrapVault` deployed on Sepolia
([docs/TESTNET.md](docs/TESTNET.md)) is the previous version, with a fixed
window, per-token caps only and a freezer the admin could override at once;
these caps and roles land with the next deployment.

Details: [docs/OUTBOUND.md](docs/OUTBOUND.md) (burn and the four releases),
[docs/BEDROCK-FINALITY.md](docs/BEDROCK-FINALITY.md) (how LEZ sits on Bedrock,
what a light client would need, what was built),
[docs/PRIVATE-CLAIM-MEASURED.md](docs/PRIVATE-CLAIM-MEASURED.md),
[docs/LOCAL-E2E.md](docs/LOCAL-E2E.md),
[docs/REQUIREMENTS.md](docs/REQUIREMENTS.md) (each RFP requirement: met, partial or planned).

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

**Proving** (real proving; the private rows are ranges over the committed
local runs, `evidence/local*/`, and three re-runs from fresh clones on
2026-11-14; the release row is an idle machine):

| proof | time | memory | size |
|---|---:|---:|---:|
| private claim, through the client and the sequencer | 230-362 s | 4.6-8.1 GB | 251,335-251,777 B on chain |
| private burn (`BurnFrom` out of a private account) | 178-276 s | 5.6-8.7 GB | 248,358 B on chain |
| zero-knowledge release: STARK, then Groth16 (emulated) | 19-21 s + 52-56 s | 2.5 GB host, ~7.6 GiB container | 260 B seal |
| chain proof, LEZ blocks 28-120 then 121-317 (recursive) | 87 s, 141 s | | 1.31M, 2.10M cycles |
| Bedrock anchor, 5 headers deep (devnet) | 111 s STARK | 5.9 GB | 1.14M user cycles |
| Bedrock-anchored release, three guests in one seal | 52 s STARK + 62 s Groth16 | 5.3 GB, container 7.5 GiB | 260 B seal |

Private-proof times above about 300 s were measured at a load average of 30
to 56 on a shared machine, where the release row's proof took 87-94 s end to
end. The peak memory of a private proof varies by up to 3.5 GB from run to
run with segment scheduling, and its size by a few hundred bytes.

A Bedrock anchor 30 headers deep (k on the public testnet), on a real testnet
block, is 1.80M user cycles in the executor (24.3k per header). The chain
proof costs about 1.5 cycles per block byte plus 4.3k per block, and it covers
every LEZ block from the anchor to the tip: the sequencer makes one every 5 s,
so a kept run that sits for an hour before `e2e-chain.sh` proves some 700 more
blocks (on 2026-11-14, 865 blocks, 4.77M user cycles, 587 s, 8.41 GB, after a
55-minute wait for the machine's prover lock).

**Gas** (LEZ figures from `executor-tests --bin measure`, exact; Ethereum
`release` figures from the local runs, which move by up to about 100 gas with
the anvil deployment addresses, on the vault before the rolling-window limits;
the `deposit` row and the two rate-limit rows from Foundry, the latter on cold
storage, [`docs/raw/vault-gas-2026-11-14.txt`](docs/raw/vault-gas-2026-11-14.txt)):

| | gas |
|---|---:|
| LEZ `Claim` (path, chained `Attest`, chained token `Mint`) | 297,457 (2.97% of the 10M cap) |
| of which Sextant's `Attest` of the storage slot | 111,122 |
| LEZ `Burn` (burn-tree append, chained ATA `Burn`) | 228,728 |
| LEZ `Register` (entry, chained `NewFungibleDefinition`) | 90,958 |
| Ethereum `deposit` (median, max) | 256,230, 359,982 |
| rate limits on a deposit, a release: per token only, change from the fixed window | −1,576, +645 (a token's first release: about +20,000, once) |
| global caps on: added to a deposit, a release | 3,794, 4,037 |
| Ethereum `release`, `CommitteeBurnVerifier` | 143,096 (fee 0), 152,400 (relayer paid) |
| Ethereum `release`, `ZkBurnVerifier` | 334,215 (fee 0), 343,550 (relayer paid) |
| Ethereum `release`, `ZkChainBurnVerifier` | 347,250 |
| Ethereum `release`, `ZkBedrockBurnVerifier` | 349,997 |
| of which RISC Zero's Groth16 `verify` | 253,318 |

The anvil storage proof has 3 nodes; a mainnet one has 8 to 9, which Sextant
measured at 304,478 gas for an `Attest` (v2 binary, ImageID `ad6fbedb…`;
304,267 with the earlier `fa229edd…`), so a mainnet claim is about 490,000
gas, under 5% of the cap.

**Tests:** 131 Foundry tests (`vault/`, the 66 of the three zero-knowledge verifiers on real Groth16 seals, 26 of
the rate limits, timelock and freezer), the vault's mutation test 106 of 106; 15 executor tests running the committed
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

`sextant_lc.bin` is Sextant's earlier build, lez-sextant at `8153272`
(642,168 bytes, sha256 `d17de1ea1457c136…`, ImageID
`fa229eddbae15b2a20126e37e03fa82a2b42c7c98f74001a7d49e93fafe205c8`), which
the local runs and executor tests load. It is **not** the program the public
run used: that is Sextant v2, `Ebe8rifvjyvwWUXnk7bsKeTwuyCAFSRMFuPMVHE8CbUB`
(651,932 bytes, sha256 `58a600a4c9e1d608…`, ImageID
`ad6fbedb983da6b6d75b287da354ce3041905d152d4e576ec318871eddfaa928`),
`artifacts/programs/v0.3/sextant_lc.bin` of lez-sextant `main` (`0338eb3`),
which `scripts/testnet-inbound.sh` takes from a lez-sextant checkout. Hash
that file, not this one, against the testnet program. `token.bin` and
`associated_token_account.bin` are LEZ v0.3.0's.

## What is still trusted

On the release path, in the strongest verifier (`ZkBedrockBurnVerifier`):

1. **The top Bedrock header is canonical.** The 2-of-3 `BedrockHeaderCommittee`
   says so. A header chain can be made up: checking Cryptarchia's proof of
   leadership needs note-set roots that no header commits to, so a light client
   would have to replay the note ledger
   ([docs/BEDROCK-FINALITY.md](docs/BEDROCK-FINALITY.md), section 3). Measured
   in RISC Zero, that light client costs about 240-330 billion cycles per day
   of testnet: 34.8M cycles per leadership proof and 4.6-5.0M per note update
   ([docs/POL-MEASURED.md](docs/POL-MEASURED.md)). The
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
8 GB of free memory per proof. To serialize the provers with other work on
the machine, set `HEAVY` in the environment to a lock-style wrapper command;
the scripts wrap each prover in it themselves, so do not also prefix the
script with it (a non-re-entrant lock then waits on itself).

Each run writes its manifest and raw measurements into its run directory
(`$RUN/evidence/`, `RUN` a new temporary directory by default) and leaves the
committed records alone; `OUT=evidence/<dir>/<file>.tsv` rewrites one of them.

```console
# LEZ at db66590a (v0.3.0): the wallet; at 1a003bfb (v0.3.1): two sequencers,
# in two target directories (one with `standalone`, one without)
cargo build --release -p wallet                                    # at db66590a
cargo build --release --features standalone -p sequencer_service   # at 1a003bfb, step 1
cargo build --release -p sequencer_service                         # at 1a003bfb, another target dir, step 2
cargo build --release --manifest-path cli/Cargo.toml
# 1. claim, burn and zero-knowledge release on a standalone sequencer and anvil
SEQUENCER=<lez@1a003bfb, standalone>/target/release/sequencer_service \
WALLET=<lez@db66590a>/target/release/wallet \
LEZ_SRC=<lez@1a003bfb> scripts/e2e-local.sh
# 2. the Bedrock-anchored release: a separate single-node Bedrock devnet,
#    a sequencer built without `standalone` inscribing on it, then the chain
#    and Bedrock steps on the kept run. bedrock-devnet.sh returns after three
#    epoch transitions (about 5 minutes): a channel opened in the devnet's
#    first epoch loses every inscription at its first transition. e2e-local.sh
#    then fails early if the sequencer's latest block is not inscribed.
BEDROCK_BIN=<logos-blockchain-node 0.3.1> LEZ_SRC=<lez@1a003bfb> DIR=<new dir> scripts/bedrock-devnet.sh
cargo build --release --manifest-path bedrock/tools/Cargo.toml
KEEP=1 BEDROCK_NODE=http://127.0.0.1:18480 BEDROCK_CHANNEL=<64 hex characters> RUN=<run dir> \
  SEQUENCER=<lez@1a003bfb, without standalone>/target/release/sequencer_service WALLET=… LEZ_SRC=… scripts/e2e-local.sh
RUN=<run dir> BASE=<run dir>/evidence/e2e.tsv scripts/e2e-chain.sh
RUN=<run dir> BASE=<run dir>/evidence/e2e.tsv BEDROCK_NODE=http://127.0.0.1:18480 scripts/e2e-bedrock.sh
kill "$(cat <devnet dir>/node.pid)"   # and the kept sequencer and anvil, whose pids e2e-local.sh printed
# re-check a kept run's manifest against the chains, and mutation-test that check
scripts/verify-onchain.sh --manifest <run dir>/evidence/e2e.tsv --rpc http://127.0.0.1:3051 --eth-rpc http://127.0.0.1:8551
scripts/mutation-test-verifier.sh --manifest <run dir>/evidence/e2e.tsv --rpc http://127.0.0.1:3051 --eth-rpc http://127.0.0.1:8551
```

`BEDROCK_BIN` is `logos-blockchain-node` of logos-blockchain at tag `0.3.1`
(`a023e89`; `cargo build --release -p logos-blockchain-node`, or that tag's
release binary). `BEDROCK_CHANNEL` is the channel id as 64 hex characters
without `0x`; it is optional (default: the debug configuration's
`0101…01`), and the committed run used `0202…02`.

[docs/LOCAL-E2E.md](docs/LOCAL-E2E.md) describes each step, its ports and its
limits; [docs/OUTBOUND.md](docs/OUTBOUND.md), section 5, the Bedrock step.

The public-network inbound run is resumable by stage. Its commands are in
[docs/TESTNET.md](docs/TESTNET.md#reproduce). To re-check the recorded run
against the public LEZ testnet and Sepolia, with JSON-RPC only, and to
mutation-test the LEZ half of that check:

```console
scripts/verify-testnet.sh
scripts/mutation-test-verifier.sh --manifest evidence/testnet/run.tsv --rpc https://testnet.lez.logos.co   # 16 of 16
```

### The LEZ v0.4.0-rc3 port (this branch)

Tests (as CI's `v04` job) and the reproducible builds (CI's `reproduce-v04`
job rebuilds each in Docker and `cmp`s it against the committed binary):

```console
cargo test --release --manifest-path chain/v04/Cargo.toml
cargo test --release --manifest-path v04/executor-tests/Cargo.toml   # r0vm 3.0.5; bridge, erc20, private, sepolia, chain_v04
cargo run --release --manifest-path v04/executor-tests/Cargo.toml --bin measure_chain
DOCKER=1 v04/scripts/build-guests.sh token ata wrap_bridge lez_chain_v04 sextant   # v04/artifacts/*.bin
```

The end-to-end run on a local v0.4.0-rc3 standalone sequencer and anvil,
driven by the v0.4 client `wrap` (`v04/executor-tests/src/bin/wrap.rs`):

```console
cd <logos-execution-zone@v0.4.0-rc3> && cargo build --release --features standalone -p sequencer_service
SEQ=<lez@v0.4.0-rc3>/target/release/sequencer_service LEZ=<lez@v0.4.0-rc3> v04/scripts/e2e-seq.sh <run dir>
python3 scripts/v04-run-json.py <run dir> vectors/lez-blocks-v04            # the block vectors chain_v04 reads
```

It ends on anvil: each applied v0.4 burn released from WrapVault through
`CommitteeBurnVerifier` (each member reads the burn tree's root from the rc3
sequencer), with seven refusals ([evidence/v04-rc3/e2e-seq/releases.tsv](evidence/v04-rc3/e2e-seq/releases.tsv),
docs/V04-PORT.md section 0.2). Between them, `wrap claim 4 --private` and
`wrap burn carol eth 100000000000000000 --private` prove carol's private
claim and private burn (real proving unless `RISC0_DEV_MODE=1`; times, memory
and sizes in evidence/v04-rc3/e2e-seq/private-claim.json and
private-burn.json, docs/V04-PORT.md section 0.4).

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
| [`v04/`](v04/) | the LEZ v0.4.0-rc3 port: `wrap_bridge` on the actor model, the reproducible binaries of it, the token and ATA programs and Sextant's light client, executor tests (ETH and ERC-20), the v0.4 client `wrap`, the sequencer run ([docs/V04-PORT.md](docs/V04-PORT.md)); `chain/v04` and `zk/lez_chain_v04` decode v0.4 blocks |

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at
your option.

# Replacing the release committee: what Ethereum would have to verify about LEZ

The outbound release proves in zero knowledge that a burn is in the bridge's
burn tree (docs/OUTBOUND.md, section 3). The root of that tree came from a
2-of-3 committee. This document asks what would have to replace the
committee: a proof that the root belongs to final LEZ state, which means a
proof about Bedrock, LEZ's base layer and its consensus (Cryptarchia). It
then ranks the options and states what was built (section 6).

Everything below was read from source, at the revisions LEZ v0.3.1 builds
against:

- LEZ (logos-execution-zone) at tag v0.3.1, `1a003bfb`, and `main` at
  `f6afdd2` (2026-10-17).
- The Bedrock node (logos-blockchain) at `3926a532`, the revision pinned by
  LEZ v0.3.1's `Cargo.toml`, and `main` at `670f956` (2026-10-18).
- The leadership circuits (logos-blockchain-circuits) at v0.5.7, pinned in
  the node's `Cargo.lock`.

Paths are relative to each repository.

## 1. How LEZ is anchored on Bedrock

**LEZ v0.3.1 block.** The block is `Block { header, body, bedrock_status }`
(`lez/common/src/block.rs`).

- The header holds `block_id`, `prev_block_hash`, `hash` and `timestamp`.
- The hash is `sha256("/LEE/v0.3/Message/Block/" ‖ 8 zero bytes ‖
  borsh(block_id, prev_block_hash, timestamp, transactions))`, computed by
  `HashableBlockData::compute_hash`. It commits to every transaction byte,
  proofs included.
- The header carries **no state root**. Since v0.3.1 it also carries no
  producer signature.
- `bedrock_status` is the sequencer's local view (Pending, Safe or
  Finalized). It is not hashed.
- `lez/common/src/block.rs` is byte-identical on `main`, and no tree has a
  `state_root` anywhere. The only Merkle structure in LEE state is the
  private-note commitment set (`lee/state_machine/src/state/mod.rs`). Its
  root is in no header.
- No issue, PR or milestone in logos-execution-zone, logos-blockchain,
  research, logos-blockchain-pm or logos-blockchain-specs plans a state
  commitment, a zone validity proof or a zone light client. None of them
  mentions testnet 0.4 either (GitHub search was rate-limited partway, so this
  is a lower bound); Bedrock's latest release checklist is 0.3.2
  (logos-blockchain#3804). So the late-November date is not public.

**Publication.** The sequencer publishes each block whole, as a Bedrock
channel inscription:

- It serialises the block with `borsh::to_vec(&block)` and puts that in a
  Mantle `ChannelInscribe` operation
  (`lez/sequencer/actors/bedrock/src/actor.rs`, `PublishBlock`).
- The operation is
  `InscriptionOp { channel_id, inscription, parent: MsgId, signer: Ed25519 }`
  (`core/src/mantle/ops/channel/inscribe.rs` in the node).
- An inscription is at most 1,835,008 bytes (`MAX_BYTES`, 7/8 of a 2 MiB
  block). LEZ caps its blocks at 1 MiB by default, so a 250 kB
  privacy-preserving transaction fits.

Bedrock checks three things about an inscription:

- The signer's Ed25519 signature over the Mantle transaction hash, in
  `preverify`.
- That `parent` is the channel's current tip message, which makes the
  channel a single linear chain of messages.
- That the signer is the accredited key whose round-robin turn it is,
  `accredited_keys[round_robin(block_slot)]` (`core/src/mantle/channel.rs`).

The channel's keys and thresholds come from LEZ's `sequencer_stake` program.

**LEZ's own checks are structural.** `chain_state::apply::validate_against_tip`
recomputes the hash and checks the id and the link. Who produced the block
"is Bedrock's check" (`lez/cross_zone/src/acceptance.rs`).

A LEZ node then re-executes the block (`apply_block_to_state`):

- A block that does not apply is parked.
- The indexer then stalls with "L1 proceeds regardless"
  (`lez/indexer/core/src/lib.rs`). So an invalid block can be inscribed
  and become Bedrock-final. LEZ nodes stop at it; Bedrock does not.

**What LEZ "Finalized" means.** It means the channel message's Bedrock block
is at or below Bedrock's last irreversible block (LIB):

- `ZoneIndexer::next_messages` streams channel messages only up to
  `consensus_info().lib_slot` (`lez/chain_state/src/zone_indexer.rs`).
- The indexer marks every block it accepts as Finalized.
- zone-sdk takes the LIB from the node's block events and verifies nothing
  itself (`zone-sdk/src/sequencer/block_fetch/mod.rs`).

## 2. What Bedrock finality is

**Fork choice.** It is Ouroboros-Genesis style
(`consensus/cryptarchia-engine/src/lib.rs`):

- While bootstrapping, `maxvalid_bg`: within `k` blocks the longest chain
  wins; beyond `k`, the denser chain in `[lca.slot, lca.slot + s_gen)` wins.
- Online, `maxvalid_mc` refuses forks deeper than `k`.
- The online **LIB is the k-th ancestor of the local tip**, counted in
  blocks, not slots.

The deployment compiled into the node
(`nodes/node/binary/src/config/deployment/settings.yaml`):

| parameter | value |
|---|---|
| `security_param` k | 30 |
| active slot coefficient f | 1/20 |
| slot duration | 1 s |
| epoch phases | 3, 3, 4 (epoch = 6,000 slots) |
| s_gen = k / (4f) | 150 slots |

So a block is final 30 blocks deep, about 600 s on average (one block per
20 slots). Whether the public testnet overrides these values was not checked.
Bedrock#3682 (open) proposes a time-based LIB.

**Header.** It is `{slot, parent_block, body_root, proof_of_leadership}`
(`core/src/header/mod.rs`).

- `HeaderId = blake2b256("BLOCK_ID_V1" ‖ parent ‖ slot_le ‖ body_root ‖
  voucher_cm ‖ entropy ‖ proof ‖ leader_key)`.
- `body_root = blake2b256("BODY_ROOT_V1" ‖ uncle_headers ‖ tx_root)`, where
  `tx_root` is a binary Blake2b Merkle tree of Mantle transaction hashes (at
  most 1,024, so a path is at most 10 nodes).
- A transaction hash is `blake2b256("MANTLE_TXHASH_V1" ‖ ops)`. It covers
  the operations, the inscription included, but not their proofs or
  signatures.
- The block also carries an Ed25519 signature over the header, by the
  one-time `leader_key` the leadership proof commits to.

**Proof of leadership.** It is Groth16 over BN254, with a static
verification key (`zk/proofs/pol`). It has nine public inputs
(`zk/proofs/pol/src/inputs.rs`; circuit `mantle/pol.circom`): entropy,
slot, epoch nonce, two lottery constants, `aged_root`, `latest_root` and
the leader key (in two limbs). It proves three things:

- A note in the stake snapshot (`aged_root`) and in the parent block's note
  set (`latest_root`), Poseidon2 Merkle paths of depth 32.
- That its lottery ticket `Poseidon2(LEAD_V1, nonce, slot, note_id, sk)`
  wins against its value.
- The block's entropy contribution.

The node checks it against the ledger (`ledger/src/cryptarchia/mod.rs`,
`LeaderPublic::new(aged_utxos().root(), latest_utxos().root(), nonce, slot,
lottery_0, lottery_1)`).

## 3. What a light client would have to verify, and why headers are not enough

| what | from headers alone? |
|---|---|
| header chain (HeaderId, parent links) | yes, Blake2b |
| block signature | yes, Ed25519 against the header's leader key |
| epoch nonce, lottery constants | yes: the nonce chains Poseidon2 over each header's entropy; total stake is inferred from block density from genesis |
| **`aged_root`, `latest_root`** | **no**: roots of the note set (every unspent note), which transfers and per-block reward notes change; no header commits to them |
| channel state (accredited keys, tip message) | no: ledger state, in no header |
| fork choice / finality | needs the competing chains, or a density argument (below) |

The node says so itself: `verify_header_alone` "does not check
`proof_of_leadership` and the parent header since they require a ledger
state" (`core/src/block/mod.rs`).

No light-client or checkpoint path exists:

- The chain service starts from genesis only, with "TODO: add Checkpoint
  when implementing fast bootstrapping"
  (`services/chain/chain-service/src/lib.rs`).
- Sync ships whole blocks.
- The specs model checkpoint sync as a trusted `LedgerState`
  (`cryptarchia/sync.py`).
- The only light-node issue is dormant (logos-blockchain#763, 2024).
- Nothing on node `main` changes this.

So a sound Cryptarchia light client must **replay the note ledger**, from
genesis or from a trusted ledger checkpoint, to know the two roots each
leadership proof is checked against. Accepting the roots as hints without
that replay is forgeable: whoever picks the roots picks a note set in which
their own note wins the lottery.

A proof checked on Ethereum also sees only one chain. "30 blocks deep" alone
does not exclude a private fork that a minority stakeholder grows slowly.
The proof must also show density: enough blocks within a slot window that
ends near Ethereum's current time. Only the honest majority's chain reaches
that density. The guest can commit the tip's slot, and the vault can compare
it with `block.timestamp`.

## 4. The primitives and their cost in RISC Zero (r0vm 3.0.5)

| per Bedrock block | primitive | in the zkVM |
|---|---|---|
| HeaderId, body_root, Merkle path, tx hash | Blake2b-256 | no precompile; about 5-10k cycles per compression on the 32-bit machine (estimate) |
| block signature, inscription signature | Ed25519 strict verify (SHA-512 inside) | an accelerated curve25519-dalek exists; cost not measured here |
| epoch nonce, note ids, note-tree updates | Poseidon2 over BN254 Fr, width 3 | software, or risc0-bigint2 modular multiplication; not measured |
| proof of leadership | Groth16 BN254: ~9 G1 scalar multiplications, 3-4 Miller loops, one final exponentiation, a G2 decompression | RISC Zero ships BN254 acceleration (`substrate-bn` fork); no published cycle count found; by analogy with the BLS12-381 pairing check measured for Sextant (7.85M cycles with accelerated blst), an estimate of 5-15M cycles |

These figures are estimates, not measurements, except what this prototype
measured on LEZ blocks (section 7): about 1.5 cycles per block byte plus
about 4,300 cycles per block for the chain guest, and about 15,000 proven
cycles per second on this machine (an M4 Pro CPU shared with other work,
load average 6 to 22; RISC Zero proves faster on an idle machine or a GPU).

What each option implies:

- **Light client.** Per Bedrock block: about 0.2M cycles of Blake2b, the
  signature checks, the nonce, the leadership proof (5-15M), and the
  note-tree updates of its transactions (32 Poseidon2 hashes per note
  inserted or spent). Bedrock makes one block per 20 s, 4,320 a day, so a
  continuous light client is roughly 25-75 billion cycles a day before the
  note updates. At the rate measured here that is 20 to 60 days of this
  machine per day of Bedrock. It needs GPU proving or a proving market,
  continuously.
- **Proving one inscription final, with a trusted ledger checkpoint.** At
  least 31 headers (the block and 30 descendants), each with its leadership
  proof and the note-set updates between them: about 0.2-0.6 billion cycles,
  plus the checkpoint's trust.
- **The LEZ chain, as built (section 6).** A LEZ block of a few kilobytes
  costs under 10,000 cycles, and a 250 kB privacy-preserving transaction
  about 0.4M cycles. A chain of one block every 5 s, mostly small blocks, is
  on the order of 0.1-0.2 billion cycles a day. At the rate measured here
  that is a few hours of this machine per day, proven incrementally, by
  anyone, each step verifying the previous one.

## 5. The options, ranked

The question is what a release can rest on that is not "a committee says the
root is right".

**(b) Prove LEZ block inclusion and rebuild the burn tree from LEZ blocks.**
This is what section 6 builds, minus the inclusion in Bedrock.

- No state root is needed. The guest rebuilds the burn tree from the
  blocks' transactions, so the root is a function of the blocks.
- What LEZ v0.3.1 allows here is narrower than "burn transactions":
  - A **privacy-preserving** transaction is fee-exempt
    (`chain_state::classify`: "only user-submitted public transactions are
    charged"). An exempt transaction that fails settlement makes the whole
    block invalid (`settle_transaction`). So in a valid block, every
    privacy-preserving transaction's deferred effects were applied, and an
    `Append(leaf)` of the bridge program on its burn-tree account is in the
    tree.
  - A **public** transaction is charged. If its action fails, it "reverts,
    keeping the fee" and stays in its block (`settle_charged_transaction`).
    The block does not say which transactions reverted. So a public `Burn`
    in a block may never have burned.
  - The rebuilt tree therefore holds private burns only. Public burns
    release through another path, or are made as privacy-preserving
    transactions with public accounts.
  - If LEZ starts charging private transactions (the classifier calls the
    exemption "interim, pending the private-payer decision"), the same
    question applies to them.
- What remains unproven:
  1. **The tip's finality.** The committee now signs a LEZ block hash
     instead of a root.
  2. **The blocks' validity.** Their private transactions' proofs,
     nullifiers and commitment roots, and their public effects. LEZ's
     sequencer checks them and LEZ nodes re-execute them, but an invalid
     block can still be inscribed and finalized (section 1).
- Adding the Bedrock inclusion is a bounded step:
  - The inscription's bytes are the LEZ block.
  - The Mantle transaction hash covers the operation.
  - A Blake2b path of at most 10 nodes leads to `tx_root`, then
    `body_root`, then the HeaderId.
  - That is about 1-3M cycles on top. The committee would then attest a
    Bedrock HeaderId final rather than a LEZ hash, which any Bedrock node
    can check.
  - Checking the inscription's signer against the channel's accredited keys
    needs channel state, which only the light client of (a) has.

**(c) Optimistic, over (b).** The committee's attestation of the tip becomes
a claim with a bond and a challenge window. Two attestations of different
hashes at one LEZ height are a self-contained fraud proof: an equivocation
that the vault can check and slash. A claim that names a non-final block,
consistently, can only be disproven by (a), or by an arbiter. So (c) cuts the
committee's power to "stall, or be slashed for equivocating", but not to
"lie consistently". Releases wait for the window (hours) on top of Bedrock's
~10 min.

**(a) A full Cryptarchia light client in the zkVM.** It is the only option
that removes the committee for finality. It must replay the note ledger
from genesis or a checkpoint, verify each block's leadership proof against
the replayed roots, and prove density near Ethereum's time (section 3). It
is a recursive proof that someone keeps running. Cost (section 4) puts it
beyond an L budget on CPU proving. It also depends on Bedrock's ledger
format, which changes between testnets. Even then, (b)'s second gap
remains: Bedrock finality says the block was inscribed, not that LEZ would
accept it.

**(d) The committee attests the root.** This was the design up to now, and
(b) supersedes it. With (b) the committee can no longer name a root; it can
only vouch for a block hash.

Ranking for this RFP:

1. (b) as built, the committee over block hashes.
2. Bedrock inclusion added to (b).
3. Equivocation slashing and a challenge window, as (c).
4. (a), the end state, if Logos ships a checkpoint format or a ledger
   commitment that makes the note roots provable.

Each step is a new verifier behind `IBurnVerifier`, so it needs a new vault.

## 6. What was built: the chain-derived release (option b, minus Bedrock)

**`chain/` (crate `lez-chain`).** It holds the guest's statement as plain
functions:

- The v0.3.1 block layout, mirrored in plain types (`block.rs`).
- An allocation-free strict scan (`scan.rs`) that refuses what borsh would
  refuse and skips byte strings without copying them.
- `content_hash`, which recomputes the LEZ hash from the block's own bytes.
- `apply_block`: decode strictly, check the hash, check the next id and
  parent, append the private burn leaves (privacy-preserving transactions
  only; the bridge program's `Append` effect on its burn-tree account).
- The journal, 35 words: its own image id, the program, the burn-tree
  account, the anchor, the tip, and the tree's frontier.

Tests: 10 unit tests. They cover tampering anywhere in a block, trailing
and short bytes, skipped, replayed and forked blocks, effects of another
program, shard, account or variant, the recursion's prior of another guest,
deployment or anchor, and a differential test. The differential test makes
every truncation and every single-byte change of a block to six values, and
checks that the scan and the typed decoding agree on each.
`wrap chain-layout` decodes real sequencer blocks with LEZ's own `Block`
and with the mirror, and checks that both re-encode byte for byte, hash
alike, and find the same leaves.

**`zk/lez_chain` (the chain guest).** It reads a head, then the blocks one
at a time into one buffer, and applies them from a pinned anchor block. It
can also start from an earlier receipt of itself: the journal names the
guest's own image id, and the guest verifies the prior with `env::verify`.
So the chain is proven incrementally, by anyone.

**`zk/burn_release` (the release guest).** It verifies a chain receipt
inside, under the image id the chain journal names, and proves the burn's
opening under that journal's root, as `zk/burn_membership` does. It commits
the vault's 352-byte release preimage, then the chain image id, program,
burn-tree account, anchor and tip. The chain receipt is resolved as an
assumption before the Groth16 wrap, so Ethereum checks one seal for both
guests.

**`vault/src/ZkChainBurnVerifier.sol`.** It checks five things:

1. `sha256(journal[0:352])` is the vault's digest.
2. The tail names the pinned chain guest, program, burn-tree account and
   anchor.
3. `RiscZeroGroth16Verifier.verify(seal, IMAGE_ID, sha256(journal))`.
4. The tip block hash is attested final by `LezBlockCommittee` (2 of 3,
   `FINALITY_DEPTH` LEZ blocks).
5. Through the vault, the nullifier is unspent.

The burn-tree account depends on the LEZ config, the config names the vault,
and the verifier names the burn-tree account. So a fresh deployment predicts
the vault's address (CREATE nonce or CREATE2) before deploying the verifier.

**Measured** on the local run (evidence/local-chain/, section 7).

**What it does not prove, stated plainly.**

- The tip's finality: a committee signs the tip's LEZ block hash at a depth.
- The blocks' validity: LEZ's sequencer and nodes check it, not the proof.
- A threshold of the committee signing a hash off the final chain, or a
  sequencer inscribing a block LEZ would refuse, can still release against
  burns that never happened. The run shows this: a real proof over a
  made-up block is refused only by the attestation.
- F.7 is still not met as the RFP words it. What changed is what the
  committee can lie about. It can no longer name a root or a tree; it can
  only call a block final. That is a public fact anyone with a Bedrock node
  can check, and a check (b-with-inclusion) or a light client (a) can
  replace it without touching the rest.

## 7. Numbers

The local run of 2026-10-19 (`evidence/local-chain/`):

- `scripts/e2e-local.sh` with KEEP=1, run from `e769d0d` with this work
  still uncommitted (committed next as `66b489a`; the client and
  scripts gained the chain commands, their zk paths are unchanged). It passed: 14 LEZ transactions, 11 releases through
  `ZkBurnVerifier`, verifier 81/0, its mutation test 32/32.
- Then `scripts/e2e-chain.sh` on the same sequencer and anvil, at commit
  `49a5b49`. It passed: 12 chain-derived releases (1 applied, 11 refused
  as expected) and 23 checks.

Machine: Apple M4 Pro (14 cores, 24 GiB). r0vm 3.0.5, RISC Zero dev mode
off. Each prover ran under `$HEAVY` and `/usr/bin/time`,
at load averages of 6 to 26 from other work.

| | chain, step 1 | chain, step 2 (prior verified inside) | release (carol) | release (made-up block) |
|---|---:|---:|---:|---:|
| LEZ blocks | 28-120 (93) | 121-317 (197) | | 1 (made up) |
| block bytes | 343,403 | 389,016 | | 251 |
| user cycles | 931,315 | 1,449,583 | 307,259 | 307,259 |
| total cycles, segments | 1,310,720, 2 | 2,097,152, 2 | 524,288 | 524,288 |
| STARK (succinct) | 86.9 s | 141.1 s | 39.0 s | 38.3 s |
| Groth16 (x86_64 image, emulated) | | | 48.0 s | 48.0 s |
| wall | 86.9 s | 141.2 s | 87.0 s | 86.3 s |
| host peak RSS | 7.53 GB | 8.65 GB | 5.37 GB | 5.37 GB |
| Groth16 container peak (sampled) | | | 7.61 GiB | 7.58 GiB |

A few other figures from the run:

- **Tree.** Step 2's tree holds one leaf, carol's private burn (block
  121, transaction 0). Alice's public burn (block 85) and the refused
  public burn included in block 84 are not in it.
- **Layout check.** `wrap chain-layout` checked all 290 blocks the run
  read: 591 public and 2 privacy-preserving transactions, decoded by LEZ's
  `Block` and the mirror, re-encoded byte for byte, hashed alike, and the
  same leaves found by the scan and by the typed mirror.
- **Recursion.** The made-up-block chain proof (1 block, 113,530 user
  cycles) took 19.0 s. An earlier version of the chain guest decoded each
  block into owned vectors. On the same kind of input (the development
  run's first 38 blocks, 1.9 MB, the program deploys included) it took
  19.4M cycles and 1,413 s. The allocation-free scan takes 3.1M cycles on
  those blocks, six times fewer, most of the saving in paging.
- **Seal.** 260 bytes, RISC Zero's selector `73c457ba` and a Groth16
  proof. Release calldata is 1,732 bytes.

Gas on anvil (gas used, including the 21,000 base and the calldata):

| release (ZkChainBurnVerifier) | expected | gas used |
|---|---|---:|
| `release-carol-not-final` | `NotFinal` | 289,361 |
| `release-carol-redirected-to-relayer` | `NotThisRelease` | 60,935 |
| `release-carol-another-amount` | `NotThisRelease` | 60,935 |
| `release-carol-front-run` | `NotThisRelease` | 60,935 |
| `release-carol-tampered-journal` | `VerificationFailed` | 289,097 |
| `release-carol-another-anchor` | `NotThisChain` | 60,767 |
| `release-carol-membership-proof-only` (a `ZkBurnVerifier` proof) | `NotThisRelease` | 57,571 |
| `release-forged-chain-committee` (a real proof over a made-up block; the members sign the real block at that height) | `BlockNotAttested` | 301,428 |
| `release-forged-chain-outsiders` (the same, the made-up block signed by two outsiders) | `BlockNotAttested` | 301,408 |
| `release-carol-one-member` | `BlockNotAttested` | 294,011 |
| `release-carol` (by the relayer, fee 0.01 ETH) | applied | **347,250** |
| `release-carol-replay` | `AlreadyReleased` | 53,312 |

That is 3,700 gas more than the same release through `ZkBurnVerifier`
(343,550): the longer journal and the five pinned words. RISC Zero's
`verify` alone is 253,306 gas.

**Tests.**

- `chain/`: 10 unit tests.
- `executor-tests/tests/chain.rs`: 2 tests. The scanned `Append` index is
  wrap_bridge's, and the committed blocks in `vectors/lez-blocks/` (83, the
  private claim; 84, a refused public burn that is still in its block; 85,
  a public burn; 121, the private burn) decode transaction by transaction
  with LEE's own types.
- `vault/test/ZkChainBurnVerifier.t.sol`: 20 Foundry tests on the run's
  real seal (fixture `vault/test/fixtures/zk_chain_release.json`).
- `scripts/mutation-test-vault.sh`: 42 of 42 caught, 20 of them new for this
  verifier and its committee (`docs/raw/mutation-vault-2026-10-19-chain.txt`).
  Its first run missed three, because three tests could not tell the
  mutation from the original. A member and an outsider were not tested
  together. The malleated-signature test malleated a member's second
  signature, which counts once anyway. The pairing mutation hit a doc
  comment. The tests and the mutation were fixed, and the rerun catches
  all 42.
- **Reproducibility.** `lez_chain.bin` (294,832 bytes, ImageID `68718ab0…`)
  and `burn_release.bin` (330,048 bytes, ImageID `fd3b52ce…`) were rebuilt
  from scratch in RISC Zero's pinned builder (r0.1.91.1), identical.
  `wrap_bridge.bin` (`073f046c…`) and `burn_membership.bin` (`855dfc80…`)
  are untouched.

**Caveats.**

- **No Bedrock.** The run has no Bedrock: "final" is the standalone
  sequencer's tip, `FINALITY_DEPTH` (3) blocks above the proven tip. The
  members read the block hash from the sequencer. A real member would read
  it from its own LEZ indexer, which reaches a block only if every block
  before it applied, so its attestation would also cover validity as that
  node sees it.
- **Anonymity set.** The chain-derived tree holds one private burn, so this
  release's anonymity set is 1. In the program's tree it was 2 (alice's
  public burn is not in this tree).
- **Second vault.** The chain vault is a second `WrapVault` with the same
  pairing, deployed after the burns, with the anchor set to the block
  before the bridge's Init. A real deployment pins the anchor when the
  vault is deployed, and predicts the vault's address before deploying the
  verifier.
- **Groth16.** It was measured under emulation only.

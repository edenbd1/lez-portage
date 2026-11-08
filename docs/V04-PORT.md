# Portage on LEZ v0.4.0-rc1: what changes, and a port spike

RFP-022 asks for LEZ program delivery on testnet 0.4. LEZ v0.4.0-rc1
(logos-execution-zone tag `v0.4.0-rc1`, commit `1806e873`, 2026-10-29) is the
first tag of that line. This document lists what it changes for each part of
Portage, citing upstream files and lines at that tag, and records a spike on
branch `v0.4-spike` that ports the parts that matter most and measures them.
Nothing here is merged into `main`; the v0.3.1 stack on `main` is unchanged.

Upstream paths below are relative to the logos-execution-zone checkout at
`v0.4.0-rc1` unless they name `lez-programs` (logos-blockchain/lez-programs).

## 1. What v0.4.0-rc1 changes

### 1.1 Programs: the actor model

v0.3 ran a program's `plan` once over a list of account rows and then
`apply` once per effect, on any of those rows. v0.4 replaces this with actors:

- An **actor** is an account paired with a program
  (`lee/state_machine/core/src/account.rs:194`, `Actor { account_id,
  program_account_id }`). Each account holds one state per program
  (`actor_states`).
- A run handles **one message to one actor** and may change **only that
  actor's state** (`lee/state_machine/core/src/program/mod.rs:373`
  `ReceiveInput { receiver, from, is_authorized, pre_state, message }`,
  `:428` `Response { post_state, sync_messages, async_messages, events,
  validity }`; entry point `actor_entrypoint!`, `:14`).
- Everything else is a message: **sync** (same transaction, depth first, a
  refusal anywhere fails the transaction) or **async** (executed in the same
  transaction only if its destination is declared, otherwise left in the
  mailbox; `lee/state_machine/core/src/execution_state.rs:1372-1399`).
- A program **cannot read another program's state**. A check that v0.3 made
  as an effect on another row becomes a message to the actor that owns the
  state, which refuses (the token program even has `ReadState`, answered with
  a sync `StateReply`; lez-programs `programs/token/src/read_state.rs`).
- **Authorization**: an actor is authorized if its account signed the
  transaction, or if a message to it carries a PDA seed of the sending
  program that derives its address (`execution_state.rs:1431-1476`,
  `authorize`); a seed grant is inherited down the sync chain it starts.
- **Admission**: every public actor reached must be declared in the
  transaction's public context, and its account must exist, be seeded by the
  delivering message, or be backed by admission evidence
  (`PublicAccountEvidence::{Key, Pda}`) (`execution_state.rs:1190-1210`,
  `lee/state_machine/src/validated_state_diff/public_backend.rs:104`).
- A public transaction is a **root call** to one actor
  (`lee/state_machine/src/public_transaction/message.rs:19`,
  `PublicExecution { root: RootCall { to, message }, fee }`), not an
  instruction to a program over rows.

### 1.2 Blocks: the reverted list, and charged transactions only

- `BlockBody` gains `reverted: Vec<u64>`, the indices of the charged
  transactions whose action reverted, ascending
  (`lez/common/src/block.rs:60-66`), and `HashableBlockData` gains the same
  field after `transactions` (`:122-128`), so **the block hash covers it**.
  The hash is still `sha256("/LEE/v0.3/Message/Block/"‖0^8 ‖ borsh(...))`
  (`:137`).
- A node applying a block refuses it if the list differs from what
  settlement reverted (`lez/chain_state/src/apply.rs:214-219`,
  `BlockIngestError::RevertMismatch`). So in a valid block, a transaction
  took effect exactly when its index is not listed.
- Every block ends with the forced fee transaction and the clock transaction
  (`apply.rs:150-170`, `split_last` twice); indices in `reverted` are user
  transaction positions.
- A user public transaction must declare a fee
  (`lez/chain_state/src/classify.rs:113-175`, `MissingFeeDeclaration`); a
  failed charged action is included, keeps the fee and is listed
  (`apply.rs:573-699`, `settle_charged_transaction`).
- **Private transactions pay their fee inside their proof**
  (`classify.rs:118-145`: `MissingPrivateFee` without one;
  `lee/state_machine/core/src/circuit_io.rs:160` `ProvenFee { refund_to,
  fallback }`). A private transaction whose public part fails reverts to its
  fee fallback and **is listed in `reverted` too**
  (`apply.rs:435-555`, `Ok((events, charge.revert.is_some()))`). On v0.3.1 a
  failing private transaction invalidated its block; on v0.4 it does not.
- **At most 4 private transactions per block**: each is priced at
  `PRIVATE_GAS_STOR = PROOF_BYTES + PRIVATE_PAD_BYTES = 224,063` storage gas
  against `MAX_GAS_STOR = 1,000,000` (`lez/programs/fee/core/src/market.rs:27,
  51, 56`), and `PRIVATE_VERIFY_GAS = 409,764` execution gas each against
  `MAX_GAS_EXEC = 10,000,000` (`:15, 49`).
- **Still no state root**: neither the header (`block.rs:50-56`: `block_id,
  prev_block_hash, hash, timestamp`) nor anything else in `lez/common`,
  `lez/chain_state` or `lee` names one (a search for `state_root` finds
  none).
- The private transaction's message is now
  `TransactionMessage<ProvenExecution>`
  (`lee/state_machine/src/privacy_preserving_transaction/message.rs:10`;
  `circuit_io.rs:498-515`: `boundary, async_messages, recovery_bindings,
  public_root, private_actions, validity, program_image_claims, fee`). A
  private transaction may reveal a **public root call** (`public_root`).

### 1.3 Canonical token and ATA programs

The token program is no longer in logos-execution-zone; the canonical one is
in lez-programs. Its `main` still pins LEZ `v0.2.5-rc2`; the actor-model port
is on branch `refactor/lez-v0.4.x` at `2801ea1` (2026-11-06,
"port the token program to LEZ's actor model"), pinned to LEZ `616256b9`
(149 commits before the rc1 tag, 14 not in it; `lee_core::program`, the
program ABI, is identical at both). Each token account is an actor holding
one `TokenState` (a definition or a holding); `Mint` goes to the
authority's actor, which forwards `IncreaseSupply` to the definition, which
credits the recipient with an async `IncreaseBalance`
(`programs/token/src/mint.rs`); `Burn` debits the holding and tells the
definition (`burn.rs`); a credit lands only on an existing holding of the
same definition (`transfer.rs:37-56`). **The ATA program has no actor-model
port yet** (`programs/ata` is unchanged on that branch, and its integration
tests are not built: `programs/integration_tests/Cargo.toml`,
`autotests = false`).

### 1.4 Bedrock

LEZ v0.4.0-rc1 pins the Bedrock node at `d480efc6` (v0.3.1 pinned
`3926a532`; 36 commits). Between them `core/src/header` is unchanged and the
inscription op's layout is unchanged (`core/src/mantle/ops/channel/inscribe.rs`:
only the verification plumbing moved); `core/src/mantle/ops/mod.rs` gains
test vectors and `transactions/codec.rs` changes 11 lines. The sequencer
still inscribes `borsh(Block)` signed by its channel key
(`lez/sequencer/actors/bedrock/src/actor.rs:353-364`), but v0.4 adds staked
decentralized sequencing (`lez/programs/sequencer_stake`,
`lez/sequencer/actors/slasher`): the channel can be written by any staked
key, not one pinned key.

## 2. What it means for each part of Portage

### 2.1 wrap_bridge

Ported in the spike (`v04/programs/wrap_bridge`, section 3). What changed:

| v0.3 (`programs/wrap_bridge`) | v0.4 (`v04/programs/wrap_bridge`) |
|---|---|
| one `plan` over rows, effects on config, entry, nullifier, burns | one actor per PDA; the root call goes to the nullifier (Claim) or the burn tree (Burn), which messages the others |
| `Attest` as a chained call over Sextant's header row | a sync `Attest` message to Sextant's header actor (`v04/programs/sextant_attest`) |
| mint: chained `Mint` to the token program with the definition's seed | the definition's mint authority is the entry PDA; the entry sends `Mint` to its own token actor with its seed, which forwards to the definition |
| mint to the destination's ATA (ATA program) | mint to the destination's own token actor (no ATA port upstream); the destination opens its holding first |
| `Burn` via the ATA program, `BurnFrom` from a holding | `Burn` moves the holder's tokens into the entry's escrow holding (which refuses another definition), then the escrow burns them: the burn is of the wrapped token by construction, without reading the holding |
| a burn from any caller | `Burn` only as a transaction's **root call**, so its leaf is in the block |
| supply holding PDA at Register | the entry's own token actor is the zero holding `NewDefinition` needs (authorized by the entry's seed, inherited down the sync chain) |

### 2.2 chain/ (block decoding)

Ported (`chain/v04`, crate `lez-chain-v04`: a crate of its own, since a module inside `lez-chain` moved the bytes of the committed v0.3.1 guest `artifacts/zk/lez_chain.bin`, which CI rebuilds and compares). The mirror gains `reverted`, the public message
(public context, root call, nonces map, admission evidence) and the whole
`ProvenExecution`. The hash formula of `content_hash` is unchanged (the
reverted list sits between the transactions and the unhashed Bedrock status
byte). The burn rule changes from "the `Append` effects of private
transactions" to **"root calls that are `Burn` to this deployment's burn
tree, in transactions not listed in `reverted`"**, public or private
(`public_root`). This is what makes **public burns derivable from blocks**,
which v0.3.1 could not do (docs/BEDROCK-FINALITY.md: a failed public
transaction stayed in its block unmarked). It also makes the v0.3.1 rule
unsound on v0.4: a private transaction can now revert and stay in its block.

### 2.3 zk/lez_chain

A v0.4 guest is added (`zk/lez_chain_v04`, ImageID `23396bb9…`, measured in
section 4). Its journal is 36 words (the pairing is word 4, since leaves are
computed from root-call fields under it). Not yet ported: recursion over a
prior receipt (zk/lez_chain has it).

### 2.4 zk/bedrock_anchor (and the release guests)

Not ported. `bedrock/src/compose.rs:71-76` decodes the inscribed LEZ block
with the v0.3.1 scan and parses the v0.3.1 chain journal; both must move to
`chain/v04` (the inscribed payload is a v0.4 `borsh(Block)`, and the
content hash is the same formula). The Bedrock-side checks
(`bedrock/`: header id, body root, tx hash, inscription, signature) need a
re-run of `bedrock/tools` against lb-core `d480efc6`. The pinned sequencer
key becomes a set: with staked sequencing the channel's writers are the
staked keys, so the anchor must accept any key the stake program lists at
that height, or pin a committee of them. zk/burn_release and
zk/bedrock_release parse the chain journal and change with it; so do the
ImageIDs the vault's verifiers pin.

### 2.5 The vault

`WrapVault.sol` and the release digest do not depend on LEZ's version. The
verifiers that pin guest ImageIDs and journal layouts
(`ZkChainBurnVerifier`, `ZkBedrockBurnVerifier`) need the v0.4 guests'
ImageIDs and the 36-word chain journal. A public burn's leaf is the same
`burn_leaf(pairing, token, amount, eth_commitment)`, so a release of a public
burn is the same proof shape as of a private one.

### 2.6 The private paths

Not ported. v0.3's private claim (a privacy-preserving transaction minting to
a fresh private account, no public signer) and private `BurnFrom` change
shape: the fee is paid inside the proof from a witnessed private payer
holding native balance (`execution_state.rs`: `UnwitnessedFeePayer`,
`FeePayerWithoutAuthorization`), so **a private claimant needs a funded
private account**, which v0.3.1 did not require. A private holder can burn
with a public root call (`public_root`) and stay hidden while the leaf is
public, which the v0.4 chain rule already counts.

## 3. The spike

All on branch `v0.4-spike`, alongside the v0.3 tree:

- `v04/programs/wrap_bridge`: the port (Init, Register, SetPolicy, Freeze,
  Claim, Burn), refusal codes as on v0.3 (`E74xx`).
- `v04/programs/sextant_attest`: the Attest half of Sextant on the actor
  model: a header record actor per slot, and `Attest` verbatim
  (`sextant_core::statement::evaluate`). **The light client is not ported**:
  a record is written by one pinned key (the instance id is
  `sha256("sextant-attest/writer" ‖ writer)`), a stand-in for it.
- `v04/artifacts/token.bin`: lez-programs `2801ea1` built against the
  v0.4.0-rc1 `lee_core` (v04/scripts/build-guests.sh).
- `v04/executor-tests`: the three programs through `lee::V03State` at
  v0.4.0-rc1 (`ValidatedStateDiff::from_public_transaction_metered`, the
  call settlement makes), on the anvil deposits of vectors/anvil (the same
  vault pairing as the v0.3 tests: the program account id is the same).
- `chain/v04` and `zk/lez_chain_v04`.
- `v04/scripts/e2e-seq.sh` + `seq_drive`: a local standalone v0.4.0-rc1
  sequencer (built from the tag with `--features standalone`), the three
  programs loaded through the program loader, a local anvil vault with this
  deployment's pairing, and the bridge driven over JSON-RPC with
  fee-declaring signed transactions. Every block is saved
  (vectors/lez-blocks-v04, evidence/v04-spike/e2e-seq).

Guest builds use the local RISC Zero toolchains, with local paths remapped;
they are not Docker-reproducible yet, so ImageIDs are recorded, not pinned
(evidence/v04-spike/image-ids.txt).

## 4. Results

### 4.1 Tests

- `v04/executor-tests`, `tests/bridge.rs`: **11 passed**. Register (zero
  supply, the entry as mint authority and escrow), admin-only register, a
  claim minted once to its committed destination (replays under the same or
  an earlier root refused, `E7402`), redirection and a forged amount refused
  (`E7403`), the root attested (an unknown root fails `Attest`, `E7296`; a
  proof of another slot `E7404`; an unrecorded header is an unadmitted actor),
  the destination's holding required, freezes and the cap (`E7401`, `E7406`,
  `E7409`), a burn (supply down, escrow empty, leaf appended, event), and
  burns refused: not a denomination (`E7408`), not signed by the holder,
  another token's holding (the escrow refuses it), a wrong salt (`E7407`).
- `v04/executor-tests`, `tests/chain_v04.rs`: **4 passed**, on 11 real
  blocks of the local v0.4.0-rc1 sequencer: the mirror and LEZ's own
  `common::block::Block` both decode every block and re-encode it to the same
  bytes; LEZ's `recompute_hash`, the header and `content_hash` agree; the
  reverted list is exactly the three transactions the sequencer reverted;
  **four burns are in the blocks, two applied and two reverted, and the tree
  rebuilt from the blocks alone holds the two applied ones, with the root of
  the program's own burn tree read back from the sequencer**
  (`0x40427808…`); un-listing a reverted burn breaks the block hash.
- `chain/v04` (crate `lez-chain-v04`): **7 passed**, and `chain` (v0.3.1, unchanged) 10 as before. The v0.4 tests:
  the hash covers the reverted list; public and private-root burns count
  unless reverted; other accounts, programs, variants and private roots do
  not; chaining; every truncation and every flipped byte borsh refuses, the
  scan refuses; out-of-order sets and bad reverted lists refused).

### 4.2 The sequencer run (evidence/v04-spike/e2e-seq/manifest.tsv)

13 program-loader transactions (3 programs), then Record, Init, Register,
alice's holding, two claims (applied), a replayed claim (reverted, block 24),
burns of 0.1 ETH and 1 ETH (applied, blocks 25 and 27), of 12,345 wei (not a
denomination) and of 1 ETH beyond alice's balance (reverted, blocks 26 and
28). All on a local sequencer; nothing on a public network.

### 4.3 Cycles

Per transaction, in the executor (`cycles_per_transaction`; cycles are the
execution gas a transaction is charged, `MAX_GAS_EXEC = 10,000,000` per
block):

| transaction | v0.4.0-rc1 cycles | v0.3 gas, same machine and vectors |
|---|---:|---:|
| Record (the Attest stand-in's header write) | 24,995 | (sextant_lc Init: 1,831,160) |
| Init | 63,464 (also creates the burn tree) | 27,049 |
| Register | 84,958 | 90,958 |
| InitializeHolding (token) | 30,947 | (none: ATA created in the claim) |
| Claim, 1 ETH | 259,022 | 297,457 |
| Burn, 0.1 ETH | 198,834 | 228,728 (1 ETH) |

(v0.3: `executor-tests --bin measure` at `main`, evidence/v04-spike/cycles-v03-same-machine.md.)
The Claim is 13% cheaper on v0.4 with the same `statement::evaluate` (the
storage-proof check), although it is seven guest runs (nullifier, Sextant's
header, config, entry, and the token program at the entry, the definition and
the destination); the Burn is 13% cheaper, also seven runs (burn tree,
config, entry, and four of the token program).

The v0.4 chain guest in the executor (`measure_chain`; user cycles are the
guest's own, total the padded segments; evidence/v04-spike/cycles-chain-guest.md):

| input | blocks | bytes | burns | user cycles | total cycles |
|---|---:|---:|---:|---:|---:|
| real blocks 18-28 | 11 | 28,186 | 2 | 283,753 | 524,288 |
| 1 x 18 public burns (1 reverted) | 1 | 23,227 | 17 | 1,241,142 | 1,572,864 |
| 10 x 18 public burns | 10 | 232,270 | 170 | 11,951,298 | 12,845,056 |
| 50 x 18 public burns | 50 | 1,161,110 | 850 | 59,504,775 | 63,438,848 |
| 1 x 4 private txs (223,551-byte proofs) | 1 | 915,857 | 0 | 1,445,810 | 3,145,728 |
| 5 x 4 private txs | 5 | 4,579,285 | 0 | 5,803,178 | 10,747,904 |
| 10 x empty (fee + clock) | 10 | 4,850 | 0 | 92,533 | 131,072 |

So about **9.3k cycles per block** of overhead, **1.3 cycles per byte** of
private transaction (the scan skips proofs; v0.3.1 measured ~1.5), and about
**69k cycles per counted burn** (one leaf hash and a depth-24 frontier
insert; the root-call parse is a small part). A full block of 18 public
burns is ~1.24M cycles; a day of 2 s blocks that are all full burn blocks
would be ~54G cycles, so the per-burn cost, not the block scan, is what to
optimize (batching the frontier's upper levels per block).

## 5. Gaps, with estimates

| gap | what it takes | estimate |
|---|---|---|
| Sextant's light client on v0.4 (the record writer stands in) | port `Init`/`Begin`/`Miller`/`Finish` to actors: the job account is already one actor; the instance and header become actors written by sync messages | 2-3 days |
| ATA | wait for lez-programs' port, or keep minting to the account's own token actor (one wrapped token per account) | upstream; 0.5 day to adopt |
| Private claim and private burn | hybrid transactions (public root, private holder or destination), the in-proof fee from a funded private payer; re-measure proving | 3-4 days |
| zk/bedrock_anchor, burn_release, bedrock_release on v0.4 | move compose.rs to chain/v04 and the 36-word journal; re-verify bedrock/ against lb-core d480efc6; accept staked sequencer keys | 2 days |
| Chain guest recursion and per-burn cost | prior-receipt recursion as in zk/lez_chain; batch frontier inserts | 1 day |
| Vault verifiers | new ImageIDs and journal layout in ZkChainBurnVerifier / ZkBedrockBurnVerifier, Foundry tests and mutations | 1 day |
| Reproducible builds | Docker builds of the v0.4 guests (RISC Zero's pinned builder) and a CI cmp | 0.5 day |
| A private transaction in real v0.4 blocks | the scan's private path is tested on synthetic blocks in the exact layout only | with the private paths |
| testnet 0.4 | deploy on the public 0.4 network when it exists (needs funds) | 1 day |

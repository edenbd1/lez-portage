# Portage on LEZ v0.4: what changes, and a port

RFP-022 asks for LEZ program delivery on testnet 0.4. LEZ v0.4.0-rc1
(logos-execution-zone tag `v0.4.0-rc1`, commit `1806e873`, 2026-10-29) is the
first tag of that line; **branch `v0.4-spike` now runs on `v0.4.0-rc3`**
(commit `bafd07be`, 2026-11-11), the latest release candidate, and **on
Sextant's real light client ported to v0.4**, which replaces the spike's
pinned-key stand-in (section 0), **with the ERC-20 path, a v0.4 client and
reproducible builds of every v0.4 binary** (section 0.1). This document lists what v0.4 changes for each
part of Portage, citing upstream files and lines, and records the port and its
measurements. Nothing here is merged into `main`; the v0.3.1 stack on `main`
is unchanged.

Upstream paths below are relative to the logos-execution-zone checkout at
`v0.4.0-rc1` unless they name `lez-programs` (logos-blockchain/lez-programs)
or say otherwise; section 1 was checked again at rc3 and holds there.

## 0. 2026-11-14: rc3, and Sextant's light client instead of the stand-in

**rc1 to rc3.** 67 commits. For Portage's programs: the clock is one actor read
by message (PR #988: `ReadTimestamp`/`ReadBlockId` with a granularity, replies
to an actor only; Portage reads no clock), the program loader's state is a
tagged `LoaderEntry` and its reads are charged at one cycle per byte, and an
outbound cross-zone send pays a fee like any user transaction. The program ABI
(`ReceiveInput`, `Response`, validity windows), the block layout (the
`reverted` list, the hash formula) and the public message layout are
unchanged: chain/v04 decodes rc3 blocks as it decoded rc1's, and the rc1
wrap_bridge source builds against rc3 as is. The clock transaction at the end
of every block now targets the single clock actor. A transaction whose
validity window has closed is included, charged and reverted at settlement.

**Sextant.** The spike's `v04/programs/sextant_attest` (a header record per
slot written by one pinned writer key, a federation of one) is gone.
`v04/artifacts/sextant_lc.bin` is Sextant's light client on v0.4
(edenbd1/lez-sextant branch `v0.4`, `v04/programs/sextant`, reproducible
Docker build, ImageID `bf3df59e…`; its docs/V04.md): `Init` from a checkpoint
(the bootstrap's committee branch and execution header verified),
sync-committee updates in three transactions (`Begin`, `Miller`, `Finish`), and
`Attest`, the message wrap_bridge's Claim sends to the header actor, with
Sextant's own refusal codes. Here:

- **The claim path on the real light client.** The executor tests and the
  sequencer run initialise Sextant with `Init` on a devnet beacon header that
  commits to the anvil block's execution header and to a committee (as the
  v0.3 tests do; anvil has no beacon chain): the light client verifies that
  bootstrap and records the header, and every claim's `Attest` reads that
  record. A claim against a tree the vault never had is now refused by Sextant
  itself (`E7207`, its inclusion check; the stand-in answered `E7296`).
- **A real Sextant attestation of the real Sepolia deposits**
  (`v04/executor-tests/tests/sepolia.rs`): sextant_lc on v0.4 is initialised
  from the real Sepolia checkpoint finalized after the two deposits into the
  Sepolia WrapVault (slot 11,536,448, execution block 12,092,950, lez-sextant
  `vectors/sepolia-portage`), and attests the exact statement wrap_bridge's
  Claim sends (`knownRoot[root] == 1` at the vault, over the vault's real
  account and storage proofs at that block, `evidence/testnet/sepolia`) for
  the vault's three roots (154,004 to 158,185 cycles each); a root the vault
  never had is refused `E7208`, as on the v0.3 testnet, and the same
  statement as a mainnet one `E7209`. The full Claim of a Sepolia deposit
  (the mint) is not run: it mints to the deposit's committed destination,
  whose holding only that destination's key can open.
- **The token program** is lez-programs `refactor/lez-v0.4.x` at `2801ea1`
  rebuilt against rc3; the branch's head (`4c0b8c7`) changes only comments in
  `programs/token`. lez-programs now also has an actor-model **ATA** port
  (`refactor/ata-lez-v0.4.x`, `064b73f`), not adopted here (section 5).

Results on rc3 (evidence/v04-rc3): executor tests 11 + 9 + 4 + 1 passed
(`executor-tests.txt`; the 9 are the ERC-20 tests of section 0.1), chain/v04
7 passed, and a local rc3 sequencer run (section 4.2).

## 0.1 2026-11-14 (later): the ERC-20 path, the v0.4 client, reproducible builds

**The ERC-20 path.** wrap_bridge on v0.4 needed no change for an ERC-20: an
entry, its definition and its escrow are per Ethereum token address, the
deposit leaf and the burn leaf commit to that address, and the policy is in
the token's own base units. `v04/executor-tests/tests/erc20.rs` (**9 tests**)
runs the USDT-shaped ERC-20 of vectors/anvil (6 decimals, transfers that
return nothing; deposit 1, 100 USDT, committed to alice) through the three
programs: the entry is its own wrapped token (zero supply, the entry as mint
authority); the claim mints 100,000,000 units once (the same opening and an
earlier root refused, `E7402`); a root the vault never had is refused by
Sextant (`E7208`, the vault's proof shows `knownRoot[root]` empty) or by the
bridge (`E7404`, a proof of another slot); **the wrong token** at the claim (the
USDT leaf claimed as ETH or as an unregistered ERC-20, an ETH leaf claimed as
USDT: `E7403`; the right leaf under another definition salt: `E7407`);
freezes and the cap per token (USDT frozen, ETH still claims); the burn (leaf
naming the ERC-20, supply and escrow back to zero); and burns refused (not a
USDT denomination, an ETH denomination, more than held, unsigned, wrapped USDT
burned as ETH and wrapped ETH burned as USDT, each refused by the other entry's
escrow). Cycles (evidence/v04-rc3/cycles.md): Claim of 100 USDT 263,398, Burn
199,098, as for ETH.

What the ERC-20 path shows about v0.4 without an ATA program: an account has
one token actor per token program, so **an account holds one wrapped token**.
`one_wrapped_token_per_account_without_an_ata` pins it: alice, holding wrapped
ETH, cannot open a USDT holding, and her USDT claim reverts whole (the
nullifier is not spent). Until an ATA is adopted (section 5), a depositor
commits each token to its own LEZ account; the sequencer run does (alice: ETH,
bob: USDT).

(`erc20_receiver`, lez-sextant's small consumer of Sextant that credits ERC-20
`Transfer` logs, is not part of Portage: Portage claims against the vault's
`knownRoot` storage, not against transfer logs, and deploys only token,
sextant_lc and wrap_bridge. It is not ported here.)

**The v0.4 client.** `wrap` (`v04/executor-tests/src/bin/wrap.rs`, RPC in
`src/seq.rs`) is the v0.4 counterpart of the v0.3 client (`cli/`): `deploy`,
`plan`, `init-sextant`, `init-bridge`, `register`, `open-holding`, `claim`
(`--root-after`, `--redirect-to`, `--as-token`, `--phantom`), `burn`,
`balance`, `release-proof` (a burn's leaf, index, path to the chain's burn-tree
root, nullifier) and `finish` (blocks and manifest). **Every transaction is
pre-checked** on rc3's own state machine (`ValidatedStateDiff::
from_public_transaction_metered`) with the committed binaries at the deployed
program ids and the declared accounts as the sequencer holds them
(`getAccount`); a refused one is reported with its typed code and sent only
with `--force`, and the client fails if the block's `reverted` list disagrees
with the pre-check. It replaces the spike's `seq_drive`.

**Claims are public on v0.4; the private claim is not the default because it
does not exist yet.** On v0.4 a private claim is a privacy-preserving
transaction whose fee is paid inside the proof by a funded private account
(section 2.6), and the bridge's private paths are not ported; `wrap claim
--private` is refused by the client with that reason rather than silently sent
in public. (The v0.3 client claims publicly unless `--private` too; on v0.3 the
private claim is ported and was run on the public testnet, docs/TESTNET.md.)

**The sequencer run** (section 4.2) is now driven entirely by `wrap`, with
ETH and the ERC-20.

**Reproducible builds.** Every v0.4 binary is a Docker build in RISC Zero's
pinned builder image (`r0.1.91.1`, the one lez-sextant uses):
`DOCKER=1 v04/scripts/build-guests.sh token wrap_bridge lez_chain_v04 sextant`.
The token program is lez-programs `2801ea1` with its `lee_core` pin moved to
the rc3 tag and the lockfile committed in `v04/locks/token-guest.Cargo.lock`
(the local build used to regenerate it, so its dependencies floated).
CI's `reproduce-v04` job rebuilds each on a clean runner and `cmp`s it against
`v04/artifacts` (evidence/v04-rc3/image-ids.txt):

| binary | bytes | ImageID |
|---|---:|---|
| token | 230,252 | `5a238c4922c0eea070a29d9e2438cff6a03bd56a4293e3488570a46418fba9d1` |
| wrap_bridge | 411,136 | `5658d68962190df5dc84f54a18cb22b5431c630e717b997ae71705c0afbc2df3` |
| lez_chain_v04 | 291,496 | `9da6eb51f279f827a6b6f04f524c1c958f0fdfe2ff711aa34b36e6bfb3ae5c2c` |
| sextant_lc | 616,172 | `bf3df59e28d180ca1109cc39b0a5fd479106d007b49f1d1dc5e48aa8fc6e530a` (unchanged, lez-sextant `1e55629`) |

None contains a build-machine path. The earlier local-toolchain builds (token
`7b551db2`, wrap_bridge `eed96b93`, lez_chain_v04 `23396bb9`) are replaced; the
programs' behaviour is the same (every test passes on both).

**Run it:**

```console
cargo test --release --manifest-path chain/v04/Cargo.toml                # 7
cargo test --release --manifest-path v04/executor-tests/Cargo.toml        # bridge 11, erc20 9, chain_v04 4, sepolia 1
cargo run --release --manifest-path v04/executor-tests/Cargo.toml --bin measure_chain
DOCKER=1 v04/scripts/build-guests.sh token wrap_bridge lez_chain_v04 sextant
# a local rc3 sequencer (logos-execution-zone v0.4.0-rc3, cargo build --release --features standalone -p sequencer_service) and anvil:
SEQ=<lez@v0.4.0-rc3>/target/release/sequencer_service LEZ=<lez@v0.4.0-rc3> v04/scripts/e2e-seq.sh <run dir>
python3 scripts/v04-run-json.py <run dir> vectors/lez-blocks-v04
```

## 1. What v0.4 changes (found at rc1, checked at rc3)

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
| `Attest` as a chained call over Sextant's header row | a sync `Attest` message to Sextant's header actor (sextant_lc on v0.4, lez-sextant `v0.4`) |
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

A v0.4 guest is added (`zk/lez_chain_v04`, ImageID `9da6eb51…`, reproducible, measured in
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
- `v04/artifacts/sextant_lc.bin`: Sextant's light client on v0.4 (section 0;
  the rc1 spike used a pinned-key stand-in, `v04/programs/sextant_attest`,
  now removed).
- `v04/artifacts/token.bin`: lez-programs `2801ea1` built against the
  v0.4.0-rc3 `lee_core` (v04/scripts/build-guests.sh, Docker, with the
  committed lockfile v04/locks/token-guest.Cargo.lock).
- `v04/executor-tests`: the three programs through `lee::V03State` at
  v0.4.0-rc3 (`ValidatedStateDiff::from_public_transaction_metered`, the
  call settlement makes), on the anvil deposits of vectors/anvil (the same
  vault pairing as the v0.3 tests: the program account id is the same).
- `chain/v04` and `zk/lez_chain_v04`.
- `v04/executor-tests/tests/erc20.rs`: the ERC-20 path (section 0.1).
- `wrap`, the v0.4 client (`v04/executor-tests/src/bin/wrap.rs`, section 0.1).
- `v04/scripts/e2e-seq.sh` + `wrap`: a local standalone v0.4.0-rc3
  sequencer (built from the tag with `--features standalone`, run with
  `RISC0_DEV_MODE=0`), the three
  programs loaded through the program loader, a local anvil vault with this
  deployment's pairing and the USDT-shaped ERC-20, and the bridge driven by
  `wrap` over JSON-RPC with fee-declaring signed transactions, each
  pre-checked on rc3's state machine. Every block is saved
  (vectors/lez-blocks-v04, evidence/v04-rc3/e2e-seq; the rc1 run is kept in
  evidence/v04-spike/e2e-seq).

Every v0.4 binary (token, wrap_bridge, lez_chain_v04, sextant_lc) is a
reproducible Docker build, pinned by CI's `reproduce-v04` job (section 0.1,
evidence/v04-rc3/image-ids.txt).

## 4. Results

### 4.1 Tests

- `v04/executor-tests`, `tests/bridge.rs`: **11 passed**. Register (zero
  supply, the entry as mint authority and escrow), admin-only register, a
  claim minted once to its committed destination (replays under the same or
  an earlier root refused, `E7402`), redirection and a forged amount refused
  (`E7403`), the root attested by Sextant's light client (a tree the vault
  never had fails `Attest`, `E7207`; a
  proof of another slot `E7404`; an unrecorded header is an unadmitted actor),
  the destination's holding required, freezes and the cap (`E7401`, `E7406`,
  `E7409`), a burn (supply down, escrow empty, leaf appended, event), and
  burns refused: not a denomination (`E7408`), not signed by the holder,
  another token's holding (the escrow refuses it), a wrong salt (`E7407`).
- `v04/executor-tests`, `tests/erc20.rs`: **9 passed**, the ERC-20 path
  (section 0.1): claim once, double claims, a root the vault never had
  (`E7208`, `E7404`), the wrong token at the claim (`E7403`, `E7407`) and at
  the burn (each entry's escrow), per-token freeze and cap, the burn and its
  refusals, one wrapped token per account without an ATA.
- `v04/executor-tests`, `tests/sepolia.rs`: **1 passed**, the real Sepolia
  attestation of section 0.
- `v04/executor-tests`, `tests/chain_v04.rs`: **4 passed**, on 23 real
  blocks of the local v0.4.0-rc3 sequencer (20 to 42, section 4.2): the mirror and LEZ's own
  `common::block::Block` both decode every block and re-encode it to the same
  bytes; LEZ's `recompute_hash`, the header and `content_hash` agree; the
  reverted list is exactly the eight transactions the sequencer reverted;
  **seven burns (ETH and USDT) are in the blocks, four applied and three
  reverted, and the tree rebuilt from the blocks alone holds the four applied
  ones, with the root of the program's own burn tree read back from the
  sequencer** (`0xf36d194f…`); un-listing a reverted burn breaks the block hash.
- `chain/v04` (crate `lez-chain-v04`): **7 passed**, and `chain` (v0.3.1, unchanged) 10 as before. The v0.4 tests:
  the hash covers the reverted list; public and private-root burns count
  unless reverted; other accounts, programs, variants and private roots do
  not; chaining; every truncation and every flipped byte borsh refuses, the
  scan refuses; out-of-order sets and bad reverted lists refused).

### 4.2 The sequencer run (evidence/v04-rc3/e2e-seq/manifest.tsv)

On a local standalone v0.4.0-rc3 sequencer, `RISC0_DEV_MODE=0`, 2026-11-14,
driven entirely by the v0.4 client `wrap` (v04/scripts/e2e-seq.sh;
transcript.txt): 15 program-loader transactions (token, sextant_lc,
wrap_bridge, the Docker builds), anvil with WrapVault at this deployment's
pairing, the USDT-shaped ERC-20 and four deposits (alice 1 ETH twice, bob 100
and 1,000 USDT), then 22 transactions in blocks 21 to 42, **each pre-checked
on rc3's state machine, and every chain verdict equal to its pre-check**:

- applied: Sextant's `Init` on the devnet header of the anvil block, the
  bridge's Init, Register ETH, Register USDT, alice's ETH holding, bob's USDT
  holding, the four claims (each attested by Sextant), burns of 0.1 ETH and
  1 ETH (alice) and of 100 and 1,000 USDT (bob);
- reverted (sent with `--force`, refused by the pre-check with the code
  shown): two replays (`E7402`, one under an earlier root), the deposit never
  made under a root the vault never had (`E7208`, Sextant), a USDT deposit
  redirected to alice and the same deposit claimed as ETH (`E7403`), 12,345
  USDT units (`E7408`), 100 USDT burned from alice's wrapped ETH (the USDT
  escrow refuses another token), 1,000 USDT beyond bob's balance (the token
  program refuses the debit).

Final state (balances.jsonl): alice 0.9 wrapped ETH, bob 0 wrapped USDT,
supplies 0.9 ETH and 0 USDT. The program's own burn tree, read back:
`0xf36d194f…`, four leaves, equal to the applied burns'; `wrap release-proof`
gives each applied burn's opening against it (release-openings.jsonl). `wrap
claim --private` is refused by the client (private-claim.txt). The blocks are
vectors/lez-blocks-v04. All on a local sequencer; nothing on a public network.
(The previous rc3 run, two ETH claims and four burns by `seq_drive`, is
replaced; the rc1 run, with the stand-in: evidence/v04-spike/e2e-seq.)

### 4.3 Cycles

Per transaction, in the executor (`cycles_per_transaction`; cycles are the
execution gas a transaction is charged, `MAX_GAS_EXEC = 10,000,000` per
block):

| transaction | v0.4.0-rc3 cycles | v0.4.0-rc1 cycles (spike) | v0.3 gas, same machine and vectors |
|---|---:|---:|---:|
| Sextant `Init` (devnet checkpoint; rc1: the stand-in's Record, 24,995) | 1,815,646 | - | 1,831,160 |
| Init | 63,520 (also creates the burn tree) | 63,464 | 27,049 |
| Register (ETH; USDT 84,324) | 84,818 | 84,958 | 90,958 |
| InitializeHolding (token) | 31,448 | 30,947 | (none: ATA created in the claim) |
| Claim, 1 ETH (Attest by sextant_lc) | 263,763 | 259,022 | 297,457 |
| Claim, 100 USDT | 263,398 | - | 297,529 |
| Burn, 0.1 ETH | 199,311 | 198,834 | 228,728 (1 ETH) |
| Burn, 100 USDT | 199,098 | - | - |

(rc3 column: the Docker-built binaries, evidence/v04-rc3/cycles.md. An earlier
version of this table gave 1,672,261 for Sextant's `Init` and 261,204 for the
Claim; the committed tests give 1,815,646 and 263,494 with the previous
binaries too, so those figures were stale, not a change of the build. v0.3:
`executor-tests --bin measure` at `main`, evidence/v04-spike/cycles-v03-same-machine.md.)
The Claim is 11% cheaper on v0.4 with the same `statement::evaluate` (the
storage-proof check), although it is seven guest runs (nullifier, Sextant's
header, config, entry, and the token program at the entry, the definition and
the destination); the Burn is 13% cheaper, also seven runs (burn tree,
config, entry, and four of the token program).

The v0.4 chain guest in the executor (`measure_chain`; user cycles are the
guest's own, total the padded segments; the Docker build `9da6eb51`,
evidence/v04-rc3/measure-chain.md; the rc1 spike's figures, within 1%, are in
evidence/v04-spike/cycles-chain-guest.md):

| input | blocks | bytes | burns | user cycles | total cycles |
|---|---:|---:|---:|---:|---:|
| real rc3 blocks 20-42 (section 4.2) | 23 | 87,566 | 4 | 558,635 | 1,048,576 |
| 1 x 18 public burns (1 reverted) | 1 | 23,227 | 17 | 1,248,529 | 1,572,864 |
| 10 x 18 public burns | 10 | 232,270 | 170 | 12,019,399 | 12,845,056 |
| 50 x 18 public burns | 50 | 1,161,110 | 850 | 59,842,701 | 63,963,136 |
| 1 x 4 private txs (223,551-byte proofs) | 1 | 915,857 | 0 | 1,446,594 | 3,145,728 |
| 5 x 4 private txs | 5 | 4,579,285 | 0 | 5,804,534 | 10,747,904 |
| 10 x empty (fee + clock) | 10 | 4,850 | 0 | 93,794 | 262,144 |

So about **9.4k cycles per block** of overhead, **1.3 cycles per byte** of
private transaction (the scan skips proofs; v0.3.1 measured ~1.5), and about
**70k cycles per counted burn** (one leaf hash and a depth-24 frontier
insert; the root-call parse is a small part). A full block of 18 public
burns is ~1.25M cycles; a day of 2 s blocks that are all full burn blocks
would be ~54G cycles, so the per-burn cost, not the block scan, is what to
optimize (batching the frontier's upper levels per block).

## 5. Gaps, with estimates

| gap | what it takes | estimate |
|---|---|---|
| ~~Sextant's light client on v0.4~~ | done 2026-11-14 (section 0): lez-sextant `v0.4`, on rc3 | - |
| A full Claim of the real Sepolia deposits on v0.4 | the Sextant half is shown (section 0); the mint needs the committed destination to open its holding, so a fresh Sepolia deposit to a v0.4 destination | 0.5 day |
| ATA (one wrapped token per account until then: section 0.1) | lez-programs has an actor-model port (`refactor/ata-lez-v0.4.x`, `064b73f`, which also changes the token program); adopt both, mint to the destination's ATA, rebuild, rerun | 1 day |
| Private claim and private burn (so the client's claim is public by default and `--private` is refused) | hybrid transactions (public root, private holder or destination), the in-proof fee from a funded private payer; the client's private path; re-measure proving | 3-4 days |
| A release on v0.4 | `wrap release-proof` gives a burn's opening against the chain's tree; the release itself (CommitteeBurnVerifier on anvil, or the zk verifiers below) is not run on v0.4 | 0.5 day (committee) |
| zk/bedrock_anchor, burn_release, bedrock_release on v0.4 | move compose.rs to chain/v04 and the 36-word journal; re-verify bedrock/ against lb-core d480efc6; accept staked sequencer keys | 2 days |
| Chain guest recursion and per-burn cost | prior-receipt recursion as in zk/lez_chain; batch frontier inserts | 1 day |
| Vault verifiers | new ImageIDs and journal layout in ZkChainBurnVerifier / ZkBedrockBurnVerifier, Foundry tests and mutations | 1 day |
| ~~Reproducible builds~~ | done 2026-11-14 (section 0.1): token, wrap_bridge, lez_chain_v04 and sextant_lc, CI `reproduce-v04` | - |
| A private transaction in real v0.4 blocks | the scan's private path is tested on synthetic blocks in the exact layout only | with the private paths |
| testnet 0.4 | deploy on the public 0.4 network when it exists (needs funds) | 1 day |

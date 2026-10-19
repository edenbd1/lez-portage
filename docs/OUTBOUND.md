# Outbound: the private burn and the release (F.6, F.7, PP.2)

A wrapped token goes back to Ethereum in two steps. On LEZ, the holder burns
it and the bridge appends a burn leaf to its burn tree. On Ethereum, the vault
releases the escrowed asset against that leaf. This document covers what is
built for each step, what it was measured at, and what the release path rests
on. There are two release verifiers. `ZkBurnVerifier` proves the burn's
membership in zero knowledge, and the local run uses it (section 3).
`CommitteeBurnVerifier` opens the leaf in public (section 2). Both take the
burn tree's root from a committee. A third, `ZkChainBurnVerifier` (section
4), takes no root from anyone: the proof rebuilds the tree from LEZ blocks,
and the committee only attests that the last block is final. That
attestation is the part of F.7 still missing (docs/BEDROCK-FINALITY.md). Record of the run: [`evidence/local/`](../evidence/local/); how it runs:
[LOCAL-E2E.md](LOCAL-E2E.md).

## 1. The burn

```text
eth_commitment = sha256("rfp022/eth-commit/v1" ‖ recipient ‖ fee ‖ secret)     (ABI words)
burn_leaf      = sha256("rfp022/burn/v1" ‖ pairing ‖ token ‖ amount (u128 LE) ‖ eth_commitment)
nullifier      = sha256("rfp022/burn-nullifier/v1" ‖ pairing ‖ secret ‖ index (u64 LE))
secret         = derive_secret(wallet seed, 'b', n)
```

- `Burn`: from the signing owner's associated token account, through the ATA
  program (public).
- `BurnFrom` (new): from the account that holds the wrapped token itself.
  Rows: config, entry, burn tree, definition, and the holding, which must be
  authorized. The chained call is the token program's own `Burn`. On the
  private path the holding is the private account that a private claim minted
  into. The owner's key authorizes it inside LEZ's privacy circuit
  (`WitnessKind::Regular { ask }`), so no public account and no signature
  appear in the transaction.

Both append the same leaf. The Ethereum recipient and the relayer fee are
hidden in `eth_commitment`. The fee is fixed at burn time because a release
opens the leaf in public (section 2): an unbound fee could be raised by
whoever saw the opening first.

### The private burn, measured through the sequencer

Carol burns the 1 wrapped ETH that her private claim minted, from
`Private/9f7CTf9s…`, with a fee of 0.01 ETH. The client (`wrap burn carol eth
1000000000000000000 --fee 10000000000000000 --private`) builds the `BurnFrom`.
The v0.3.0 wallet crate proves it (`send_privacy_preserving_tx`), and the
local v0.3.1 sequencer includes it in block 129. The run is the 2026-10-17
run at commit `c6328fe` on an Apple M4 Pro (14 cores, 24 GiB), with r0vm
3.0.5 and RISC Zero dev mode off. The prover ran under
`$HEAVY` and `/usr/bin/time -l`.

| | private burn | private claim (same run) |
|---|---:|---:|
| prove and submit (`send_privacy_preserving_tx`) | **202.9 s** | 247.6 s |
| the client process, wall | 204.9 s | 249.7 s |
| peak resident memory | **7.22 GB** | 6.17 GB |
| transaction on chain (`getTransaction`, with its variant tag) | **248,358 bytes** | 251,335 bytes |

The burn composes two programs (the bridge and the token program). The claim
composes three, with Sextant's storage-proof check among them. The machine
was busier during the burn (load average 34 before it, against 4.9 before the
claim). The heavy lock keeps a second prover out, but it does not keep other
work off the machine. So the burn's time is an upper bound for an idle
machine, and the burn was measured once.

These checks ran on the transaction's bytes as the chain returns them.
Carol's account id, her Ethereum recipient and the burn secret occur nowhere
in the transaction. The burn leaf does occur in it: it is what a release
opens. Settlement wrote the leaf to the burn tree, which the client checked
against the frontier. Carol's wallet then synced and decrypted her account:
0 wrapped ETH, its new commitment in the chain's set. Wrapped ETH supply went
to 0.

What a private burn publishes: the bridge's config and entry checks, the
token and the amount (`Effect::Burn`), the burn leaf (`Effect::Append`), and
the definition's supply change. Also public: the private account's nullifier
and new commitment, the note's view tag, and the two program image claims.

**A refused private burn.** The burn's policy checks (permitted amount, freeze
flags) are effects on public accounts, and the plan cannot read public
accounts. So the burn proves anyway, and settlement refuses it: the sequencer
drops a fee-exempt transaction that fails settlement and never includes it.
The sequencer sees such a burn, but no block does. The client checks the
policy before it proves (`refused-precheck E7408` in the run). The executor
test `a_private_burn_spends_a_private_holding_and_publishes_no_holder` shows
that settlement refuses with `E7408`, that keys other than the holder's
cannot spend the account, and that a replay is refused.

## 2. The release with a public opening: `CommitteeBurnVerifier`

`WrapVault.release(token, amount, recipient, relayer, fee, lezBurnRoot,
nullifier, proof)` checks, as before, that the token is registered, nothing
is frozen, the amount is permitted, the fee is at most the amount, and the
nullifier is unspent. It then asks `VERIFIER.verify(proof,
releaseDigest(...))`. The verifier is fixed at construction. The verifier
built here is
[`CommitteeBurnVerifier`](../vault/src/CommitteeBurnVerifier.sol), and it
works in two parts.

**Part 1: membership, checked on Ethereum.** The proof opens the burn leaf:
token, amount, recipient, fee, secret, index and the 24-node SHA-256 path.
The verifier recomputes the leaf, the root it leads to and the nullifier, and
requires `releaseDigest` of those to equal the digest the vault asked about.
So the root and the nullifier the vault records are derived, not chosen. Each
leaf releases once, and the recipient, the amount and the fee are the ones
the burner committed to on LEZ. A relayer other than the one named is
refused. Anyone may relay an opening, and earns only the committed fee.

**Part 2: the root, attested by a committee** (`BurnRootCommittee`, shared
by both verifiers). A fixed committee of `n` members signs, under EIP-191:

```text
attestationDigest = keccak256(abi.encode("rfp022/burn-root-attestation/v1", chainid, vault, pairing,
                                         root, lezBlock, lezFinalBlock))
```

This means "the paired deployment's burn tree had `root` at LEZ block
`lezBlock`, and `lezFinalBlock` is the highest block I consider final".
`THRESHOLD` distinct members must sign. A member signing twice counts once.
High-s signatures and outsiders do not count. `lezFinalBlock` must be at
least `FINALITY_DEPTH` blocks above `lezBlock` (F.12). The digest binds the
chain, the vault and the pairing, so an attestation for another deployment
does not apply.

In the local run the committee is anvil accounts 5, 6 and 7, 2 of 3, with a
finality depth of 3 LEZ blocks. Each member reads the burn tree's root from
the sequencer (`getAccount` between two equal reads of the tip) and signs
**the root it read**, not the root the release names (`scripts/anvil_release.py`).

## 3. The zero-knowledge release: `ZkBurnVerifier`

Part 1 moves into a RISC Zero guest, and the vault never sees the opening.
The guest is [`zk/burn_membership`](../zk/burn_membership/src/main.rs); its
statement is `wrap_core::zk::zk_release_journal`
([`core/src/zk.rs`](../core/src/zk.rs)).

**The guest.** It reads the release's context (chain id, vault, pairing,
relayer) and the burn's opening (token, amount, recipient, fee, secret,
index, 24-node path). It recomputes the leaf, the root the path leads to and
the nullifier. Then it commits as its journal the vault's own release
preimage:

```text
journal = abi.encode(RELEASE_TAG, chainid, vault, pairing, token, amount, recipient, relayer, fee, root, nullifier)   (352 bytes)
sha256(journal) = WrapVault.releaseDigest(token, amount, recipient, relayer, fee, root, nullifier)
```

So the digest the receipt commits to is the digest the vault computes itself,
from its own chain id, its own address, its own pairing and the release's
arguments. The guest is built in RISC Zero's pinned builder (`r0.1.91.1`,
[`scripts/build-zk-guest.sh`](../scripts/build-zk-guest.sh)). The result is
`artifacts/zk/burn_membership.bin`: 318,720 bytes, sha256 `2c800cd9…`, ImageID
`855dfc801b07750f6ebb29619e6b4629dba6ae136e07f0981c5a6605e64c2316`. Two builds
from scratch gave the same bytes, and CI rebuilds and `cmp`s it. The `zk`
module is behind a wrap-core feature that wrap_bridge does not enable, so
`wrap_bridge.bin` rebuilds byte for byte as committed (`073f046c…`).

**The prover.** `wrap release-proof OWNER --zk --relayer ADDR` (or `wrap
zk-prove INPUT.json`) goes through r0vm 3.0.5 ([`cli/src/zk.rs`](../cli/src/zk.rs)).
First a succinct STARK receipt (`prove_with_opts(.., ProverOpts::succinct())`).
Then `compress(.., ProverOpts::groth16())`, whose last step (`shrink_wrap`)
runs RISC Zero's prover image `risczero/risc0-groth16-prover:v2025-04-03.1`.
That image is x86_64 only. On this Apple Silicon machine it runs under Docker
Desktop's x86_64 emulation, selected with `DOCKER_DEFAULT_PLATFORM=linux/amd64`.
The container sees no AVX2 and no ADX. The client verifies the receipt
locally, checks that its journal is the expected preimage, and prints the
journal, the seal and the timings. It prints no opening. The seal it prints
is the one `RiscZeroGroth16Verifier` takes: the 4-byte selector `73c457ba`,
then the 256-byte proof. The verifier parameters r0vm reports (control root
`a54dc85a…`, BN254 control id `c07a6514…`) are the ones in risc0-ethereum
v3.0.1's `ControlID.sol`.

**The verifier.** [`ZkBurnVerifier`](../vault/src/ZkBurnVerifier.sol)
decodes `(seal, journal, lezBlock, lezFinalBlock, signatures)` and checks
three things:

1. `sha256(journal)` equals the digest the vault passes. If not, the error is
   `NotThisRelease`. A proof made for another vault, chain, pairing,
   recipient, relayer, amount, fee, root or nullifier stops here.
2. `RISC0.verify(seal, IMAGE_ID, digest)` succeeds. `RISC0` is RISC Zero's own
   `RiscZeroGroth16Verifier` from risc0-ethereum v3.0.1, vendored unchanged
   under `vault/lib/risc0-ethereum` with its `ControlID`. `IMAGE_ID` is fixed
   at construction. If the check fails, the error is RISC Zero's
   `VerificationFailed` (a journal edited to match edited arguments stops
   here), or `SelectorMismatch` for a seal from another verifier version.
3. The journal's root (word 9) is final and attested by the committee, under
   the journal's pairing (word 3). This is part 2 of section 2, unchanged.

The relayer is in the journal. A proof copied from the mempool cannot be
resubmitted by someone else to collect its fee: that release is a different
digest. So the prover must know who will submit. In the local run alice
submits her own release, and the relayer submits carol's.

### What the zero-knowledge release reveals, and what it does not

The release's calldata is the release's arguments plus `(seal, journal,
attestation)`. The journal is the arguments again, with the chain, the vault
and the pairing.

- **Hidden:** the burn leaf, its index, its path, the secret and the Ethereum
  commitment. The run checks each against the calldata (`# eth-zk` lines), and
  the verifier checks the leaf and every path node again from the chain. So
  the release does not say which LEZ burn transaction it pays out.
- **Public:** the token, the amount, the recipient, the relayer, the fee, the
  root and the nullifier. The nullifier is `sha256(tag ‖ pairing ‖ secret ‖
  index)`, unlinkable to the leaf without the secret.
- **The anonymity set** is the burns of the same token and amount under the
  named root. The bridge burns only fixed denominations, and a release can
  name any root the committee attested at or after the burn. In the local run
  the burn tree holds two burns of 1 ETH, alice's and carol's. Each release
  is one of two, and each one's path node is the other's leaf.
- **What it cannot hide:** the recipient and the fee are an Ethereum payment.
  The root dates the burn to before that root. A recipient who also deposited
  on Ethereum, or a fee that is unusual for its denomination, can narrow the
  set. The client should default to a recent root and a fee schedule per
  denomination.

### Measured (2026-10-19, Apple M4 Pro, 14 cores, 24 GiB; Docker Desktop 29.4, VM 14 CPUs, 11.7 GiB)

| | fixture | alice (e2e) | carol (e2e) | a forged burn (e2e) |
|---|---:|---:|---:|---:|
| STARK, succinct (`prove_with_opts`) | 19.2 s | 20.3 s | 21.3 s | 20.7 s |
| Groth16 (`compress`, x86_64 image emulated) | 52.9 s | 52.4 s | 51.9 s | 55.6 s |
| whole command, wall | 72.6 s | 72.7 s | 73.3 s | 76.3 s |
| peak RSS on the host (wrap + r0vm, `time -l`) | 2.50 GB | 2.57 GB | 2.54 GB | 2.47 GB |
| peak memory of the Groth16 container (`docker stats`, sampled about every 2 s) | 7.09 GiB | 7.56 GiB | 7.62 GiB | 7.37 GiB |
| seal | 260 B | 260 B | 260 B | 260 B |

The guest runs 165,109 user cycles: one 2^18 segment of 262,144 cycles. Two
earlier fixture proofs of the first guest build took 19.1 and 18.7 s, then 44.9
and 49.5 s. The load average was 30 to 40 during the e2e proofs, because other
work was running on the machine. The heavy lock keeps a second prover out, not
that work. The container's peak is a sampled lower bound. The Docker VM's
11.7 GiB was enough, so its memory was not raised. Raw output:
[`docs/raw/zk-prove-fixture-20261019T0923Z.txt`](raw/zk-prove-fixture-20261019T0923Z.txt)
and `evidence/local/zk-prove-{alice,carol,forged}.txt`.

Gas on anvil (gas used, including the 21,000 base and about 1,540 bytes of
calldata):

| release (zero knowledge) | expected | gas used |
|---|---|---:|
| `release-carol-not-final` | `NotFinal` | 285,901 |
| `release-alice` (self-submitted, fee 0) | applied | **334,215** |
| `release-alice-replay` | `AlreadyReleased` | 50,456 |
| `release-carol-redirected-to-relayer` | `NotThisRelease` | 58,455 |
| `release-carol-another-amount` | `NotThisRelease` | 58,455 |
| `release-carol-front-run` (an outsider names itself relayer) | `NotThisRelease` | 58,455 |
| `release-carol-tampered-journal` (the recipient changed in the arguments and the journal) | `VerificationFailed` | 285,636 |
| `release-forged-burn-committee` (a real proof of a made-up leaf in a made-up tree; the members sign what they read) | `RootNotAttested` | 298,014 |
| `release-forged-burn-outsiders` (the same, its root signed by two outsiders) | `RootNotAttested` | 298,002 |
| `release-carol-one-member` | `RootNotAttested` | 290,569 |
| `release-carol` (by the relayer, fee 0.01 ETH) | applied | **343,550** |

RISC Zero's `verify` alone costs 253,318 gas of execution (Foundry,
`test_the_groth16_check_alone`). A zero-knowledge release costs about 191,000
gas more than one that opens its leaf (143,096 and 152,400 in section 2).
An invalid seal (a point off the curve) makes the pairing precompile fail,
and that burns all the gas forwarded to it. A relayer pre-checks with
`eth_call` before it sends.

### The trust assumption, stated plainly

Part 1 is cryptographic in both verifiers. In `ZkBurnVerifier` it rests on
RISC Zero's proof system: the STARK, its recursion circuits (pinned by the
control root) and the Groth16 wrap, whose circuit-specific setup is RISC
Zero's ceremony. Part 2 is not cryptographic in either verifier. If
`THRESHOLD` members sign a root the burn tree never had, the vault pays out
against burns that never happened. With `ZkBurnVerifier` they would also
have to prove membership in a tree of their own making, which is easy: the
run does exactly that (`release-forged-burn-*`), and only the committee
check refuses it. That is a federation, which F.7 ("natively verified on
Ethereum by cryptographic proof") excludes. **F.7 is not met.** No better
source for the root exists today. The vault side of option A is in place,
and so is the zero-knowledge membership. What option A puts in place of the
committee is not built.

The committee is there because nothing on Ethereum can check LEZ state
today:

- LEZ v0.3 block headers have no state root, and since v0.3.1 no producer
  signature.
- There is no Cryptarchia (Bedrock) light client for Ethereum.
- There is no LEZ validity prover.

The committee stands in for all of it: the LEZ block's finality on Bedrock,
the burn's inclusion, and the burn tree's root in the state that finality
covers. The root cannot be derived instead. It is LEZ state, and Ethereum
has no commitment to LEZ state to derive it from.

In the local run, "final" is the standalone sequencer's tip, `FINALITY_DEPTH`
blocks above the block the member read. The standalone sequencer posts
nothing to Bedrock.

The privacy cost of section 2: that release publishes its opening, so it is
linkable to the LEZ transaction that appended its leaf. After a public burn,
that transaction names the burner's LEZ account. After a private burn it
names no account, but its block, its token and its amount are public.
Section 3 removes that link. What it still reveals is listed there.

### The path to F.7

Each step is a new verifier behind the same `IBurnVerifier` interface, so
each needs a new vault (Security #8).

1. **Done (locally):** part 1 as a RISC Zero guest, with its receipt checked
   on chain by RISC Zero's Groth16 verifier (section 3). The Groth16 wrap
   runs emulated on this machine, in about 52 s.
2. **Done (locally):** the root derived from LEZ blocks inside the proof
   (section 4). The committee attests a block hash, not a root.
3. The tip's Bedrock inclusion (the inscription in a Bedrock block) proven
   too, then its finality: a Cryptarchia light client, which must replay
   Bedrock's note ledger (docs/BEDROCK-FINALITY.md, sections 3 and 5).
4. A LEZ state commitment or validity prover, for the blocks' validity.

### Tested

Foundry, `vault/test/ZkBurnVerifier.t.sol`, 27 tests on a **real Groth16
seal**. The seal is in `vault/test/fixtures/zk_release.json`, made by
`scripts/zk-fixture.sh` with the prover above, for the test's burn tree, with
the vault placed at the address its journal names. Each case is refused with
its typed error:

- the fixture is the test tree's release, recomputed in Solidity: root,
  nullifier, preimage, selector, 260 bytes;
- the release calldata carries no leaf, secret, Ethereum commitment or path
  node;
- the valid release, paid once to the committed recipient and fee, and its
  replay (`AlreadyReleased`);
- another recipient, amount, fee, relayer, root, nullifier, chain or vault
  (`NotThisRelease`);
- the journal tampered to match another recipient, amount, fee, root or
  nullifier (`VerificationFailed`, from RISC Zero's verifier);
- a seal with a flipped byte (`VerificationFailed`), a seal with another
  selector (`SelectorMismatch`), and a verifier pinned to another image id;
- a root the committee did not sign, not final, one member, outsiders, and
  another pairing (`NotFinal`, `RootNotAttested`);
- a misconfigured verifier (`BadConfig`);
- the gas of the Groth16 check alone.

Foundry, `vault/test/CommitteeBurnVerifier.t.sol` (18 tests), each case
refused with its typed error:

- the valid release, paid once to the committed recipient and fee;
- a replay (`AlreadyReleased`), and a second nullifier for the same burn;
- another recipient, another amount, a higher fee, another relayer, another
  index or path;
- a burn not yet final by the configured depth, and a final block changed
  after signing (`NotFinal`, `RootNotAttested`);
- a burn that never happened: outsiders signing, one member alone, one member
  plus an outsider;
- a member signing three times, and a malleated (high-s) signature;
- an attestation for another vault, chain or pairing;
- a direct call that does not come from a vault;
- a malformed committee;
- the hashes shared with Rust.

`scripts/mutation-test-vault.sh` removes or weakens each of the 22 checks of
the release path in turn, and the tests catch all 22
([`docs/raw/mutation-vault-2026-10-19.txt`](raw/mutation-vault-2026-10-19.txt)):

- the 4 of `CommitteeBurnVerifier`'s opening;
- the 7 of the shared committee (finality, threshold, non-member, a member
  twice, high-s, chain and vault binding, pairing);
- the 6 of `ZkBurnVerifier`: the journal not compared, the seal not
  verified, the seal checked against a fixed digest, the committee not
  consulted, the attested root not the journal's, and the attestation not
  bound to the journal's pairing;
- the 5 of the vault: its digest not bound to the chain, the vault or the
  relayer, and a spent nullifier not refused or not recorded.

The first version of the mutation test, for the committee verifier alone,
found one check that no test covered (a non-member's signature counting). The
test for it was then added.

On anvil, through the local run of 2026-10-17 with the committee verifier
(gas used, including the 21,000 base). The 2026-10-19 rerun with
`RELEASE_VERIFIER=committee`, after the committee moved into
`BurnRootCommittee`, gives the same nine verdicts with gas within 52 of these
(143,044 and 152,420 for the two applied). Its verifier passes 73 checks with
0 failing, and its mutation test catches 28 of 28
([`evidence/local-committee/`](../evidence/local-committee/)).

| release | expected | gas used |
|---|---|---:|
| `release-carol-not-final` (signed at the tip, depth 0 < 3) | `NotFinal` | 94,684 |
| `release-alice` (self-submitted, fee 0) | applied | 143,096 |
| `release-alice-replay` | `AlreadyReleased` | 56,008 |
| `release-carol-redirected-to-relayer` | `RootNotAttested` | 107,035 |
| `release-carol-another-amount` (0.1 ETH) | `RootNotAttested` | 107,015 |
| `release-forged-burn-committee` (a made-up leaf; the members sign what they read) | `RootNotAttested` | 106,502 |
| `release-forged-burn-outsiders` (a made-up leaf, signed by two outsiders) | `RootNotAttested` | 106,522 |
| `release-carol-one-member` | `RootNotAttested` | 99,480 |
| `release-carol` (by a relayer, fee 0.01 ETH) | applied | 152,400 |

The vault paid out exactly 2 ETH. Carol received 0.99 ETH. Both burn
nullifiers are recorded as released.

## 4. The chain-derived release: `ZkChainBurnVerifier`

The committee's root is replaced by one the proof derives. The full account
is in [BEDROCK-FINALITY.md](BEDROCK-FINALITY.md), sections 5 to 7. In short:

- **The chain guest.** `zk/lez_chain`, with its statement in the
  [`chain/`](../chain/) crate, reads LEZ v0.3.1 blocks as the sequencer
  inscribes them on Bedrock. It goes from an anchor block pinned at
  deployment, or continues an earlier receipt of itself verified inside.
  For each block it checks the hash against the contents and the link to
  the previous block.
- **The tree it rebuilds.** It holds the bridge's burn leaves that
  **privacy-preserving** transactions settled. On v0.3.1 such a transaction
  is fee-exempt, and one that fails settlement invalidates its whole block.
  A public burn is left out: a public transaction whose action fails stays
  in its block, and the block does not say so. The run's block 84 is one:
  alice's refused burn, included.
- **The release guest.** `zk/burn_release` verifies the chain receipt
  inside and proves the burn's opening under the chain's root. Ethereum
  checks one Groth16 seal for both guests.
- **What the vault's verifier checks.** `ZkChainBurnVerifier` checks the
  preimage against the vault's digest, then that the journal names the
  pinned chain guest, program, burn-tree account and anchor. Then the seal,
  then that `LezBlockCommittee` attests the tip block hash final.
- **The run.** `scripts/e2e-chain.sh` on a kept `e2e-local.sh` run
  (`evidence/local-chain/`) passed: 12 releases, 1 applied, 11 refused as
  expected. Among the refusals is a real proof over a made-up LEZ block,
  refused only because the members sign the real block at that height.
  Carol's release cost 347,250 gas. The chain proofs took 87 s and 141 s
  for 93 and 197 blocks; the release took 39 s of STARK and 48 s of
  emulated Groth16.

The trust that remains, plainly:

- **Finality.** The tip's finality on Bedrock is the committee's word,
  about a public block hash.
- **Validity.** The blocks' validity rests on LEZ's sequencer and nodes.
- **Public burns.** A public burn cannot release this way. It needs the
  other verifiers, or a burn made as a privacy-preserving transaction.

F.7 is still not met as the RFP words it.


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
  (Security #8), each nullifier once.
- `core/`: `wrap-core`, every hash both halves must agree on (leaf, tree,
  commitments, nullifiers, the `knownRoot` slot, the burn tree, the release
  digest), with shared test vectors asserted on both sides.
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
  recipient to the program's burn tree. `Register`, `SetPolicy`, `Freeze`.
- `executor-tests/`: the four committed binaries (`sextant_lc` from Sextant,
  `token` and `associated_token_account` from LEZ v0.3.0, `wrap_bridge`)
  through `lee::V03State`, the state machine the v0.3 sequencer applies, under
  the 10,000,000 gas cap, on deposits made on anvil; and the claim as a
  privacy-preserving transaction to a private account, proven by LEZ's privacy
  circuit and settled through the same checks the sequencer runs.
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
forge test --root vault                                            # 21 tests
cargo test --release --manifest-path core/Cargo.toml               # 7 tests
cargo test --release --manifest-path executor-tests/Cargo.toml     # 12 tests (r0vm 3.0.5)
RISC0_DEV_MODE=1 cargo test --release --manifest-path executor-tests/Cargo.toml --test private -- --ignored   # 2 tests
scripts/measure-private-claim.sh       # the private claim with real proving (~7 GB, minutes)
cargo run --release --manifest-path executor-tests/Cargo.toml --bin measure
DOCKER=1 scripts/build-guest.sh        # rebuild artifacts/programs/v0.3/wrap_bridge.bin (reproducible)
scripts/mutation-test.sh               # each guard removed in turn must fail the tests
python3 scripts/anvil_vectors.py       # regenerate vectors/anvil (local anvil, nothing spent)
```

`artifacts/programs/v0.3/wrap_bridge.bin` is the `DOCKER=1` build, in RISC
Zero's pinned builder (`risczero/risc0-guest-builder:r0.1.91.1`): 434,496
bytes, sha256 `973aec2aceed4caa0094b06067fea0883e38b20e0bec5c9b9f12f02a42967008`,
ImageID `a7c903ab8c53cae2fc74928a569c772ef45520f323de2e1c3be43dd04d82e3d7`.
CI rebuilds it from a clean checkout and `cmp`s it with the committed bytes
(`.github/workflows/ci.yml`, job `reproduce`). The local build (no `DOCKER`)
is for iterating and for the mutation test; its bytes differ.

## Measured (docs/raw/)

| | gas |
|---|---:|
| LEZ `Claim` (path, chained `Attest`, chained token `Mint`) | 297,502 (2.98% of the cap) |
| of which Sextant's `Attest` of the storage slot | 111,122 |
| LEZ `Burn` (burn-tree append, chained ATA `Burn`) | 228,690 |
| LEZ `Register` (entry, chained `NewFungibleDefinition`) | 90,577 |
| Ethereum `deposit` (median, max) | 257,854, 336,694 |
| Ethereum `release` with a stand-in verifier (max) | 158,669 |

The anvil account proof has 3 nodes; a mainnet one has 8 to 9, which Sextant
measured at 304,267 gas for a storage `Attest`: a mainnet claim is about
490,000 gas, still under 5% of the cap.

## What this prototype is not yet

Public transactions only: a public claim carries the deposit's opening, so it
links the claim to its deposit. The unlinkable path is the same plan inside a
privacy-preserving transaction (destination a private account, the opening and
path private inputs), not yet run. The vault's release verifier is a stand-in:
nothing on Ethereum verifies LEZ state yet (LEZ v0.3 block headers carry no
state root). No protocol fee, no rolling-window cap on LEZ (an outstanding-
supply cap instead), no relayer, no Logos modules, no SPEL IDL (SPEL does not
build v0.3 programs), no deployment on any network.

MIT OR Apache-2.0.

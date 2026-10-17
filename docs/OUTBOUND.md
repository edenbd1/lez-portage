# Outbound: the private burn and the release (F.6, F.7)

A wrapped token goes back to Ethereum in two steps. On LEZ, the holder burns
it and the bridge appends a burn leaf to its burn tree. On Ethereum, the vault
releases the escrowed asset against that leaf. This document covers what is
built for each step, what it was measured at, and what the release path rests
on. Record of the run: [`evidence/local/`](../evidence/local/); how it runs:
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

## 2. The release (F.7): what the vault checks

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

**Part 2: the root, attested by a committee.** A fixed committee of `n`
members signs, under EIP-191:

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

### The trust assumption, stated plainly

Part 1 is cryptographic. Part 2 is not. If `THRESHOLD` members sign a root
the burn tree never had, the vault pays out against burns that never
happened. That is a federation, which F.7 ("natively verified on Ethereum by
cryptographic proof") excludes. **F.7 is not met.** The vault side of option A
is in place; what option A puts in place of the committee is not built.

The committee is there because nothing on Ethereum can check LEZ state
today:

- LEZ v0.3 block headers have no state root, and since v0.3.1 no producer
  signature.
- There is no Cryptarchia (Bedrock) light client for Ethereum.
- There is no LEZ validity prover.

The committee stands in for all of it: the LEZ block's finality on Bedrock,
the burn's inclusion, and the burn tree's root in the state that finality
covers.

In the local run, "final" is the standalone sequencer's tip, `FINALITY_DEPTH`
blocks above the block the member read. The standalone sequencer posts
nothing to Bedrock.

The privacy cost: the release publishes its opening, so it is linkable to the
LEZ transaction that appended its leaf. After a public burn, that transaction
names the burner's LEZ account. After a private burn it names no account,
but its block, its token and its amount are public. PP.2 needs part 1 as a
zero-knowledge proof, which is not built.

### The path to F.7

Each step is a new verifier behind the same `IBurnVerifier` interface, so
each needs a new vault (Security #8).

1. Part 1 as a RISC Zero guest, with its receipt checked by RISC Zero's
   Groth16 verifier. This hides the leaf. RISC Zero's Groth16 wrap runs in an
   x86 Docker image, which this Apple Silicon machine does not run natively.
   It was not attempted here.
2. Part 2 as a Cryptarchia light client in RISC Zero, proving the LEZ block's
   finality on Bedrock and the burn's inclusion (option A). Trust would remain
   in one place: the burn tree's validity would rest on LEZ's sequencer and
   nodes.
3. A LEZ state commitment (option C), once LEZ has one.

### Tested

Foundry (`vault/test/CommitteeBurnVerifier.t.sol`, 18 tests), each case
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

`scripts/mutation-test-vault.sh` removes each of the 13 checks of the release
path in turn (the 11 of the verifier, and the vault's nullifier check and
record). The tests catch all 13. The run first found one check that no test
covered, a non-member's signature counting; the test for it was then added.

On anvil, through the local run (gas used, including the 21,000 base):

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

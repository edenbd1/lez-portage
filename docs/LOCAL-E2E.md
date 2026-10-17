# The bridge on a local LEZ v0.3.1 sequencer, end to end

[`scripts/e2e-local.sh`](../scripts/e2e-local.sh) runs the round trip through
the client (`cli/`, `wrap`) against a **local** LEZ v0.3.1 standalone
sequencer and a **local** anvil chain: the inbound leg (deposits, public and
private claims), the burns (public, and private from a private account), and
the releases back on Ethereum, with their refusals. Nothing reaches a
public network and nothing of value is spent: the LEZ fees are paid by a
genesis test account of LEZ's debug configuration, the Ethereum side uses
anvil's development keys, and both chains are discarded at the end.

Record of the run: [`evidence/local/`](../evidence/local/)
(`e2e.tsv` the manifest, `transcript.txt`, `verify.txt`, `mutation-test.txt`,
`anvil/` the vault deployment and deposits).

## What runs

1. **LEZ: the programs.** The four committed binaries are loaded through
   program_loader by the client: Sextant's `sextant_lc.bin` (lez-sextant
   `main`, ImageID `fa229edd…`), LEZ v0.3.0's `token.bin` (`912d412c…`) and
   `associated_token_account.bin` (`2b1d5580…`), and `wrap_bridge.bin`, the
   committed Docker build (`073f046c…`), the last one with an immutable
   header. They are loaded before the vault exists because the vault is
   deployed with the pairing, which names wrap_bridge's program id.
2. **Ethereum (anvil): the deposits.** `WrapVault`, its release verifier
   (`CommitteeBurnVerifier`: anvil accounts 5, 6, 7 as a 2-of-3 committee,
   finality depth 3 LEZ blocks) and a USDT-shaped token (transfers return
   nothing) are deployed with that pairing, both tokens are registered, and
   four deposits are made
   ([`scripts/anvil_deposits.py`](../scripts/anvil_deposits.py)):
   alice 1 ETH and 100 USDT, bob 0.1 ETH, all to public LEZ accounts, and
   carol 1 ETH committed to her **private** LEZ account, created in her wallet
   before the deposit. Then the block, the `Deposit` events and an
   `eth_getProof` of `knownRoot[root]` for every root.
3. **LEZ: Sextant, the bridge, the claims, the burns.**

| step | expected |
|---|---|
| `Init` the Sextant instance from a beacon header built around the anvil block | applied |
| `Init` the bridge configuration; `Register` wrapped ETH and wrapped USDT (admin-signed) | applied |
| claim alice's 1 ETH (public, to her ATA) | applied |
| the same claim again, and again under the root right after the deposit | refused `E7402` |
| claim alice's 100 USDT | applied |
| bob's deposit opened with alice as destination (a relayer redirecting) | refused `E7403` |
| a deposit never made, under a root the vault never had (a genuine proof of `knownRoot = 0`) | refused by Sextant's `Attest`, `E7208` |
| **carol's 1 ETH as a privacy-preserving claim into her shielded account, real proving** | applied |
| carol's wallet syncs and decrypts her account: 1 wrapped ETH, its commitment in the chain's set | |
| alice burns 0.3 ETH (not a permitted amount) | refused `E7408` |
| alice burns 1 ETH (Ethereum recipient and fee 0 hidden in a commitment) | applied |
| carol burns 0.3 ETH privately (not a permitted amount) | refused by the client before proving `E7408` |
| **carol burns 1 ETH from her private account (`BurnFrom`, privacy-preserving, real proving; fee 0.01 ETH)** | applied |
| carol's wallet syncs and decrypts her account: 0 wrapped ETH | |

4. **Ethereum (anvil): the releases** ([OUTBOUND.md](OUTBOUND.md)). The client
   rebuilds the burn tree from its record and checks it against the chain's
   frontier (`wrap release-proof`). Each committee member reads the burn
   tree's root from the sequencer and signs the root it read
   (`scripts/anvil_release.py`). The release is then sent with an explicit
   gas limit, so a refused one is mined and recorded too.

| release | expected |
|---|---|
| carol's, signed at the tip (0 blocks deep, 3 required) | refused `NotFinal` |
| alice's, self-submitted | applied |
| alice's again | refused `AlreadyReleased` |
| carol's opening, redirected to the relayer | refused `RootNotAttested` |
| carol's opening, with another amount | refused `RootNotAttested` |
| a burn that never happened, the members signing what they read | refused `RootNotAttested` |
| the same, signed by two outsiders | refused `RootNotAttested` |
| carol's, signed by one member | refused `RootNotAttested` |
| carol's, by a relayer who earns her 0.01 ETH fee | applied |

**Caveat on the release, stated in the manifest:** the committee is the
trust assumption of the release path (a federation; F.7 is not met, see
[OUTBOUND.md](OUTBOUND.md)), and "final" is the standalone sequencer's tip,
three blocks above the block a member read: there is no Bedrock in this run.

**Caveat, stated in the run's output and manifest:** anvil has no beacon
chain. The Sextant instance is initialised from a beacon header built around
the anvil block, exactly as the executor tests do (`executor-tests/src/lib.rs`,
`devnet`): that header is the trusted checkpoint, the one input a light client
takes on trust, and here nothing signs it. Everything after it is verified as
on a real network: the vault's `knownRoot[root]` storage proof against the
block's state root (Sextant's `Attest`, chained from the claim), the deposit's
Merkle path, the nullifier, the mint into the committed destination.

## How a verdict is read

Every public transaction is pre-checked by the client through `lee::V03State`
with the same four binaries and the accounts as the chain holds them, sent
with an explicit gas limit (its measured gas plus 1% and 10,000), signed by
its signers (the admin for `Register`, the owner for `Burn`) and the fee payer.
On LEZ v0.3 a refused public transaction is still included, changes nothing,
and is charged its whole gas limit; refusals are sent with the gas limit of
the honest instruction of their kind, so running out of gas cannot explain
them. The verdict is read back from the chain: the accounts the instruction
touches, and the gas the block's fee transaction records.

The private claim has no signer and pays no fee. It is built and proven by
the v0.3.0 wallet crate (`send_privacy_preserving_tx`, with the bridge's plan,
Sextant's and the token program's, composed by LEZ's privacy circuit), and
submitted to the sequencer, which runs the checks `docs/PRIVATE-CLAIM-MEASURED.md`
describes at the executor level (the receipt, the program image claims
against the deployed headers, commitments new, nullifiers unspent, then the
deferred public effects, Sextant's storage-proof check among them). Its
verdict is the deposit's nullifier account, written once.

## Results

Run of 2026-10-17 16:33Z at commit `c6328fe`, Apple M4 Pro (14 cores,
24 GiB), sequencer `1a003bfb` (standalone), wallet `db66590a`, r0vm 3.0.5.
**PASS: 14 LEZ transactions, 9 applied and 5 refused as expected; 9 releases
on anvil, 2 applied and 7 refused as expected; 27 state checks ok.** Before
them, 25 program_loader transactions loaded the four programs (each header
names its ImageID).

| transaction | expected | verdict | block | gas (charged) |
|---|---|---|---:|---:|
| `init-sextant` | applied | applied | 27 | 1,831,160 |
| `init-bridge` | applied | applied | 28 | 27,049 |
| `register-eth` (admin-signed) | applied | applied | 29 | 90,958 |
| `register-usdt` (admin-signed) | applied | applied | 30 | 90,316 |
| `claim-alice-eth` | applied | applied | 31 | 287,283 |
| `claim-alice-eth-replay` | refused `E7402` | reverted | 32 | 330,000 (its limit) |
| `claim-alice-eth-replay-old-root` | refused `E7402` | reverted | 33 | 330,000 (its limit) |
| `claim-alice-usdt` | applied | applied | 34 | 287,355 |
| `claim-bob-redirected-to-alice` | refused `E7403` | reverted | 35 | 330,000 (its limit) |
| `claim-phantom-unknown-root` | refused `E7208` (Sextant) | reverted | 36 | 330,000 (its limit) |
| `claim-carol-private` | applied | applied (nullifier written) | 86 | none: no fee, no signer |
| `burn-alice-eth-not-a-denomination` | refused `E7408` | reverted | 87 | 255,000 (its limit) |
| `burn-alice-eth` (owner-signed) | applied | applied | 88 | 228,729 |
| `burn-carol-private` | applied | applied (leaf appended) | 129 | none: no fee, no signer |

Every applied public transaction was charged exactly the gas the client's
pre-check measured. Every refusal was charged exactly its gas limit, and
nothing changed.

State checks along the way:

- alice holds 1 wrapped ETH, then 100 wrapped USDT;
- the supply of wrapped ETH is 1 ETH after the public claims, 2 after the
  private claim, 1 after alice's burn and 0 after carol's;
- bob's deposit stays unclaimed (its nullifier account is empty);
- the vault paid out exactly 2 ETH, and carol received 0.99 ETH;
- both burn nullifiers are recorded as released.

### The private transactions, through the sequencer

| | private claim | private burn |
|---|---:|---:|
| prove and submit (`send_privacy_preserving_tx`, real proving) | **247.6 s** | **202.9 s** |
| the client process, wall | 249.7 s | 204.9 s |
| peak resident memory (`/usr/bin/time -l`) | **6.17 GB** | **7.22 GB** |
| transaction on chain (`getTransaction`, with its variant tag) | **251,335 bytes** | **248,358 bytes** |
| load average before | 4.9 | 34 |
| RISC Zero dev mode | off | off |

The heavy lock (`$HEAVY`) kept a second prover out during
both. The machine was busy with other work during the burn, so its time is an
upper bound for an idle machine. Each was measured once in this run. The
previous run measured the claim at 263 s and 6.87 GB, and the executor-level
measurement gave 252 to 254 s and 6.7 to 8.3 GB
([PRIVATE-CLAIM-MEASURED.md](PRIVATE-CLAIM-MEASURED.md)). The wallet pads
every privacy-preserving transaction to seven private inputs and every note to
512 bytes, so the two transactions are the same order of size whatever they
do.

What the client checked in each transaction's bytes as the chain returns
them:

- **The claim.** The destination's account id, the secret, the deposit's
  commitment and leaf, and both non-empty nodes of its Merkle path occur
  nowhere.
- **The burn.** Carol's account id, her Ethereum recipient and the burn secret
  occur nowhere. The burn leaf does occur in it, and the chain's burn tree
  appended it.

The verifier re-checks, for each transaction, that carol's account id is
absent from the chain's copy.

**Decryption.** The client's wallet (`WalletCore::sync_to_latest_block`)
found and decrypted carol's notes. After the claim she holds 1 wrapped ETH of
the definition the bridge registered, and the note's commitment is in the
chain's set. The v0.3.0 `wallet` binary shows the same record
(`wallet account get --account-id Private/… --scope <token header>`). After
the burn she holds 0. Nothing about the account is public: the chain has no
shard under its id.

### The releases, on anvil

See the table in [OUTBOUND.md](OUTBOUND.md): the applied releases used
143,096 gas (alice, fee 0) and 152,400 gas (carol, through a relayer), and
the refusals used 56,008 to 107,035 gas.

## The verifier and its mutation test

[`scripts/verify-onchain.sh`](../scripts/verify-onchain.sh) re-checks a
manifest against the sequencer and anvil with nothing but JSON-RPC: every program
header names its ImageID; every deployment transaction resolves; every
transaction resolves by hash (`getTransaction`) to its recorded block, and
the hash is SHA-256 of the returned bytes after the variant tag; a public one
was charged as its verdict says (applied: the pre-checked gas, below the
limit; refused: exactly the limit; it must be the only charged transaction of
its block); each private one is a privacy-preserving transaction of its
recorded size in which carol's account id does not occur; every account the
run wrote (the Sextant instance and header record, the bridge config, both
entries, the burn tree, the four nullifier accounts, both definitions,
alice's two ATAs) holds the bytes the run left; and a random hash does not
resolve.

On anvil (`--eth-rpc`), every release attempt is checked:

- its receipt is in the recorded block, with the recorded gas, as a call to
  the vault;
- an applied one has a `Release` event of the vault that names the recorded
  nullifier, under the root the run recorded;
- a refused one, replayed with `eth_call` at the block before, reverts with
  the recorded selector, which is the expected error's.

Then:

- the root the releases were checked against is the LEZ burn tree's root on
  the sequencer now;
- both burn nullifiers are spent and a random one is not;
- the vault's final balance is the one recorded;
- a random transaction hash does not resolve on anvil.

[`scripts/mutation-test-verifier.sh`](../scripts/mutation-test-verifier.sh)
corrupts one thing at a time in a copy of the manifest and requires the
verifier to fail on each with a FAIL line.

Against this run ([`mutation-test.txt`](../evidence/local/mutation-test.txt)),
the manifest as recorded verifies (73 checks, 0 fail), and **all 28 mutations
are caught**. The 15 on the inbound side: an applied transaction's hash replaced by one never
sent; a refusal claimed as applied; an application claimed as refused; the
pre-checked gas off by one; a transaction recorded in the next block; two
hashes swapped; the private claim's hash replaced; the private claim recorded
as public; a public transaction recorded as the private claim; the private
destination replaced by an id the transaction does carry (the bridge config);
the private claim's size off by one; a nullifier account's final bytes
altered; the unclaimed deposit's nullifier recorded as spent; another ImageID
for wrap_bridge; a deployment hash never sent. The 13 added with the burn and
release legs: the private burn's hash replaced; the private burn's hidden
account replaced by one it carries; its size off by one; an applied release's
hash replaced; a refused release recorded as applied, and the reverse; a
refusal recorded with another error than the one the chain gives; a release
recorded in another block; an applied release's nullifier altered; the
released root not the LEZ burn tree's; a released nullifier recorded as
unspent; a random nullifier recorded as spent; the vault's balance off by one
wei.

The local chain is discarded when the run ends, so the verifier and the
mutation test run inside the script against the live sequencer
([`verify.txt`](../evidence/local/verify.txt)); `KEEP=1` leaves it up to
re-run them by hand. Unlike a testnet manifest, this one cannot be re-checked
later from a clean clone: a new run makes a new chain.

## Run it

```console
# LEZ at db66590a (v0.3.0): the wallet; at 1a003bfb (v0.3.1): the standalone sequencer
cargo build --release -p wallet                                   # at db66590a
cargo build --release --features standalone -p sequencer_service  # at 1a003bfb
cargo build --release --manifest-path cli/Cargo.toml
SEQUENCER=<lez@1a003bfb>/target/release/sequencer_service \
WALLET=<lez@db66590a>/target/release/wallet \
LEZ_SRC=<lez@1a003bfb> scripts/e2e-local.sh 2>&1 | tee evidence/local/transcript.txt
```

Ports 3051 (sequencer), 9471 (its metrics) and 8551 (anvil) by default
(`PORT`, `METRICS_PORT`, `ANVIL_PORT`); the script refuses to start if one is
taken, and stops only the two processes it started (`KEEP=1` leaves them up).
Blocks every 5 s (`BLOCK_TIME`). The private claim runs under
`$HEAVY` when set (one prover at a time on the
machine) and `/usr/bin/time` for its peak memory.

## Limits

- One sequencer, no Bedrock: the standalone sequencer does not post blocks to
  Bedrock, so finality on Bedrock is not part of this run.
- The checkpoint caveat above: on Sepolia the checkpoint is a finalized beacon
  block root and Sextant follows the sync committee from there (Sextant v2 for
  Gloas); that run needs the vault on Sepolia.
- The release verifier is a 2-of-3 committee over the burn tree's root
  ([OUTBOUND.md](OUTBOUND.md)): a stated trust assumption, not the
  cryptographic F.7. The release opens its burn leaf, so it is linkable to
  the LEZ burn transaction.
- The v0.3.0 `wallet` binary cannot sync against v0.3.1 blocks itself
  (`wallet account sync-private` fails decoding the header: v0.3.1 dropped the
  producer key and signature). The sync is the wallet crate's, in the client,
  built against LEZ's v0.3.1 `common` (`cli/vendor/lez-common`); the binary
  then reads the decrypted account from the same wallet home.
- The run is one sequential client: nothing else touched the bridge's
  accounts while the private claim or burn was proven. Concurrent claims or
  burns of the same token during a proof were not exercised.

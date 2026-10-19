# The bridge on a local LEZ v0.3.1 sequencer, end to end

[`scripts/e2e-local.sh`](../scripts/e2e-local.sh) runs the round trip through
the client (`cli/`, `wrap`) against a **local** LEZ v0.3.1 standalone
sequencer and a **local** anvil chain: the inbound leg (deposits, public and
private claims), the burns (public, and private from a private account), and
the releases back on Ethereum, with their refusals. Each release carries a
real Groth16 proof of the burn's membership, checked on chain by RISC Zero's
verifier (`RELEASE_VERIFIER=zk`, the default). With `RELEASE_VERIFIER=committee`
the releases open their burn leaf instead. Nothing reaches a
public network and nothing of value is spent: the LEZ fees are paid by a
genesis test account of LEZ's debug configuration, the Ethereum side uses
anvil's development keys, and both chains are discarded at the end.

Record of the run: [`evidence/local/`](../evidence/local/)
(`e2e.tsv` the manifest, `transcript.txt`, `verify.txt`, `mutation-test.txt`,
`anvil/` the vault deployment and deposits, `zk-prove-*.txt` the three
Groth16 proofs measured). A run with `RELEASE_VERIFIER=committee` after the
same changes: [`evidence/local-committee/`](../evidence/local-committee/).

## What runs

1. **LEZ: the programs.** The four committed binaries are loaded through
   program_loader by the client: Sextant's `sextant_lc.bin` (lez-sextant
   `main`, ImageID `fa229edd…`), LEZ v0.3.0's `token.bin` (`912d412c…`) and
   `associated_token_account.bin` (`2b1d5580…`), and `wrap_bridge.bin`, the
   committed Docker build (`073f046c…`), the last one with an immutable
   header. They are loaded before the vault exists because the vault is
   deployed with the pairing, which names wrap_bridge's program id.
2. **Ethereum (anvil): the deposits.** `WrapVault` is deployed with that
   pairing, with its release verifier and a USDT-shaped token (transfers
   return nothing). The verifier is `ZkBurnVerifier`, over RISC Zero's
   `RiscZeroGroth16Verifier` (risc0-ethereum v3.0.1 with its `ControlID`, as
   RISC Zero deploys it) and pinned to the ImageID of the committed guest
   `artifacts/zk/burn_membership.bin` (`855dfc80…`). It takes the root from
   anvil accounts 5, 6 and 7, a 2-of-3 committee, at a finality depth of 3 LEZ
   blocks. With `RELEASE_VERIFIER=committee` the verifier is
   `CommitteeBurnVerifier`, with the same committee. Both tokens are
   registered, and
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

4. **Ethereum (anvil): the releases** ([OUTBOUND.md](OUTBOUND.md), section 3).
   The client rebuilds the burn tree from its record and checks it against
   the chain's frontier. It then proves each release with
   `wrap release-proof OWNER --zk --relayer ADDR`: a real STARK, then the
   Groth16 compression in RISC Zero's x86_64 prover image, emulated on this
   machine. This runs under `$HEAVY` and `/usr/bin/time`,
   with the container's memory sampled
   ([`scripts/zk-prove-measured.sh`](../scripts/zk-prove-measured.sh)). A
   third proof is of a burn that never happened: a leaf paying an outsider,
   alone in a made-up tree, proven by the same guest. Each committee member
   reads the burn tree's root from the sequencer and signs the root it read
   (`scripts/anvil_release.py --zk`). The release carries the seal, the
   journal and the signatures, never the opening. It is sent with an
   explicit gas limit, so a refused one is mined and recorded too.

| release | expected |
|---|---|
| carol's, signed at the tip (0 blocks deep, 3 required) | refused `NotFinal` |
| alice's, self-submitted | applied |
| alice's again | refused `AlreadyReleased` |
| carol's, redirected to the relayer | refused `NotThisRelease` |
| carol's, with another amount | refused `NotThisRelease` |
| carol's proof, submitted by an outsider naming itself relayer (a front-run for her fee) | refused `NotThisRelease` |
| carol's, redirected to the relayer with the journal edited to match | refused `VerificationFailed` (RISC Zero's verifier) |
| a burn that never happened, really proven, the members signing what they read | refused `RootNotAttested` |
| the same, its root signed by two outsiders | refused `RootNotAttested` |
| carol's, signed by one member | refused `RootNotAttested` |
| carol's, by the relayer, who earns her 0.01 ETH fee | applied |

   With `RELEASE_VERIFIER=committee`, the 2026-10-17 table: the release
   opens the leaf, and a redirection, another amount or a made-up leaf leads
   to a root nobody signed (`RootNotAttested`).

**Caveat on the release, stated in the manifest:** the committee is the
trust assumption of the release path (a federation; F.7 is not met, see
[OUTBOUND.md](OUTBOUND.md)), and "final" is the standalone sequencer's tip,
three blocks above the block a member read: there is no Bedrock in this run.
The zero-knowledge proof covers the burn's membership under the root, not the
root itself. The journal (token, amount, recipient, relayer, fee, root,
nullifier) is public.

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

Run of 2026-10-19 09:46Z at commit `f34010b`, with the e2e and verifier
changes committed right after it. Apple M4 Pro (14 cores, 24 GiB), sequencer
`1a003bfb` (standalone), wallet `db66590a`, r0vm 3.0.5, Docker Desktop 29.4
(linux/arm64, the Groth16 image emulated as linux/amd64).
**PASS: 14 LEZ transactions, 9 applied and 5 refused as expected; 11
zero-knowledge releases on anvil, 2 applied and 9 refused as expected; 32
state checks ok.** Before them, 25 program_loader transactions loaded the four
programs (each header names its ImageID).

| transaction | expected | verdict | block | gas (charged) |
|---|---|---|---:|---:|
| `init-sextant` | applied | applied | 27 | 1,831,160 |
| `init-bridge` | applied | applied | 28 | 27,049 |
| `register-eth` (admin-signed) | applied | applied | 29 | 90,958 |
| `register-usdt` (admin-signed) | applied | applied | 30 | 90,316 |
| `claim-alice-eth` | applied | applied | 31 | 287,388 |
| `claim-alice-eth-replay` | refused `E7402` | reverted | 32 | 330,000 (its limit) |
| `claim-alice-eth-replay-old-root` | refused `E7402` | reverted | 33 | 330,000 (its limit) |
| `claim-alice-usdt` | applied | applied | 34 | 287,460 |
| `claim-bob-redirected-to-alice` | refused `E7403` | reverted | 35 | 330,000 (its limit) |
| `claim-phantom-unknown-root` | refused `E7208` (Sextant) | reverted | 36 | 330,000 (its limit) |
| `claim-carol-private` | applied | applied (nullifier written) | 94 | none: no fee, no signer |
| `burn-alice-eth-not-a-denomination` | refused `E7408` | reverted | 95 | 255,000 (its limit) |
| `burn-alice-eth` (owner-signed) | applied | applied | 96 | 228,729 |
| `burn-carol-private` | applied | applied (leaf appended) | 137 | none: no fee, no signer |

Every applied public transaction was charged exactly the gas the client's
pre-check measured. Every refusal was charged exactly its gas limit, and
nothing changed. The claims cost 105 gas more than on 2026-10-17 (287,283):
the anvil vault's address changed, because RISC Zero's verifier is deployed
first. Its storage-proof nodes changed with it.

State checks along the way:

- alice holds 1 wrapped ETH, then 100 wrapped USDT;
- the supply of wrapped ETH is 1 ETH after the public claims, 2 after the
  private claim, 1 after alice's burn and 0 after carol's;
- bob's deposit stays unclaimed (its nullifier account is empty);
- the three Groth16 proofs are real (dev mode off); the zero-knowledge
  verifier is pinned to the committed guest's ImageID, it is the vault's
  verifier, and the seals carry its RISC Zero verifier's selector;
- none of the nine zero-knowledge releases that carry carol's or alice's
  proof sent the leaf, the secret, the Ethereum commitment or a path node;
- the vault paid out exactly 2 ETH, and carol received 0.99 ETH;
- both burn nullifiers are recorded as released.

### The private transactions, through the sequencer

| | private claim | private burn |
|---|---:|---:|
| prove and submit (`send_privacy_preserving_tx`, real proving) | **285.0 s** | **201.8 s** |
| the client process, wall | 291.1 s | 203.9 s |
| peak resident memory (`/usr/bin/time -l`) | **7.85 GB** | **7.82 GB** |
| transaction on chain (`getTransaction`, with its variant tag) | **251,335 bytes** | **248,358 bytes** |
| load average before | 3.2 (13.7 over 5 min) | 33 |
| RISC Zero dev mode | off | off |

The heavy lock (`$HEAVY`) kept a second prover out during
both, but other work was running on the machine, so these times are upper
bounds for an idle machine. On 2026-10-17 the claim measured 247.6 s and
6.17 GB, and the burn 202.9 s and 7.22 GB. The executor-level measurement
gave 252 to 254 s and 6.7 to 8.3 GB
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
  appended it. The zero-knowledge release does not send that leaf back.

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

See [OUTBOUND.md](OUTBOUND.md), section 3, for the proof timings and the gas
table. The two applied zero-knowledge releases used 334,215 gas (alice,
fee 0) and 343,550 gas (carol, through the relayer). Each proof took about
20 s for the STARK and 52 s for the emulated Groth16 wrap, with a seal of
260 bytes.

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

For the zero-knowledge releases:

- the vault's `VERIFIER()` is the recorded `ZkBurnVerifier`;
- its `IMAGE_ID()` is the committed guest's, and its `RISC0()` is the
  recorded RISC Zero verifier;
- for each applied release, the seal and the journal are decoded from the
  transaction's input on chain. `sha256(journal)` must equal the vault's
  `releaseDigest` of the release's arguments, and the journal's root and
  nullifier must be the recorded ones. RISC Zero's verifier must accept the
  seal for that ImageID and digest (`eth_call`), and must refuse it with one
  byte flipped;
- the burn's leaf and every node of its path occur nowhere in that input.

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
the manifest as recorded verifies (81 checks, 0 fail), and **all 32 mutations
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
wei. The 4 added with the zero-knowledge releases: the committed guest
recorded with another ImageID; the zero-knowledge verifier recorded as RISC
Zero's verifier (which the vault does not use); a released burn's leaf
recorded as a word the release does send (its nullifier); a path node
recorded as a word the release does send (its root). The committee run
([`evidence/local-committee/`](../evidence/local-committee/)) has no
zero-knowledge lines, so its mutation test runs the first 28.

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
# the same with the releases opening their leaf (CommitteeBurnVerifier):
RELEASE_VERIFIER=committee OUT=evidence/local-committee/e2e.tsv SEQUENCER=… WALLET=… LEZ_SRC=… scripts/e2e-local.sh
```

The zero-knowledge releases need Docker and RISC Zero's Groth16 prover image,
`risczero/risc0-groth16-prover:v2025-04-03.1`. It is x86_64 only, 2.8 GB; pull
it first with `docker pull --platform linux/amd64 …`. On Apple Silicon,
`scripts/zk-prove-measured.sh` sets `DOCKER_DEFAULT_PLATFORM=linux/amd64` so
that r0vm's `docker run` gets the emulated image. One proof takes about 75 s
and about 7.6 GiB in the Docker VM (11.7 GiB here, which was enough).

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
- Both release verifiers take the burn tree's root from a 2-of-3 committee
  ([OUTBOUND.md](OUTBOUND.md)). That is a stated trust assumption, not the
  cryptographic F.7. The zero-knowledge release hides which burn it pays
  out. Its journal (token, amount, recipient, relayer, fee, root, nullifier)
  is public, so it hides only among the burns of the same token and amount
  under that root: two in this run. The committee release opens its burn
  leaf, so it is linkable to the LEZ burn transaction.
- The Groth16 wrap ran emulated (x86_64 on arm64). On a native x86_64 host
  it should be faster, but that was not measured here.
- The v0.3.0 `wallet` binary cannot sync against v0.3.1 blocks itself
  (`wallet account sync-private` fails decoding the header: v0.3.1 dropped the
  producer key and signature). The sync is the wallet crate's, in the client,
  built against LEZ's v0.3.1 `common` (`cli/vendor/lez-common`); the binary
  then reads the decrypted account from the same wallet home.
- The run is one sequential client: nothing else touched the bridge's
  accounts while the private claim or burn was proven. Concurrent claims or
  burns of the same token during a proof were not exercised.

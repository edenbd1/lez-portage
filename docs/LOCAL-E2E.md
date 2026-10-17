# The bridge on a local LEZ v0.3.1 sequencer, end to end

[`scripts/e2e-local.sh`](../scripts/e2e-local.sh) runs the whole inbound leg
and a public burn through the client (`cli/`, `wrap`) against a **local**
LEZ v0.3.1 standalone sequencer and a **local** anvil chain. Nothing reaches a
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
   committed Docker build (`a7c903ab…`), the last one with an immutable
   header. They are loaded before the vault exists because the vault is
   deployed with the pairing, which names wrap_bridge's program id.
2. **Ethereum (anvil): the deposits.** `WrapVault` and a USDT-shaped token
   (transfers return nothing) are deployed with that pairing, both tokens are
   registered, and four deposits are made
   ([`scripts/anvil_deposits.py`](../scripts/anvil_deposits.py)):
   alice 1 ETH and 100 USDT, bob 0.1 ETH, all to public LEZ accounts, and
   carol 1 ETH committed to her **private** LEZ account, created in her wallet
   before the deposit. Then the block, the `Deposit` events and an
   `eth_getProof` of `knownRoot[root]` for every root.
3. **LEZ: Sextant, the bridge, the claims, the burn.**

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
| alice burns 1 ETH (Ethereum recipient hidden in a commitment) | applied |

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

Run of 2026-10-17 12:54Z at commit `66210e5`, Apple M4 Pro (14 cores,
24 GiB), sequencer `1a003bfb` (standalone), wallet `db66590a`, r0vm 3.0.5.
**PASS: 13 transactions, 8 applied and 5 refused as expected, 15 state checks
ok**, after 25 program_loader transactions loading the four programs (each
header names its ImageID).

| transaction | expected | verdict | block | gas (charged) |
|---|---|---|---:|---:|
| `init-sextant` | applied | applied | 27 | 1,831,160 |
| `init-bridge` | applied | applied | 28 | 27,053 |
| `register-eth` (admin-signed) | applied | applied | 29 | 90,577 |
| `register-usdt` (admin-signed) | applied | applied | 30 | 89,935 |
| `claim-alice-eth` | applied | applied | 31 | 291,351 |
| `claim-alice-eth-replay` | refused `E7402` | reverted | 32 | 330,000 (its limit) |
| `claim-alice-eth-replay-old-root` | refused `E7402` | reverted | 33 | 330,000 (its limit) |
| `claim-alice-usdt` | applied | applied | 34 | 291,423 |
| `claim-bob-redirected-to-alice` | refused `E7403` | reverted | 35 | 330,000 (its limit) |
| `claim-phantom-unknown-root` | refused `E7208` (Sextant) | reverted | 36 | 330,000 (its limit) |
| `claim-carol-private` | applied | applied (nullifier written) | 90 | none: no fee, no signer |
| `burn-alice-eth-not-a-denomination` | refused `E7408` | reverted | 91 | 255,000 (its limit) |
| `burn-alice-eth` (owner-signed) | applied | applied | 92 | 228,691 |

Every applied transaction was charged exactly the gas the client's pre-check
measured, and every refusal exactly its gas limit, with nothing changed. The
public claim costs 291,351 gas on the sequencer (297,502 in the executor
tests, whose vault, root and proofs differ).

State checks along the way: alice holds 1 wrapped ETH, then 100 wrapped USDT;
supply 1 ETH after the public claims, 2 after the private claim, 1 after the
burn, alice 0; bob's deposit stays unclaimed (its nullifier account is empty).

### The private claim, through the sequencer

Carol's deposit (index 3, 1 ETH) committed to `Private/4NRsHioF…`, an account
her wallet created before the deposit. The client proved the claim and the
sequencer included it in block 90:

| | |
|---|---:|
| prove and submit (`send_privacy_preserving_tx`, real proving) | **263.1 s** |
| the client process, wall | 269.2 s |
| peak resident memory (`/usr/bin/time -l`) | **6.87 GB** |
| transaction on chain (`getTransaction`, with its variant tag) | **251,527 bytes** |
| RISC Zero dev mode | off |

The same order as the executor-level measurement (252 to 254 s, 6.7 to 8.3
GB, [PRIVATE-CLAIM-MEASURED.md](PRIVATE-CLAIM-MEASURED.md)). The transaction
is 21 kB larger than that one's 230,510 bytes: the wallet pads every
privacy-preserving transaction to seven private inputs (six dummies here) and
every note to 512 bytes, so that the number of private accounts a transaction
touches does not show; the executor-level transaction has neither. The split
of those 21 kB was not measured.

What the client checked in the transaction's bytes as the chain returns
them: the destination's account id, the secret, the deposit's commitment and
leaf, and both non-empty nodes of its Merkle path occur nowhere. The verifier
re-checks the destination's absence from the chain's copy.

Decryption: the client's wallet (`WalletCore::sync_to_latest_block`) synced
to block 90, found the note by its view tag and decrypted it: carol holds
1 wrapped ETH of the definition the bridge registered, and the note's
commitment is in the chain's commitment set. The v0.3.0 `wallet` binary then
shows the same record from the same wallet home (`wallet account get
--account-id Private/4NRsHioF… --scope <token header>`: a fungible holding of
`0x0de0b6b3a7640000` = 10^18). Nothing about the account is public: the
chain has no shard under its id.

## The verifier and its mutation test

[`scripts/verify-onchain.sh`](../scripts/verify-onchain.sh) re-checks a
manifest against the sequencer with nothing but JSON-RPC: every program
header names its ImageID; every deployment transaction resolves; every
transaction resolves by hash (`getTransaction`) to its recorded block, and
the hash is SHA-256 of the returned bytes after the variant tag; a public one
was charged as its verdict says (applied: the pre-checked gas, below the
limit; refused: exactly the limit; it must be the only charged transaction of
its block); the private one is a privacy-preserving transaction of the
recorded size in which the private destination's id does not occur; every
account the run wrote (the Sextant instance and header record, the bridge
config, both entries, the burn tree, the four nullifier accounts, both
definitions, alice's two ATAs) holds the bytes the run left; and a random
hash does not resolve.

[`scripts/mutation-test-verifier.sh`](../scripts/mutation-test-verifier.sh)
corrupts one thing at a time in a copy of the manifest and requires the
verifier to fail on each with a FAIL line.

Against this run ([`mutation-test.txt`](../evidence/local/mutation-test.txt)):
the manifest as recorded verifies (57 checks, 0 fail), and **all 15
mutations are caught**: an applied transaction's hash replaced by one never
sent; a refusal claimed as applied; an application claimed as refused; the
pre-checked gas off by one; a transaction recorded in the next block; two
hashes swapped; the private claim's hash replaced; the private claim recorded
as public; a public transaction recorded as the private claim; the private
destination replaced by an id the transaction does carry (the bridge config);
the private claim's size off by one; a nullifier account's final bytes
altered; the unclaimed deposit's nullifier recorded as spent; another ImageID
for wrap_bridge; a deployment hash never sent.

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
- The private burn and the release verifier are not built; the burn here is
  public.
- The v0.3.0 `wallet` binary cannot sync against v0.3.1 blocks itself
  (`wallet account sync-private` fails decoding the header: v0.3.1 dropped the
  producer key and signature). The sync is the wallet crate's, in the client,
  built against LEZ's v0.3.1 `common` (`cli/vendor/lez-common`); the binary
  then reads the decrypted account from the same wallet home.
- The run is one sequential client: nothing else touched the bridge's
  accounts while the private claim was proven (block 36 to 90). Concurrent
  claims of the same token during a proof were not exercised.

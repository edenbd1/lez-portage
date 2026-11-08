# The inbound path on public networks: Sepolia to the LEZ testnet

On 2026-11-08 the inbound path ran on two public networks. Two deposits of
0.001 ETH were made into a `WrapVault` on **Sepolia**. A **Sextant** instance
on the **public LEZ testnet** (v0.3.1) was initialised from a Sepolia checkpoint
finalized after them; its execution block is the block of the second
deposit. `wrap_bridge`, deployed on the same testnet, then minted both
deposits: one by a **public claim** into alice's account, one by a
**privacy-preserving claim** into carol's shielded account, which her wallet
decrypts. Each claim proves `knownRoot[root] == 1` in the vault's storage
against Sextant's record of that header, by a chained `Attest`. No signer and
no committee is involved.

On LEZ: 7 transactions, 5 applied and 2 refused as expected, plus the
deployment of two programs. On Sepolia: a verifier, the vault, a registration
and two deposits. `scripts/verify-testnet.sh` re-checks all of it with
JSON-RPC only: 29 checks on LEZ and 14 on Sepolia and the link between them,
0 failures. The LEZ verifier's mutation test catches 16 of 16 mutations.

Before any of it, the same stages ran against a local v0.3.1 sequencer, with
a second Sepolia vault and a checkpoint of its own (a dry run). The local
chain is discarded, so that run is not kept as evidence.

Record of the run: [`evidence/testnet/`](../evidence/testnet/) (`run.tsv` the
manifest, `transcript.txt`, `verify.txt`, `sepolia/` the vault's deployment,
deposits, the checkpoint's execution block and the storage proofs,
`sextant-run.tsv` Sextant's own manifest of its part).

## What ran, in order

1. **LEZ: the programs.** Two were already on the testnet and are reused as
   they are: Sextant v2 (`Ebe8rifvjyvwWUXnk7bsKeTwuyCAFSRMFuPMVHE8CbUB`,
   ImageID `ad6fbedb…`, the program of
   [lez-sextant's testnet runs](https://github.com/edenbd1/lez-sextant/blob/main/docs/ONCHAIN.md))
   and LEZ v0.3.0's token program (`3cwJjDqD17a7k5zMaYM82sgai7WdRzWfLCtRGntZ6SBb`,
   ImageID `912d412c…`, loaded immutable; the v0.3.1 testnet has no builtin
   token program). The client loaded two more through program_loader: LEZ
   v0.3.0's associated-token-account program (`2b1d5580…`) and the committed
   `wrap_bridge.bin` (`073f046c…`, immutable header), after a clean
   `DOCKER=1 scripts/build-guest.sh` rebuild `cmp`-identical to it.
2. **Sepolia: the vault and two deposits.** `WrapVault` is deployed with the
   pairing of that wrap_bridge deployment (LEZ chain, zone, program id,
   deployment id), Ether is registered with one permitted amount, 0.001 ETH,
   and two deposits of 0.001 ETH are made: one committed to alice's public LEZ
   account, one to carol's private account. The depositors' seeds are fresh
   for this run and are not published.
3. **Sepolia: a checkpoint after the deposits.** Once Sepolia finalized the
   block of the last deposit, Sextant's `scripts/fetch-vectors.py sepolia
   vectors/sepolia-portage --finalized-after <block>` took the finalized
   header of the latest light-client finality update, the
   `LightClientBootstrap` at it (identical from Nimbus and Lodestar), and the
   RLP of its execution block (Gloas headers commit to it by hash only). At
   that execution block, `eth_getProof` of the vault's `knownRoot[root]` slot
   for every root the tree has had, each checked against the block's state
   root.
4. **LEZ: a Sextant instance from that checkpoint.** `Init` costs one
   transaction (about 1.84M gas) where advancing an instance by a
   sync-committee update costs three (about 13M gas); since the checkpoint
   is after the deposits, no update is needed: the header record `Init`
   writes is the one the claims are attested against. Sextant's own
   `scripts/e2e.sh SCENARIO=chain CHAIN=sepolia-portage` ran it, with a log
   of the checkpoint's block attested and the same query refused as a mainnet
   statement (`E7209`).
5. **LEZ: the bridge and the claims.** `Init` and `Register` (Wrapped Sepolia
   Ether, the one amount, a cap of 0.01); the public claim: the deposit's
   path to the root, Sextant's `Attest` of `knownRoot[root] == 1` under the
   checkpoint's state root as a chained call, the nullifier, the chained
   token `Mint` into alice's associated token account; two refusals, charged
   their whole gas limit: the same deposit again (`E7402`) and a genuine
   storage proof of a root the vault never had (`E7208`); then the private
   claim, a privacy-preserving transaction proven on this machine, into
   carol's shielded account, which her wallet decrypts.

## Transactions

Sepolia (chain 11155111), deployer `0xf4e827cd18597f071FB8bca9A77319AD619927D7`:

| step | transaction | block |
|---|---|---:|
| `CommitteeBurnVerifier` (release path, not exercised) | [`0x8e6b2b1610297ea0…`](https://sepolia.etherscan.io/tx/0x8e6b2b1610297ea0464d8a1bfd9fafcf1f324b77a7401f98abd59d0444dad38e) | |
| `WrapVault` `0x4Ae65712A472e11842A2e1Fe61C7A52Da99c8F7a` | [`0x86521e22b33f49aa…`](https://sepolia.etherscan.io/tx/0x86521e22b33f49aa3a533d7e6a01251cd4c6d8265c77fe0b02b37ad42f84d288) | 12,092,946 |
| `registerToken(ETH, 18, [0.001 ETH], …)` | [`0x1c8f710574e9a050…`](https://sepolia.etherscan.io/tx/0x1c8f710574e9a050497cb0b80af03c2d26b0565749b7d2b04cca1a4ff497df12) | |
| deposit 0, 0.001 ETH, committed to alice (public), 847,409 gas | [`0x8385eedf92b93fba…`](https://sepolia.etherscan.io/tx/0x8385eedf92b93fba39939376e17717da29a0ca77c329cba65e7b9f3e5ac8dbfb) | 12,092,949 |
| deposit 1, 0.001 ETH, committed to carol (private), 531,046 gas | [`0x2508a66321461e38…`](https://sepolia.etherscan.io/tx/0x2508a66321461e382fe7c00fe5cff9ca9232ef089ecde500bebcb92934207fdd) | 12,092,950 |

The checkpoint: beacon slot 11,536,448 (finalized), execution block
12,092,950 (`0x95055659…`, state root `0xcb2f2b05…`), from
`fetch-vectors.py --finalized-after 12092950`; in lez-sextant at
`vectors/sepolia-portage/`, here at
[`evidence/testnet/sepolia/`](../evidence/testnet/sepolia/).

LEZ public testnet (`https://testnet.lez.logos.co`), payer
`9nqn7xA7z54QZPHs8wWn5VzTeXTgbwq8SREr38tVK9aw`:

| step | verdict | transaction | block | gas |
|---|---|---|---:|---:|
| ATA program, header `2ME6FvQq…` (4 segments, then the header) | loaded | [`4f53672f5e28442a…`](https://explorer.testnet.lez.logos.co/transaction/4f53672f5e28442a49304caf6f8edfd883fc76db9f7cc3520970441ea42ea1b6) | 79,159 | storage only |
| `wrap_bridge`, immutable header `D6XntqqUqgFRUhAJkeM6hGzRSfJm7B3KKukyDxR5Sb4F` (5 segments, then the header) | loaded | [`59ef746a13bcb30d…`](https://explorer.testnet.lez.logos.co/transaction/59ef746a13bcb30d913ea3639b706cf366a20fb161b7b32c2b1d1c535d48bf62) | 79,173 | storage only |
| Sextant `Init`, instance `sepolia-portage-1108` (`8if4JHY1…`), header record `9Vs169Nn…` | applied | [`a229935353b74089…`](https://explorer.testnet.lez.logos.co/transaction/a229935353b740896e3916c505dd7167de3d7251d38a54642b69d8cd7b839ccc) | 79,184 | 1,834,893 |
| Sextant `Attest`, a log of the checkpoint's block | applied | [`7cbd9d7bdc7c2ff8…`](https://explorer.testnet.lez.logos.co/transaction/7cbd9d7bdc7c2ff8897fb8c44ee3c687332e57d668194ebcab0ec0aece6c177f) | 79,185 | 95,791 |
| Sextant `Attest`, the same query as a mainnet statement | refused `E7209` | [`76a47309ba9625e0…`](https://explorer.testnet.lez.logos.co/transaction/76a47309ba9625e087df62980f29eff16803075ad0af468709549c31185db886) | 79,186 | 100,290 (its limit) |
| bridge `Init` | applied | [`aa654a7ae947c521…`](https://explorer.testnet.lez.logos.co/transaction/aa654a7ae947c5219cb9e14c363953aa1c15df0e764ac052e61e25e060c6badc) | 79,187 | 27,049 |
| `Register` Wrapped Sepolia Ether | applied | [`3ad7971422cec2a7…`](https://explorer.testnet.lez.logos.co/transaction/3ad7971422cec2a73fa1dcf901cbaf14ea6966e199e7000900a25a00a2d121c6) | 79,188 | 90,410 |
| **public claim**, deposit 0 to alice: path, chained `Attest` (9 account and 4 storage nodes), nullifier, chained `Mint` | applied | [`834a346002e0410e…`](https://explorer.testnet.lez.logos.co/transaction/834a346002e0410e5248a45e4ac79ce92bda88e327c3d8c7f1611b6ee5881752) | 79,190 | 451,107 |
| the same deposit again | refused `E7402` | [`b0c840a246afbc3b…`](https://explorer.testnet.lez.logos.co/transaction/b0c840a246afbc3b58a5bf093ac499bc4f43910950a09b88b8e4deee37370579) | 79,190 | 496,217 (its limit) |
| a genuine proof of `knownRoot[r] == 0` for a root the vault never had | refused `E7208` | [`974f5e7de5c1914b…`](https://explorer.testnet.lez.logos.co/transaction/974f5e7de5c1914bb28098e949fbff5655d7037f185930a7d7f1898e39a901b3) | 79,191 | 496,217 (its limit) |
| **private claim**, deposit 1 into carol's shielded account (fee-exempt) | applied | [`4f6dfe0ab587d1c1…`](https://explorer.testnet.lez.logos.co/transaction/4f6dfe0ab587d1c15f6c4292c8be9183d8f1ef036b250f67fc70bdebfa837ca3) | 79,196 | |

Carol's wallet decrypts 0.001 wrapped ETH in `Private/F8WThjdH…`, and her
note's commitment is in the chain's commitment set. The account has no public
state on chain. The wrapped token's supply is 0.002, and both nullifiers are
spent. The private claim was proven on an Apple M4 Pro (r0vm 3.0.5, dev mode
off): 264.6 s to prove and submit, 8.72 GB peak memory, 257,739 bytes on
chain. The destination, secret, commitment, leaf and path do not occur in its
bytes.

Two things happened on the testnet during the run, and both are in the
manifest. **It stalled** several times, once for about half an hour. Some
sends were therefore repeated with the same nonce; only the first landed
(`# resend` lines), and one refusal was included after the client had
stopped waiting (`E7209`, `# late` in Sextant's manifest). **It rewrote
recent blocks**: the public claim was read back alone in block 79,189,
charged exactly its pre-checked gas, and now resolves in block 79,190
together with the replay. The verifier checks such a block as a group: its
execution gas, 947,324, is exactly 451,107 + 496,217, and its storage gas is
exactly the two transactions' sizes (`# shared-block`).

## Cost

| | testnet token |
|---|---:|
| ATA program and `wrap_bridge` loaded (storage gas only) | 6,272,160 |
| Sextant: `Init`, the log attested, the `E7209` refusal | 15,676,568 |
| bridge `Init`, `Register`, the public claim, two refusals (the private claim is fee-exempt) | 13,470,736 |
| **total** | **35,419,464** |

The payer held 66,259,116 before the run and 30,839,652 after it. The base
fee was 8 throughout. A public claim with a Sepolia storage proof (9 account nodes, 4
storage nodes) is 451,107 gas, 4.5% of the 10M cap; on anvil, with a 3-node
proof, the same claim is 297,457 gas (README): the difference is Sextant's
`Attest` walking the longer proof. Sepolia: 0.004 ETH deposited across the two vaults
(the dry run's and this one), the rest in gas.

## What this shows, and what it does not

- **Shown:** a deposit on a public Ethereum testnet is minted on the public
  LEZ testnet on nothing but a light client's record of a Sepolia header and
  a Merkle proof under it. No one signs for the deposit. The record is the
  one Sextant's `Init` wrote from a public checkpoint, and the claim's
  storage proof is checked on chain against it, inside the claim.
- **The trusted input** is the checkpoint, as for any light client: a
  finalized Sepolia header taken from public beacon nodes (two
  implementations, identical bytes). This instance is never advanced: it
  attests the state at its checkpoint only, which is enough for deposits made
  before it. A deposit made later needs a later header, by an update of this
  instance (three transactions, about 108M of the testnet token per update at
  the current base fee) or by a new instance from a later checkpoint (one
  `Init`, about 15M).
- **Outbound is not exercised here.** The Sepolia vault's release verifier is
  a `CommitteeBurnVerifier` (2 of 3) and no release was made on Sepolia; the
  release path, with its zero-knowledge and Bedrock-anchored verifiers, ran
  on the local runs only ([OUTBOUND.md](OUTBOUND.md)).
- **A public claim links to its deposit**: it carries the deposit's opening.
  The private claim carries none of it (destination, secret, commitment,
  leaf, path), which `verify-onchain.sh` re-checks on the transaction's bytes.
- **Accounts are published**: the manifest names carol's private account so
  that the verifier can check that its id occurs nowhere in the private
  claim's bytes. A real user would not publish it.

## Re-check

```console
scripts/verify-testnet.sh            # LEZ testnet and Sepolia, JSON-RPC only
```

It runs `scripts/verify-onchain.sh` against `https://testnet.lez.logos.co`
(every transaction resolves to its block and hashes to its bytes, each one
charged what its verdict implies, the private claim carries no signature and
not its destination, the four programs' ImageIDs from their program_loader
headers, the final state of every account the run wrote, a random hash does
not resolve) and `scripts/verify-sepolia.py` (the vault, its pairing and
verifier, every deposit and its event, the checkpoint's execution block and
state root on Sepolia now, the committed bootstrap committing to that block,
the committed storage proof under that state root, `knownRoot` on Sepolia
now and a random root as a control, Sextant's header record on LEZ holding
that state root, and the public claim carrying the root and the vault).
Output of the run: [`evidence/testnet/verify.txt`](../evidence/testnet/verify.txt).

## Reproduce

```console
cargo build --release --manifest-path cli/Cargo.toml
# lez-sextant at 0338eb3 or later (`--chain sepolia-portage`, fetch-vectors.py --finalized-after), its client built
export RUN=<a private directory> SEXTANT_REPO=<lez-sextant> LEE_WALLET_HOME_DIR=$RUN/wallet PAYER=<funded account>
export SEXTANT_PROGRAM=Ebe8rifvjyvwWUXnk7bsKeTwuyCAFSRMFuPMVHE8CbUB TOKEN_PROGRAM=3cwJjDqD17a7k5zMaYM82sgai7WdRzWfLCtRGntZ6SBb
INSTANCE=<at most 32 bytes> scripts/testnet-inbound.sh lez-deploy
SEPOLIA_KEY_FILE=<deployer key file> scripts/testnet-inbound.sh eth
scripts/testnet-inbound.sh fetch          # retry until Sepolia has finalized the last deposit (about 15 minutes)
scripts/testnet-inbound.sh sextant
HEAVY=<optional wrapper> scripts/testnet-inbound.sh claims
scripts/verify-testnet.sh
```

`MODE=local` runs the same stages against a local v0.3.1 standalone sequencer
(`local-up` starts it; `SEQUENCER`, `WALLET`, `LEZ_SRC` as for
[`e2e-local.sh`](LOCAL-E2E.md)), and loads Sextant v2 and the token program
there too. The vault is still deployed on Sepolia, and the checkpoint is still
Sepolia's. A deployment cut short by a stalled testnet is resumed with
`RESUME_<program>="<header> <segment 0> …"`, the accounts it had already
created.

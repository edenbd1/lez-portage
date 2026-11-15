# Portage on public networks: Sepolia and the LEZ testnet

Both directions have run on public networks: inbound on 2026-11-08 (this
section and the next ones), outbound on 2026-11-14 and 15
([The outbound path](#the-outbound-path-back-from-the-lez-testnet-to-sepolia)).

## The inbound path: Sepolia to the LEZ testnet

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
JSON-RPC only: 26 checks on LEZ and 14 on Sepolia and the link between them,
0 failures (29 on LEZ before the outbound run rewrote three of the accounts
it pins; those three are now checked against the outbound manifest). The LEZ verifier's mutation test catches 16 of 16 mutations.

Before any of it, the same stages ran against a local v0.3.1 sequencer, with
a second Sepolia vault and a checkpoint of its own (a dry run). The local
chain is discarded, so that run is not kept as evidence.

Record of the run: [`evidence/testnet/`](../evidence/testnet/) (`run.tsv` the
manifest, `transcript.txt`, `verify.txt`, `sepolia/` the vault's deployment,
deposits, the checkpoint's execution block and the storage proofs,
`sextant-run.tsv` Sextant's own manifest of its part). The manifest's header
reads `commit 33250a0 (uncommitted changes)`: the run's own client and
scripts (`cli/src/main.rs`, `scripts/testnet-inbound.sh`,
`scripts/sepolia_vault.py` and the verifiers) were not yet committed, and
went into `9774399` with this record; nothing under `programs/`, `core/`,
`vault/` or `artifacts/` differs between the two commits, and the binaries
that ran are pinned by size, sha256 and ImageID in the same header.

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
| `CommitteeBurnVerifier` (the release verifier; exercised by the outbound run) | [`0x8e6b2b1610297ea0…`](https://sepolia.etherscan.io/tx/0x8e6b2b1610297ea0464d8a1bfd9fafcf1f324b77a7401f98abd59d0444dad38e) | 12,092,946 |
| `WrapVault` `0x4Ae65712A472e11842A2e1Fe61C7A52Da99c8F7a` | [`0x86521e22b33f49aa…`](https://sepolia.etherscan.io/tx/0x86521e22b33f49aa3a533d7e6a01251cd4c6d8265c77fe0b02b37ad42f84d288) | 12,092,947 |
| `registerToken(ETH, 18, [0.001 ETH], …)` | [`0x1c8f710574e9a050…`](https://sepolia.etherscan.io/tx/0x1c8f710574e9a050497cb0b80af03c2d26b0565749b7d2b04cca1a4ff497df12) | 12,092,948 |
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
- **Outbound is a separate run** ([below](#the-outbound-path-back-from-the-lez-testnet-to-sepolia)).
  It released through this vault's `CommitteeBurnVerifier` (2 of 3). The
  zero-knowledge and Bedrock-anchored verifiers ran on the local runs only
  ([OUTBOUND.md](OUTBOUND.md)).
- **A public claim links to its deposit**: it carries the deposit's opening.
  The private claim carries none of it (destination, secret, commitment,
  leaf, path), which `verify-onchain.sh` re-checks on the transaction's bytes.
- **Accounts are published**: the manifest names carol's private account so
  that the verifier can check that its id occurs nowhere in the private
  claim's bytes. A real user would not publish it.

## The outbound path: back from the LEZ testnet to Sepolia

On 2026-11-14 and 15 both wrapped deposits of the inbound run went back. On
the **public LEZ testnet**, alice burned her 0.001 wrapped ETH by a public
`Burn`, and carol burned hers from her shielded account by a
privacy-preserving `BurnFrom` proven on this machine. Each burn committed to
a fresh Sepolia address. On **Sepolia**, the `WrapVault` deployed on
2026-11-08 released 0.001 ETH to each address, against the root of the LEZ
burn tree, attested by its committee. Three release attempts were refused
on Sepolia as expected: not final, one member only, and a replay. The vault
now holds 0 wei.

`scripts/verify-outbound.sh` re-checks all of it with public JSON-RPC
(the LEZ testnet and a Sepolia endpoint serving historical state): 23 checks
on the transactions and final state and 21 on the releases against the burns
they pay out (29 with `--bedrock-tools`, which re-derives the saved Bedrock
blocks with the node's own types), 0 failures. Its mutation test, `scripts/mutation-test-outbound.sh`,
catches 27 of 27 mutations.

The comparison of a LEZ block with its Bedrock inscription skips one byte:
the last, `bedrock_status` (the last field of `Block` in
`lez/common/src/block.rs`, Borsh: Pending 0, Safe 1, Finalized 2). It is
the node's own view of the block's progress on Bedrock, it is not covered by
the block hash, and it changes after inscription. The blocks were inscribed
while Pending, and the RPC now returns them Finalized, so a byte-exact
comparison failed for the first time on 2026-11-15. Every other byte, the
header with its hash and the body, must still match. Two mutations cover the
rule: one changes the byte before the status, one sets an invalid status.
The members' signing path (`scripts/sepolia_release.py`) uses the same
comparison.

Record: [`evidence/testnet-outbound/`](../evidence/testnet-outbound/)
(`run.tsv` the manifest, `transcript.txt` the stages' output, `verify.txt`,
`mutation-test.txt`, `bedrock/` the Bedrock blocks each member walked before
signing). The manifest's header reads `commit 36c4d98`, without the
"uncommitted changes" mark: the run's scripts (`scripts/testnet-outbound.sh`,
`scripts/sepolia_release.py`) were new, untracked files, which the check did
not see. They went into the commit that added this record, with the check
fixed. The client (`cli/`), the programs, `vault/` and `artifacts/` are those of
`36c4d98`, and the binaries are pinned by size, sha256 and ImageID in the
manifest's header.

### Which release verifier, and what is trusted

The deployed vault's release verifier was fixed at construction
(Security #8). It is the `CommitteeBurnVerifier` deployed with it: 2 of 3 members, a
finality depth of 30 LEZ blocks. The stronger verifiers (`ZkBurnVerifier`,
`ZkChainBurnVerifier`, `ZkBedrockBurnVerifier`, [OUTBOUND.md](OUTBOUND.md))
each need a new vault, and the escrowed Ether is in this one. A new vault
would need new deposits, a new Sextant instance and new claims before any
release, so none was deployed. This is the weakest of the four verifiers.

What the vault checks on chain, for each release:

- the opening: the release carries the burn's token, amount, recipient, fee,
  secret, index and Merkle path; the verifier recomputes the Ethereum
  commitment, the leaf, the root and the nullifier (SHA-256), so the payout
  goes only to the recipient and fee the burn committed to;
- 2 distinct members' EIP-191 signatures over
  `attestationDigest(vault, pairing, root, lezBlock, lezFinalBlock)`, with
  `lezFinalBlock >= lezBlock + 30`;
- the nullifier is spent once (`AlreadyReleased` on the replay).

**What is trusted: the committee's word that the root is the LEZ burn
tree's root, final.** Nothing on Ethereum checks LEZ. Before signing, each
member:

1. reads the burn tree's root from the public LEZ testnet itself
   (`getAccount`, between two equal reads of the tip) and signs only the
   root it read, at the block it read it;
2. waits until the LEZ tip is 30 blocks above that block, and names that tip
   final;
3. reads that final LEZ block (`getBlock`) and refuses to sign unless it is,
   byte for byte, the inscription opening a transaction on the LEZ channel
   (`0101…01`) of a Bedrock block on its node's canonical chain, with at
   least k = 30 Bedrock blocks above it. `bedrock-tools anchor` re-derives
   those Bedrock blocks with the node's own types and checks the
   inscription's Merkle path to the header's body root, its Ed25519
   signature and the headers above it.

The limits of that, stated plainly:

- **One operator holds all three member keys**, and the relayer's. The 2 of
  3 is the contract's rule, not three independent parties.
- **One Bedrock node.** It is a `logos-blockchain-node` 0.3.1 following the
  public Bedrock testnet, run on this machine for this run and stopped
  after it. All members used it. It was "Online" and "Following". Its
  canonical chain is its own view: its LIB (slot 3,095,116 when the members
  signed) was below both inscriptions (slots 3,146,664 and 3,148,037). So the
  inscriptions were 124 and 103 headers deep on the node's chain, well over
  k = 30, but not yet below the node's LIB.
- **The inscription's signer was not pinned.** The member checks the Ed25519
  signature against the key named in the inscription itself. The two
  inscriptions are signed by different keys (`0d27b709…` for LEZ block
  82,406, `30cfd05a…` for 82,412). The public testnet's LEZ channel carries
  more than one key, and this run did not establish which keys the channel
  authorizes. What the Bedrock check does show is that the LEZ block the
  public RPC serves, byte for byte, was published on Bedrock and had k
  blocks built on it in that node's view.
- **The opening is public.** A committee release opens the leaf, so the
  release links to its burn. For alice's public burn that link is already
  public. For carol's private burn, the release reveals the leaf, the index
  and the recipient. Her LEZ account still appears nowhere: the private burn's
  bytes carry neither the account, nor the recipient, nor the secret. The
  zero-knowledge verifiers, which hide the opening, ran on local chains only.

`scripts/verify-outbound.sh` re-checks the members' checks rather than
trusting them. It reads the vault's `VERIFIER()` and `PAIRING()`, and the
verifier's `members()`, `THRESHOLD()` and `FINALITY_DEPTH()`. It decodes each
release's opening from its calldata and recomputes it. It finds the LEZ burn
it pays out on the testnet, carrying the commitment (public) or the leaf
(private). It recovers 2 distinct members' signatures over the digest,
recomputed. It checks that the LEZ blocks from the burn's to the final one
are one hash-linked chain, and that the payout went to the recipient and out
of the vault in the release's block. It rebuilds the burn tree's current root
from the released openings. With the saved Bedrock blocks, it checks that the
final LEZ block, as the public RPC returns it now, is byte for byte the
inscription in the recorded Bedrock block, and that the saved walk links down
from the node's tip, at least 30 blocks above it. With
`--bedrock-tools bedrock/tools/target/release/bedrock-tools`, it re-derives
every saved block with the node's own types and re-checks the anchor. There
is no public Bedrock RPC to re-check against, so the Bedrock part is checked
against the saved blocks, which the members' node served.

### Transactions

LEZ public testnet, payer `9nqn7xA7z54QZPHs8wWn5VzTeXTgbwq8SREr38tVK9aw`:

| step | verdict | transaction | block | gas |
|---|---|---|---:|---:|
| alice's public `Burn` of 0.001 wrapped ETH, committed to `0x794Dc407…` | applied | [`dc459d9af502a320…`](https://explorer.testnet.lez.logos.co/transaction/dc459d9af502a32037c4aee3784fe93b8d113ae43e18f34b5ba1f4ea0a8e8df0) | 82,365 | 228,499 (limit 240,783) |
| carol's privacy-preserving `BurnFrom` from `Private/F8WThjdH…`, committed to `0xf3AB6cAB…` (fee-exempt) | applied | [`3eb6e8af4ef7b2c5…`](https://explorer.testnet.lez.logos.co/transaction/3eb6e8af4ef7b2c512ba77afa3984f587b6cc9c3c6cecc1106a5309e5b350c8f) | 82,368 | |

The burn tree (`8tUnHjsP…`) went from 0 leaves to 2, root `0xcaa24e7e…`.
Carol's private burn was proven with dev mode off: 258.6 s to prove and
submit, 6.06 GB peak memory, 248,358 bytes on chain. Her account, the
recipient and the secret do not occur in its bytes, and her wallet decrypts
0 wrapped ETH after it. **The testnet rewrote its tip once more.** The
private burn was read back in block 82,368. Minutes later the tip was back at
82,367: `getTransaction` returned null for about 20 minutes, and the Bedrock
node's channel held LEZ blocks up to 82,367 only. By 22:20 UTC the same
transaction was included again, in a new block 82,368. The first release
pass was stopped and resumed. No second proof of the burn was sent.

Sepolia, relayer `0xf4e827cd18597f071FB8bca9A77319AD619927D7` (also member
0), vault `0x4Ae65712A472e11842A2e1Fe61C7A52Da99c8F7a`:

| attempt | verdict | transaction | block | gas |
|---|---|---|---:|---:|
| alice's release, members signing at depth 0 | refused `NotFinal` | [`0xee8a043cf63055fe…`](https://sepolia.etherscan.io/tx/0xee8a043cf63055fe6fe3f1581e49f651c8bd87eea4351b10b43abe6e0cd63874) | 12,136,049 | 127,896 |
| alice's release, one member's signature | refused `RootNotAttested` | [`0xfa69239412d4ce8f…`](https://sepolia.etherscan.io/tx/0xfa69239412d4ce8fce089c10115d295d1316e5547acfb05ed84d140d922959a9) | 12,137,717 | 117,656 |
| **alice's release**: 0.001 ETH to `0x794Dc407…`, root at LEZ block 82,372, final 82,406 | applied | [`0x0b0a7078d74f2707…`](https://sepolia.etherscan.io/tx/0x0b0a7078d74f27072f6000ade4d35a75ba897e4bd3483996c290bc6aef975f0d) | 12,137,797 | 421,504 |
| alice's release again | refused `AlreadyReleased` | [`0xf9d0f76e6a819a04…`](https://sepolia.etherscan.io/tx/0xf9d0f76e6a819a04098e230d83d8a838cc2c9327bdf211102bc94ce8ba77e0e0) | 12,137,876 | 127,896 |
| **carol's release**: 0.001 ETH to `0xf3AB6cAB…`, root at LEZ block 82,372, final 82,412 | applied | [`0x5e7685dd356592ca…`](https://sepolia.etherscan.io/tx/0x5e7685dd356592ca2fa954bb11e694b4895ab6a0261ea4c92abeaf039602d9b2) | 12,138,027 | 421,267 |

The Bedrock blocks the members checked, on the public Bedrock testnet:
LEZ block 82,406 is inscribed in Bedrock block `5becd8ff3270d00f…` (slot
3,146,664, transaction 5), and LEZ block 82,412 in `ceb7a5e43fb71b2e…`
(slot 3,148,037, transaction 0). Each is in the walk from the node's tip
`9039c0614b43396d…` (slot 3,154,900) saved under
[`evidence/testnet-outbound/bedrock/`](../evidence/testnet-outbound/bedrock/).
No Bedrock transaction was sent by this run.

Cost: 1,837,104 of the testnet token for the public burn (228,499
execution and 1,139 storage gas at base fee 8). The private burn is
fee-exempt. The payer held 30,839,652 before the run and holds 29,002,548.
On Sepolia, the five attempts cost 0.0000012 ETH of gas.

Re-check, mutation test, and the two stages, run from a copy of the inbound
run's directory:

```console
scripts/verify-outbound.sh            # LEZ testnet and Sepolia, JSON-RPC only
scripts/verify-outbound.sh --bedrock-tools bedrock/tools/target/release/bedrock-tools   # and the node's own types
scripts/mutation-test-outbound.sh     # 27 of 27
RUN=<copy of the inbound run directory> LEE_WALLET_HOME_DIR=$RUN/wallet PAYER=<funded account> \
  PRIVATE=1 HEAVY=<optional wrapper> scripts/testnet-outbound.sh burn
RUN=… SEPOLIA_KEY_FILE=<deployer key file> SEPOLIA_RPC=<Sepolia endpoint> BEDROCK_NODE=http://127.0.0.1:8080 \
  BEDROCK_EVIDENCE=evidence/testnet-outbound/bedrock BEDROCK_TOOLS=bedrock/tools/target/release/bedrock-tools \
  REFUSALS_FOR=alice scripts/testnet-outbound.sh release
```

## Re-check (inbound)

```console
scripts/verify-testnet.sh            # LEZ testnet and Sepolia, JSON-RPC only
scripts/mutation-test-verifier.sh --manifest evidence/testnet/run.tsv --rpc https://testnet.lez.logos.co   # 16 of 16
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
The outbound run later burned alice's wrapped ETH, so three accounts the
inbound run wrote (the bridge's ETH entry, the token definition, alice's
token account) hold other bytes now. `verify-testnet.sh` and the mutation
test pass `--later evidence/testnet-outbound/run.tsv`: those three accounts
are checked against the outbound manifest, which pins their bytes now, and
the others against this one. Since then the LEZ half is 26 checks, not 29. The Ethereum endpoint must serve historical
state: the deposits are checked at their blocks. The default
`rpc.sepolia.ethpandaops.io` does. `ethereum-sepolia-rpc.publicnode.com`,
the default until 2026-11-15, now prunes everything below block 12,129,511,
so the deposit checks failed there.
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

# The private claim, measured

`Claim` as a LEZ v0.3 privacy-preserving transaction: a deposit made into
`WrapVault` on anvil is minted into the private (shielded) LEZ account the
deposit committed to, with real proving. Driver:
[`scripts/measure-private-claim.sh`](../scripts/measure-private-claim.sh)
(`executor-tests --bin measure_private`). Raw output: [`docs/raw/`](raw/),
`private-claim-*.txt`.

## What runs

Carol's deposit of 1 ETH (vectors/anvil, index 4, the fifth deposit) commits
to `carol_private()`, a private account derived from her wallet seed. Her
client builds the same `Claim` the public tests send, against the vault root
after five deposits, and proves it with `lee::execute_and_prove` (LEZ v0.3.0,
`db66590a`): the plans of `wrap_bridge`, Sextant's `sextant_lc` (the chained
`Attest`) and the token program (the chained `Mint`), and the token program's
apply to the private account, are each proven in RISC Zero and composed by
LEZ's privacy circuit into one succinct receipt. The effects on public
accounts are deferred: they travel in the transaction, and settlement applies
them.

The transaction is then settled with
`V03State::transition_from_privacy_preserving_transaction`, which calls
`ValidatedStateDiff::from_privacy_preserving_transaction`, the check the
sequencer makes: the receipt against the privacy circuit's ImageID, the
program images it claims against the deployed programs, commitments new,
nullifiers unspent, then the deferred public effects applied by each program
in the executor (Sextant's storage-proof check runs here). Finally carol's keys
decapsulate and decrypt the note, and the decrypted account must open to the
commitment the chain recorded: she holds 1 ETH of the wrapped token.

- **Programs**: the committed binaries, `wrap_bridge.bin` the reproducible
  Docker build (ImageID `a7c903ab…`), `sextant_lc.bin` (`fa229edd…`), LEZ
  v0.3.0's `token.bin`.
- **Prove** is the wall time of `execute_and_prove`, from the first program
  session to the composed receipt. Real proving: `RISC0_DEV_MODE` unset (the
  binary refuses to run with it set), r0vm 3.0.5, CPU.
- **Peak RAM** is `maximum resident set size` from `/usr/bin/time -l` over the
  whole process tree (on macOS, the largest single process, `r0vm` included;
  not a sum).
- **Bytes** is the borsh encoding of the `PrivacyPreservingTransaction`; the
  `LeeTransaction` a sequencer carries adds a one-byte tag.

Machine: Apple M4 Pro (`hw.model` Mac16,8), 14 cores, 24 GiB. No other prover
was running during either run (`r0vm` processes counted every 30 s in the raw
files: only this run's).

## Results, 2026-10-17

| run | prove (s) | peak RAM | tx bytes | of which receipt | settle (ms) |
|---|---:|---:|---:|---:|---:|
| [12:18Z](raw/private-claim-20261017T1218Z.txt) | 252.1 | 6.73 GB | 230,510 | 226,831 | 161 |
| [12:23Z](raw/private-claim-20261017T1223Z.txt) | 253.9 | 8.27 GB | 230,510 | 226,831 | 171 |

- About **4 minutes and 7 to 8 GB** on a laptop, the same order as the other
  private transactions we have measured on LEZ v0.3 (160 to 190 s, 6 to 8 GB,
  for a multisig spend). The claim is slower, plausibly because it composes
  three programs' plans and five public accounts' deferred effects; the split
  was not measured.
- **230.5 kB**, of which the receipt is 226.8 kB and the message 3.7 kB: five
  public actions (1,569 bytes of effect data, 1,484 of them Sextant's check),
  one private action with a 1,299-byte encrypted note, three program image
  claims. No signature, no nonce. A 1 MiB block holds four.
- The transaction is identical in both runs (same size, deterministic note
  randomness in the test; a wallet draws it from OS entropy).

The second run's header reads "uncommitted changes": the first run's raw file
and a fix to the driver's load logger were not committed yet. The binaries
(sha256 recorded in each file) are the committed ones.

## What is public and what is hidden

Hidden (private inputs of the proof, verified in it, absent from the
transaction):

- the **destination** (carol's private account id), the **secret**, the
  deposit's **commitment** and **leaf**, its **index** and its **Merkle path**.
  `measure_private` and `tests/private.rs` search the whole serialized
  transaction for each of them, and for every non-empty node of the path: none
  occurs;
- the **minted balance's holder**: the token program's deposit into the
  private account is applied inside the circuit, and only a new commitment
  and a note encrypted to carol's viewing key leave it;
- **who claims**: the transaction carries no signature and no public account
  of the claimant. A relayer can submit it without learning anything the
  chain does not.

Public (the deferred effects, applied in the clear by settlement):

| account | effect | reveals |
|---|---|---|
| Sextant header record | `Check(query)`: the `knownRoot` storage proof of the vault, the predicate "= 1" | the header slot, the vault, and the slot `keccak(root ‖ 0)`, hence the **root** (every root is public on Ethereum) |
| bridge config | `CheckLive { mint }` | the deployment |
| token entry | `Mint { salt, amount }` | the **token** and the **amount** |
| nullifier PDA | `Spend` (written once) | the deposit's **nullifier account**, unlinkable to the deposit without the secret |
| wrapped token definition | `MintSupply { amount }` | the supply change |

Also public: the three program image claims (that this is a bridge claim), the
private action's commitment, its nullifier, and its note's one-byte view tag.

So the claim is linkable to its deposit only within its anonymity set: the
deposits of the **same token and amount under the same root** that are not yet
claimed. In the vectors, carol's claim under the five-deposit root hides among
the three 1 ETH deposits (indices 0, 2, 4). Against a real vault, the set grows
with every deposit of that denomination before the chosen root, and the
claimant should pick the latest root (any root once known stays known, R.4).

What the private path does not hide, stated plainly:

- **Timing.** A claim soon after a deposit, with few deposits of that
  denomination since, narrows the set; the root it names bounds the deposit
  from above.
- **The destination's creation.** The nullifier of a newly created private
  account is a hash of its account id (`Nullifier::for_account_initialization`).
  Whoever knows carol's private account id (she, and anyone she gave it to,
  for instance a depositor paying her) can tell this transaction created it.
  It is a fresh account per wrapped token and its id is never published, so
  this reaches only people who already know the destination.
- **Token and amount** are public by design (fixed denominations, F.8).
- **Events.** `Claimed { nullifier }` is not emitted on the private path: the
  privacy circuit's output carries no events. Recovery (F.9) reads nullifier
  accounts, not events, so it is unaffected.

A private claim that the bridge refuses (wrong secret, wrong path, frozen
token) fails while the claimant proves it, on the claimant's machine, and
never reaches the chain (`tests/private.rs`,
`a_refused_private_claim_fails_before_it_reaches_the_chain`): unlike a refused
public claim, it publishes nothing (Privacy #5).

## What changed in the program for this

`Claim`'s sixth row may now be the committed destination itself, as well as
its associated token account: a private account cannot be an ATA (a public
PDA), and minting into the committed account directly is what lets the
destination be private. The destination is still bound by the commitment;
another account, or another account's ATA, is refused (`bridge.rs`, two
mutations in `scripts/mutation-test.sh`). The claim reads no clock, so the
per-block clock trap of private transactions does not arise; a rolling-window
cap must read the 50-block clock account.

## Limits of this measurement

- It runs at the executor level, through `V03State`, with the sequencer's
  validation function, not through a running sequencer: there is no network,
  mempool or block production, and no wallet. The prototype has no client CLI
  yet; running the same transaction on a local v0.3.1 standalone sequencer
  needs one (the deploy of the four programs, Sextant's `Init`, the bridge's
  `Init` and `Register`, then the private claim through the wallet), and is
  the next step.
- The public part's gas (Sextant's MPT check runs in settlement) is not
  reported by `from_privacy_preserving_transaction`; the public claim's
  297,502 gas is an upper bound for it.
- Anvil's 3-node account proof is smaller than a mainnet one (8 to 9 nodes):
  a mainnet claim's transaction grows by a few kilobytes of public proof.
- CI runs the two private tests in RISC Zero dev mode (stand-in receipt);
  real proving is this document.

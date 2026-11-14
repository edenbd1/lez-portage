# What a Cryptarchia light client costs in RISC Zero, measured

docs/BEDROCK-FINALITY.md (section 4) estimated the cost of a full Cryptarchia
(Bedrock) light client in the zkVM at 25-75 billion cycles a day, before the
note updates. That estimate rested on two figures nobody had measured: the
cost of one leadership proof check, and the cost of keeping the note set up to
date. This document measures both in r0vm 3.0.5 and multiplies them by rates
observed on the public testnet.

Everything here is reproducible with `scripts/pol-measure.sh`. The raw output
is in `evidence/pol-measure/`.

## 1. What is measured, and against what

**Proof of Leadership (PoL).** The Bedrock node at `3926a532` checks each
block's leadership proof with `lb_pol::verify`. The steps are:

- expand the 128-byte `CompressedGroth16Proof` (arkworks
  `deserialize_compressed`: two G1 points and one G2 point, each checked on
  the curve and in the subgroup);
- call `lb_groth16::groth16_verify`, which is
  `ark_groth16::Groth16::<Bn254, LibsnarkReduction>::verify_proof`. It takes
  the prepared verification key of the PoL circuit, from
  logos-blockchain-circuits v0.5.7 (`verification_key.json`, sha256
  `973513d9…`), and the nine public inputs.

The verification key's curve is BN254, through arkworks 0.5 (`ark-bn254`,
`ark-groth16`). Two implementations were timed:

- **node**: that exact code (arkworks 0.5) compiled for the zkVM. RISC Zero
  has no accelerator for arkworks, so this runs on the plain RV32IM machine.
- **accelerated**: the same check written on `substrate-bn`, patched to
  RISC Zero's fork `risc0/paritytech-bn` tag `v0.6.0-risczero.0`. That fork
  does its Fq/Fr arithmetic with the risc0-bigint2 precompiles (256-bit
  `modmul`, and the degree-2 extension field operations); risc0-bigint2 is
  1.4.14. The steps are the same as arkworks':
  - decompression, with the same sign rule and the same curve and subgroup
    checks;
  - the nine-input multi-scalar multiplication over `gamma_abc`;
  - one batched pairing of three pairs, (A, B), (L, -gamma) and (C, -delta),
    compared with e(alpha, beta). That target is computed once per run, as
    arkworks' prepared key holds it.

  RISC Zero 3.0.5 ships no other BN254 precompile.

Both implementations are checked natively, before any cycle is counted,
against the node's own `lb_pol::verify` and `lb_groth16::groth16_verify` on
every vector. They must give the same verdict and decompress to the same
points (`tools/pol-measure/host`, `gen`). Inside the guest, a verdict that
differs from the node's panics.

**The PoL vectors.**

- **A fresh, valid proof.** It is made by `lb_pol::prove`, the node's own
  prover: the v0.5.7 witness generator and proving key, rapidsnark. Its
  inputs are the node's own circuit test (`zk/proofs/pol/src/lib.rs`,
  `test_full_flow`). It is a real Groth16 proof under the deployed key, not a
  dev-mode one, and the node verifies it true. It is regenerated on each run,
  because rapidsnark is randomised.
- **The same proof with `latest_root + 1`.** The node refuses it. This is the
  cost of a rejection.
- **The 31 public-testnet headers** of `vectors/bedrock/testnet-lez-36401.json`
  (Bedrock block `5c6d484f…` at slot 852,908 and its 30 descendants). Their
  proofs are real, and the header gives the entropy, the slot and the leader
  key. The other five public inputs are ledger state: the epoch nonce, the
  two lottery values, `aged_root` and `latest_root`. They are not in any
  header (section 3 of BEDROCK-FINALITY.md), the saved vectors do not have
  them, and the node API does not serve them. With zeros in their place the
  node's verifier refuses all 31, and so do both measured implementations.
  The point of these vectors is that real testnet proofs decompress and run
  the same code; they cannot be accepted here.
- **Testnet header 0 with full-width stand-ins.** These are 254-bit Poseidon2
  outputs for the nonce and the roots, and the fresh proof's lottery values.
  Scalar multiplication costs follow the bits of the scalar, so this is the
  representative cost of a real check. The zero-filled testnet runs
  understate it.

The devnet (`scripts/bedrock-devnet.sh`) was not used. Its leadership proofs
are in dev mode.

**Note-set update.** The node's note set is
`lb_utxotree::UtxoTree<NoteId, Utxo, ZkHasher>`:

- a fixed-height Merkle tree, 32 levels;
- the leaf is the note id, and an empty leaf is zero;
- a node is `Poseidon2Bn254Hasher::compress([left, right])`, one Poseidon2
  permutation (BN254 Fr, width 3, x^5, 8 full and 56 partial rounds,
  jf-poseidon2);
- insertions fill the lowest free position.

A light client in a zkVM does not hold the tree. It takes a witness for each
update and checks it:

1. the leaf's position and its 32 siblings must open the old root (zero for
   an insertion, the note id for a removal);
2. the same siblings, with the new leaf, give the new root.

That is 2 x 32 permutations per note. An insertion also derives the note id,
`digest([NOTE_ID_V1, op_id, output_index, value, pk])`, which is six
permutations. The witnesses come from a real `lb_utxotree::UtxoTree` holding
1,000 notes, two insertions and two removals, and the host checks that they
give the node's roots. Inside the guest, an update that does not open the old
root or reach the new one panics.

There are three Poseidon2 implementations:

- **node**: `lb_poseidon2` itself, which is jf-poseidon2 over arkworks.
- **substrate-bn Fr**: the same rounds over the accelerated Fr. It is checked
  natively against the node's hasher.
- **bigint2**: the same rounds straight on `risc0_bigint2::field::modmul_256`
  and `modadd_256`, the checked variants, which assert that the result is
  below the modulus.

## 2. Results

r0vm 3.0.5, guest built in `risczero/risc0-guest-builder:r0.1.91.1`. The
execution run was made from the tree committed with `tools/pol-measure`; the
guest's sha256, `6905828f…`, is in `run.txt`. The
counts are cycle-count deltas (`env::cycle_count()`) around each step,
inside one execution. They are user cycles, and they do not depend on the
machine.

**One leadership proof check.**

| step | node (arkworks, RV32IM) | accelerated (substrate-bn on bigint2) |
|---|---:|---:|
| expand the proof (3 decompressions, validated) | 9,652,965 | 6,659,799 |
| prepare inputs (9-input MSM), full-width inputs | 31,842,147 | 9,334,763 |
| Miller loops (3 pairs) | 29,159,923 | 18,854,210 (pairing + compare, with the final exponentiation) |
| final exponentiation + compare | 16,926,554 | (included above) |
| **one PoL check, full-width inputs (testnet header 0)** | **87,583,189** | **34,848,995** |
| one PoL check, the fresh valid proof (small slot, nonce and key) | 79,219,826 | 32,413,774 |
| one PoL check, testnet headers with zeros (31 headers, mean) | 65,159,316 | 28,319,320 |
| once per run: prepare the key (`prepare_verifying_key`) | 41,958,805 | 11,860,431 (load key, -gamma, -delta, e(alpha, beta)) |

The accelerated check is 2.5 times cheaper than the node's code.

Of the accelerated check:

- 54% is the pairing;
- 27% is the input MSM;
- 19% is decompression. Most of it is the G2 square root and the G2
  subgroup check, a 254-bit scalar multiplication in this fork.

Neither crate caches G2 line coefficients across calls for the fixed -gamma
and -delta (arkworks' prepared key does; substrate-bn's `pairing_batch`
recomputes them). This is RISC Zero's own accelerated crate as it ships, not
a hand-tuned verifier.

**One note-set update** (2 x 32 permutations; an insertion also derives its
note id):

| | node (lb_poseidon2) | substrate-bn Fr | bigint2 |
|---|---:|---:|---:|
| one Poseidon2 permutation (`compress`) | 364,714 | 104,080 | 71,805 |
| note id digest (6 permutations) | 2,132,969 | 625,369 | 431,268 |
| open old root + compute new root (64 permutations) | 22,769,596 | 6,661,753 | 4,597,353 |
| **insertion** | **24,902,565** | **7,287,122** | **5,028,621** |
| **removal** | **22,775,793** | **6,661,761** | **4,597,361** |

**Proven.** On this machine (an M4 Pro, 14 cores, 24 GB, CPU only, shared
with other work), with a succinct receipt verified afterwards (raw output in
`evidence/pol-measure/run.txt`):

| job | total cycles (segments) | proving time | rate |
|---|---:|---:|---:|
| `pol-bn-1`: load the key once, then one accelerated PoL check of a fresh valid proof (32.4M of the 44.3M user cycles) | 47,710,208 (46) | 3,499 s | 13,600 cycles/s |

That run had load average 21-75, from other work on the machine, and a peak
resident set of 10 GB. A second proof, of the note updates, was started and
then stopped, because one proof is enough to give the rate. The prover's
journal holds the same cycle counts as the execution's: 32,413,809 for the
check.

## 3. Per day, at testnet rates

**Inputs.**

- **Blocks per day: 2,221 observed.** The 31 saved testnet headers span
  slots 852,908 to 854,075, which is 1,167 slots over 30 intervals, 38.9
  slots per block. The slot is 1 s in the node's compiled deployment
  (`nodes/node/binary/src/config/deployment/settings.yaml`). So the
  observation is one block per 38.9 s. With 30 intervals the sampling error
  is about ±18%. The nominal rate (f = 1/20) is one block per 20 s, 4,320 a
  day, and that is given as the upper case.
- **Transactions per block: 7.57 and 11.95 observed.** `bedrock-tools check`
  read 270 testnet blocks read-only (BEDROCK-FINALITY.md section 8): 1,135
  transactions in 150 blocks at the LIB, and 1,434 in 120 blocks at the tip.
- **Note operations per transaction: 2.** The one testnet transaction whose
  operations are saved is the LEZ inscription in the vector. Its 891 bytes of
  operations are the inscription and then a `Transfer` (opcode 0x00) with
  one input and one output. All 533 transactions of the saved devnet run
  have the same shape. The bodies of the other testnet blocks were not saved,
  so 2 per transaction is an assumption for the rest of the testnet traffic.
  A transfer with change would have three.
- **Per block, besides the PoL:** the epoch-nonce update
  (`update_nonce`: four absorbed elements and the padding, five
  permutations) and the voucher MMR append (about one or two permutations,
  amortised). That is about seven permutations, counted with the same
  Poseidon2 as the notes.

Not counted:

- the Blake2b header hashing: 24.3k cycles a header, measured for the anchor
  guest, section 8;
- the Ed25519 block signature;
- uncle headers' proofs: 29 uncles in the 270 blocks, about 11% more PoL
  checks if a client checks them;
- epoch-boundary reward notes;
- the free-position bookkeeping that the lowest-free-position rule needs.

**Cycles per day.**

| case | PoL | notes | per-block hashing | total |
|---|---:|---:|---:|---:|
| observed blocks (2,221), 7.57 tx/block, accelerated PoL + bigint2 Poseidon2 | 77.4 G | 161.8 G | 1.1 G | **240 G** |
| observed blocks, 11.95 tx/block, same | 77.4 G | 255.5 G | 1.1 G | **334 G** |
| observed blocks, 7.57-11.95 tx/block, substrate-bn Poseidon2 | 77.4 G | 234-370 G | 1.6 G | 313-449 G |
| observed blocks, the node's code throughout (arkworks) | 194.5 G | 801-1,266 G | 5.7 G | 1.0-1.47 T |
| nominal blocks (4,320), 7.57-11.95 tx/block, accelerated PoL + bigint2 | 150.5 G | 315-497 G | 2.2 G | 467-650 G |

So the best case measured here is **about 240-330 billion cycles per day**
of testnet. The leadership proofs alone are 77 billion. That is the top of
the old 25-75 billion estimate, at half the block rate it assumed: one check
costs 34.8M cycles, not 5-15M.

The note updates are two to three times the PoL. A light client must make
them before it can check any PoL against the roots it replays. An update
costs 64 permutations, and at 2,221 blocks a day 33,600-53,100 updates add
162-256 billion cycles. Batching the updates of one block shares the upper
levels of paths that lie close together, which saves part of that.
Removals of old notes are spread over the tree and share little.

At the proving rate measured above (13,600 cycles/s on a shared CPU), one day of testnet is
200-280 days of this machine. The light client is a GPU or proving-market job,
continuous, and it keeps growing with Bedrock's traffic.
BEDROCK-FINALITY.md section 5 reaches the same conclusion, now on measured
numbers.

## 4. Reproduce

```
scripts/pol-measure.sh            # build, generate jobs, execute all (about 2 minutes of r0vm)
scripts/pol-measure.sh --prove    # also prove pol-bn-1 and notes-bigint2
```

`tools/pol-measure/`:

- `core`: the measured code;
- `guest`: the zkVM program; its journal is a cycle report;
- `host`: `gen` builds the jobs with the node's own crates at `3926a532` and
  checks them, and `run` executes or proves.

The `gen` step needs the v0.5.7 circuit artifacts, which `lbc-pol-sys`'s
`prebuilt` feature downloads once.

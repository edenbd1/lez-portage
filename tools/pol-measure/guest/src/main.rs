// pol_measure guest: runs one `Job` and commits a `Report` of cycle counts
// (`env::cycle_count()` deltas around each step) and verdicts. A verdict that
// differs from the node's (`PolCase::expect`) or a note update that does not
// open against its old root or reach its new root panics.
#![no_main]

use std::io::Read;

use pol_measure_core::{ark, bn, p2, root_from, Job, Report};
use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

fn timed<T>(r: &mut Report, label: &str, f: impl FnOnce() -> T) -> T {
    let t0 = env::cycle_count();
    let out = f();
    let t1 = env::cycle_count();
    r.steps.push((label.to_string(), t1 - t0));
    out
}

fn main() {
    let mut stdin = env::stdin();
    let mut n = [0u8; 4];
    stdin.read_exact(&mut n).expect("a length");
    let mut buf = vec![0u8; u32::from_le_bytes(n) as usize];
    stdin.read_exact(&mut buf).expect("the input");
    let job: Job = borsh::from_slice(&buf).expect("a borsh Job");
    drop(buf);
    let mut r = Report::default();
    match job {
        Job::PolArk { vk, pvk, proofs, breakdown } => {
            let vk = timed(&mut r, "ark: deserialize vk", || ark::vk(&vk));
            let _ = timed(&mut r, "ark: prepare_verifying_key (once per run)", || ark_groth16::prepare_verifying_key(&vk));
            let pvk = timed(&mut r, "ark: deserialize prepared vk", || ark::pvk(&pvk));
            for c in &proofs {
                let inputs: Vec<_> = c.inputs.iter().map(ark::fr).collect();
                let ok = timed(&mut r, &format!("ark: verify [{}]", c.label), || ark::verify(&pvk, &c.proof, &inputs));
                assert_eq!(ok, c.expect, "{}: the node's verdict", c.label);
                r.verdicts.push((c.label.clone(), ok));
                if breakdown {
                    let p = timed(&mut r, &format!("ark: . expand proof (3 decompressions, validated) [{}]", c.label), || ark::expand(&c.proof)).expect("a proof");
                    let l = timed(&mut r, &format!("ark: . prepare_inputs (9-input MSM) [{}]", c.label), || ark::prepare_inputs(&pvk, &inputs));
                    let m = timed(&mut r, &format!("ark: . multi_miller_loop (3 pairs) [{}]", c.label), || ark::miller(&pvk, &p, l));
                    let ok2 = timed(&mut r, &format!("ark: . final_exponentiation + compare [{}]", c.label), || ark::final_exp(&pvk, m));
                    assert_eq!(ok2, ok);
                }
            }
        }
        Job::PolBn { vk, proofs, breakdown } => {
            let k = timed(&mut r, "bn: load vk, -gamma, -delta, e(alpha,beta) (once per run)", || bn::key(&vk));
            for c in &proofs {
                let inputs: Vec<_> = c.inputs.iter().map(bn::fr).collect();
                let ok = timed(&mut r, &format!("bn: verify [{}]", c.label), || bn::verify(&k, &c.proof, &inputs));
                assert_eq!(ok, c.expect, "{}: the node's verdict", c.label);
                r.verdicts.push((c.label.clone(), ok));
                if breakdown {
                    let (a, b, cc) = timed(&mut r, &format!("bn: . expand proof (3 decompressions, validated) [{}]", c.label), || bn::expand(&c.proof)).expect("a proof");
                    let l = timed(&mut r, &format!("bn: . prepare_inputs (9 scalar mults) [{}]", c.label), || bn::prepare_inputs(&k, &inputs));
                    let ok2 = timed(&mut r, &format!("bn: . pairing_batch (3 pairs) + compare [{}]", c.label), || bn::check(&k, a, b, l, cc));
                    assert_eq!(ok2, ok);
                }
            }
        }
        Job::Notes { imp, ops, permutations } => {
            if imp == 2 {
                notes_bigint2(&mut r, &ops, permutations);
            } else if imp == 0 {
                use lb_poseidon2::{Digest, Fr, Poseidon2Bn254Hasher as H};
                use ark_ff::AdditiveGroup;
                let compress = |a: Fr, b: Fr| <H as Digest>::compress(&[a, b]);
                let x = ark::fr(&ops[0].note_id);
                let _ = timed(&mut r, &format!("ark: {permutations} x Poseidon2 compress"), || {
                    let mut h = x;
                    for _ in 0..permutations {
                        h = compress(h, x);
                    }
                    h
                });
                for (i, op) in ops.iter().enumerate() {
                    let kind = if op.insert { "insert" } else { "remove" };
                    let sib: Vec<Fr> = op.siblings.iter().map(ark::fr).collect();
                    let (old, new) = (ark::fr(&op.old_root), ark::fr(&op.new_root));
                    let id = if op.insert {
                        let pre: Vec<Fr> = op.preimage.iter().map(ark::fr).collect();
                        timed(&mut r, &format!("ark: note id digest (5 inputs) [{kind} {i}]"), || <H as Digest>::digest(&pre))
                    } else {
                        ark::fr(&op.note_id)
                    };
                    assert_eq!(id, ark::fr(&op.note_id), "note id");
                    let (empty, full) = (Fr::ZERO, id);
                    let (from, to) = if op.insert { (empty, full) } else { (full, empty) };
                    let got = timed(&mut r, &format!("ark: open old root + compute new root (2 x 32 compress) [{kind} {i}]"), || {
                        assert_eq!(root_from(from, op.position, &sib, &compress), old, "old root");
                        root_from(to, op.position, &sib, &compress)
                    });
                    assert_eq!(got, new, "new root");
                    r.verdicts.push((format!("{kind} {i}"), true));
                }
            } else {
                use substrate_bn::Fr;
                let c = p2::consts_from_jf(|f| bn::fr(&ark::fr_bytes(f)));
                let (zero, one) = (Fr::zero(), Fr::one());
                let compress = |a: Fr, b: Fr| p2::compress(&c, zero, a, b);
                let x = bn::fr(&ops[0].note_id);
                let _ = timed(&mut r, &format!("bn: {permutations} x Poseidon2 compress"), || {
                    let mut h = x;
                    for _ in 0..permutations {
                        h = compress(h, x);
                    }
                    h
                });
                for (i, op) in ops.iter().enumerate() {
                    let kind = if op.insert { "insert" } else { "remove" };
                    let sib: Vec<Fr> = op.siblings.iter().map(bn::fr).collect();
                    let (old, new) = (bn::fr(&op.old_root), bn::fr(&op.new_root));
                    let id = if op.insert {
                        let pre: Vec<Fr> = op.preimage.iter().map(bn::fr).collect();
                        timed(&mut r, &format!("bn: note id digest (5 inputs) [{kind} {i}]"), || p2::digest(&c, zero, one, &pre))
                    } else {
                        bn::fr(&op.note_id)
                    };
                    assert!(id == bn::fr(&op.note_id), "note id");
                    let (from, to) = if op.insert { (zero, id) } else { (id, zero) };
                    let got = timed(&mut r, &format!("bn: open old root + compute new root (2 x 32 compress) [{kind} {i}]"), || {
                        assert!(root_from(from, op.position, &sib, &compress) == old, "old root");
                        root_from(to, op.position, &sib, &compress)
                    });
                    assert!(got == new, "new root");
                    r.verdicts.push((format!("{kind} {i}"), true));
                }
            }
        }
    }
    env::commit_slice(&borsh::to_vec(&r).unwrap());
}

// ----------------------------------------------------------------------------
// Poseidon2 straight on the risc0-bigint2 field precompiles: an Fr is eight
// little-endian u32 words, canonical (< r); every multiplication is one
// `modmul_256` call and every addition one `modadd_256` call, each checked
// below the modulus. The same rounds as `pol_measure_core::p2::permute`.

type W = [u32; 8];

const R: W = {
    // r = 21888242871839275222246405745257275088548364400416034343698204186575808495617
    let b: [u8; 32] = [
        0x01, 0x00, 0x00, 0xf0, 0x93, 0xf5, 0xe1, 0x43, 0x91, 0x70, 0xb9, 0x79, 0x48, 0xe8, 0x33, 0x28,
        0x5d, 0x58, 0x81, 0x81, 0xb6, 0x45, 0x50, 0xb8, 0x29, 0xa0, 0x31, 0xe1, 0x72, 0x4e, 0x64, 0x30,
    ];
    let mut w = [0u32; 8];
    let mut i = 0;
    while i < 8 {
        w[i] = (b[4 * i] as u32) | ((b[4 * i + 1] as u32) << 8) | ((b[4 * i + 2] as u32) << 16) | ((b[4 * i + 3] as u32) << 24);
        i += 1;
    }
    w
};

fn w(b: &[u8; 32]) -> W {
    core::array::from_fn(|i| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap()))
}

#[inline(always)]
fn mm(a: &W, b: &W) -> W {
    let mut o = [0u32; 8];
    risc0_bigint2::field::modmul_256(a, b, &R, &mut o);
    o
}

#[inline(always)]
fn ma(a: &W, b: &W) -> W {
    let mut o = [0u32; 8];
    risc0_bigint2::field::modadd_256(a, b, &R, &mut o);
    o
}

struct C2 {
    ext: Vec<[W; 3]>,
    int: Vec<W>,
}

#[inline(always)]
fn sbox(x: &W) -> W {
    let x2 = mm(x, x);
    let x4 = mm(&x2, &x2);
    mm(&x4, x)
}

#[inline(always)]
fn ext_mat(s: &mut [W; 3]) {
    let sum = ma(&ma(&s[0], &s[1]), &s[2]);
    s[0] = ma(&s[0], &sum);
    s[1] = ma(&s[1], &sum);
    s[2] = ma(&s[2], &sum);
}

#[inline(always)]
fn int_mat(s: &mut [W; 3]) {
    let sum = ma(&ma(&s[0], &s[1]), &s[2]);
    s[0] = ma(&s[0], &sum);
    s[1] = ma(&s[1], &sum);
    s[2] = ma(&ma(&s[2], &s[2]), &sum);
}

fn permute2(c: &C2, s: &mut [W; 3]) {
    ext_mat(s);
    for rc in &c.ext[..4] {
        for i in 0..3 {
            s[i] = sbox(&ma(&s[i], &rc[i]));
        }
        ext_mat(s);
    }
    for rc in &c.int {
        s[0] = sbox(&ma(&s[0], rc));
        int_mat(s);
    }
    for rc in &c.ext[4..] {
        for i in 0..3 {
            s[i] = sbox(&ma(&s[i], &rc[i]));
        }
        ext_mat(s);
    }
}

fn compress2(c: &C2, l: W, r: W) -> W {
    let mut s = [l, r, [0u32; 8]];
    permute2(c, &mut s);
    s[0]
}

fn digest2(c: &C2, xs: &[W]) -> W {
    let mut one = [0u32; 8];
    one[0] = 1;
    let mut s = [[0u32; 8]; 3];
    for x in xs.iter().chain(core::iter::once(&one)) {
        s[0] = ma(&s[0], x);
        permute2(c, &mut s);
    }
    s[0]
}

fn notes_bigint2(r: &mut Report, ops: &[pol_measure_core::NoteOp], permutations: u32) {
    let k = p2::consts_from_jf(|f| w(&ark::fr_bytes(f)));
    let c = C2 { ext: k.ext, int: k.int };
    let compress = |a: W, b: W| compress2(&c, a, b);
    let x = w(&ops[0].note_id);
    let _ = timed(r, &format!("bigint2: {permutations} x Poseidon2 compress"), || {
        let mut h = x;
        for _ in 0..permutations {
            h = compress(h, x);
        }
        h
    });
    for (i, op) in ops.iter().enumerate() {
        let kind = if op.insert { "insert" } else { "remove" };
        let sib: Vec<W> = op.siblings.iter().map(w).collect();
        let (old, new) = (w(&op.old_root), w(&op.new_root));
        let id = if op.insert {
            let pre: Vec<W> = op.preimage.iter().map(w).collect();
            timed(r, &format!("bigint2: note id digest (5 inputs) [{kind} {i}]"), || digest2(&c, &pre))
        } else {
            w(&op.note_id)
        };
        assert_eq!(id, w(&op.note_id), "note id");
        let (from, to) = if op.insert { ([0u32; 8], id) } else { (id, [0u32; 8]) };
        let got = timed(r, &format!("bigint2: open old root + compute new root (2 x 32 compress) [{kind} {i}]"), || {
            assert_eq!(root_from(from, op.position, &sib, &compress), old, "old root");
            root_from(to, op.position, &sib, &compress)
        });
        assert_eq!(got, new, "new root");
        r.verdicts.push((format!("{kind} {i}"), true));
    }
}

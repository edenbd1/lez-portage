//! What a Cryptarchia light client repeats for every Bedrock block, in two
//! implementations each: the node's own (arkworks) and RISC Zero's accelerated
//! BN254 (`substrate-bn` on risc0-bigint2). See `Cargo.toml`.
//!
//! Encodings, as the node at 3926a532 has them:
//! - a PoL proof is `CompressedGroth16Proof::to_bytes()`: `pi_a ‖ pi_b ‖ pi_c`,
//!   arkworks compressed (G1 32 bytes, G2 64 bytes, little-endian x with the
//!   sign and infinity flags in the top two bits of the last byte);
//! - a public input is an Fr in 32 little-endian bytes (`fr_to_bytes`), nine
//!   of them in `PolVerifierInput::to_inputs()` order.

use borsh::{BorshDeserialize, BorshSerialize};

pub const PROOF_LEN: usize = 128;
pub const DEPTH: usize = 32;
pub type FrLe = [u8; 32];

/// One job for the guest. The host writes it borsh-encoded, length-prefixed.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub enum Job {
    /// The node's path: arkworks `PreparedVerifyingKey` (uncompressed,
    /// serialized by the host), then for each proof
    /// `lb_groth16::groth16_verify` as `lb_pol::verify` calls it. `vk` is the
    /// unprepared key, so the guest can also time `prepare_verifying_key`
    /// once. With `breakdown`, the guest repeats each proof phase by phase.
    PolArk { vk: Vec<u8>, pvk: Vec<u8>, proofs: Vec<PolCase>, breakdown: bool },
    /// The accelerated path: substrate-bn, same key, same proofs.
    PolBn { vk: BnVk, proofs: Vec<PolCase>, breakdown: bool },
    /// Note-set updates against a Merkle witness. `imp`: 0 = the node's
    /// hasher (lb_poseidon2: jf-poseidon2 over arkworks), 1 = the same
    /// permutation over substrate-bn's accelerated Fr, 2 = the same
    /// permutation straight on risc0-bigint2's modmul_256/modadd_256.
    Notes { imp: u8, ops: Vec<NoteOp>, permutations: u32 },
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct PolCase {
    pub label: String,
    pub proof: Vec<u8>,
    pub inputs: Vec<FrLe>,
    /// What the node's verifier says (the host ran `lb_pol::verify` or
    /// `lb_groth16::groth16_verify`); the guest panics if it disagrees.
    pub expect: bool,
}

/// The verification key for substrate-bn: affine coordinates as canonical
/// big-endian Fq, G2 as (x.c0, x.c1, y.c0, y.c1).
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct BnVk {
    pub alpha_g1: [[u8; 32]; 2],
    pub beta_g2: [[u8; 32]; 4],
    pub gamma_g2: [[u8; 32]; 4],
    pub delta_g2: [[u8; 32]; 4],
    pub gamma_abc_g1: Vec<[[u8; 32]; 2]>,
}

/// One note-set update, the way a light client without the tree does it: the
/// leaf's position (bit i = 1: the node at level i is a right child), its 32
/// siblings leaf to root, the old root it must open against and the new root
/// it must produce. An insertion opens an empty leaf (zero) and writes the
/// note id; a removal opens the note id and writes zero. For an insertion the
/// note id is derived in the guest from `preimage`
/// (`NoteId = Poseidon2::digest([NOTE_ID_V1, op_id, output_index, value, pk])`).
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct NoteOp {
    pub insert: bool,
    pub preimage: Vec<FrLe>,
    pub note_id: FrLe,
    pub position: u32,
    pub siblings: Vec<FrLe>,
    pub old_root: FrLe,
    pub new_root: FrLe,
}

/// The guest's journal: each timed step and its cycle count
/// (`env::cycle_count()` deltas), then the verdicts.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Default)]
pub struct Report {
    pub steps: Vec<(String, u64)>,
    pub verdicts: Vec<(String, bool)>,
}

// ---------------------------------------------------------------- arkworks --

pub mod ark {
    use super::*;
    use ark_bn254::{Bn254, Fr, G1Affine, G2Affine};
    use ark_ec::pairing::Pairing;
    use ark_ec::CurveGroup;
    use ark_ff::{BigInteger, PrimeField};
    use ark_groth16::{r1cs_to_qap::LibsnarkReduction, Groth16, PreparedVerifyingKey, Proof, VerifyingKey};
    use ark_serialize::{CanonicalDeserialize, Compress, Validate};

    pub fn fr(b: &FrLe) -> Fr {
        let f = Fr::from_le_bytes_mod_order(b);
        assert_eq!(&f.into_bigint().to_bytes_le()[..], &b[..], "a canonical Fr");
        f
    }

    pub fn fr_bytes(f: &Fr) -> FrLe {
        f.into_bigint().to_bytes_le().try_into().unwrap()
    }

    pub fn vk(b: &[u8]) -> VerifyingKey<Bn254> {
        VerifyingKey::deserialize_with_mode(b, Compress::No, Validate::No).expect("a verifying key")
    }

    pub fn pvk(b: &[u8]) -> PreparedVerifyingKey<Bn254> {
        PreparedVerifyingKey::deserialize_with_mode(b, Compress::No, Validate::No).expect("a prepared key")
    }

    /// `Groth16Proof::try_from(&CompressedGroth16Proof)`: arkworks
    /// `deserialize_compressed`, which validates (on curve, in subgroup).
    pub fn expand(p: &[u8]) -> Option<Proof<Bn254>> {
        let a = G1Affine::deserialize_compressed(&p[0..32]).ok()?;
        let b = G2Affine::deserialize_compressed(&p[32..96]).ok()?;
        let c = G1Affine::deserialize_compressed(&p[96..128]).ok()?;
        Some(Proof { a, b, c })
    }

    /// `lb_pol::verify`: expand, then `lb_groth16::groth16_verify`
    /// (`Groth16::<E, LibsnarkReduction>::verify_proof`). An expansion error
    /// is a rejection, as `LeaderProof::verify` makes it.
    pub fn verify(pvk: &PreparedVerifyingKey<Bn254>, proof: &[u8], inputs: &[Fr]) -> bool {
        let Some(proof) = expand(proof) else { return false };
        Groth16::<Bn254, LibsnarkReduction>::verify_proof(pvk, &proof, inputs).unwrap_or(false)
    }

    pub fn prepare_inputs(pvk: &PreparedVerifyingKey<Bn254>, inputs: &[Fr]) -> <Bn254 as Pairing>::G1 {
        Groth16::<Bn254, LibsnarkReduction>::prepare_inputs(pvk, inputs).expect("nine inputs")
    }

    pub fn miller(pvk: &PreparedVerifyingKey<Bn254>, p: &Proof<Bn254>, l: <Bn254 as Pairing>::G1) -> ark_ec::pairing::MillerLoopOutput<Bn254> {
        Bn254::multi_miller_loop(
            [<Bn254 as Pairing>::G1Prepared::from(p.a), l.into_affine().into(), p.c.into()],
            [p.b.into(), pvk.gamma_g2_neg_pc.clone(), pvk.delta_g2_neg_pc.clone()],
        )
    }

    pub fn final_exp(pvk: &PreparedVerifyingKey<Bn254>, m: ark_ec::pairing::MillerLoopOutput<Bn254>) -> bool {
        Bn254::final_exponentiation(m).map(|t| t.0 == pvk.alpha_g1_beta_g2).unwrap_or(false)
    }
}

// ------------------------------------------------------------- substrate-bn --

pub mod bn {
    use super::*;
    use substrate_bn::{pairing_batch, AffineG1, AffineG2, Fq, Fq2, Fr, Group, Gt, G1, G2};

    pub fn fq(be: &[u8; 32]) -> Fq {
        Fq::from_slice(be).expect("a canonical Fq")
    }

    fn le_to_be(b: &[u8]) -> [u8; 32] {
        let mut o = [0u8; 32];
        for i in 0..32 {
            o[i] = b[31 - i];
        }
        o
    }

    pub fn fr(le: &FrLe) -> Fr {
        Fr::from_slice(&le_to_be(le)).expect("a canonical Fr")
    }

    pub fn fr_le(f: &Fr) -> FrLe {
        let mut be = [0u8; 32];
        f.into_u256().to_big_endian(&mut be).unwrap();
        le_to_be(&be)
    }

    /// A point of the key: a trusted constant, so no curve or subgroup check.
    pub fn g1(c: &[[u8; 32]; 2]) -> G1 {
        G1::new(fq(&c[0]), fq(&c[1]), Fq::one())
    }

    pub fn g2(c: &[[u8; 32]; 4]) -> G2 {
        G2::new(Fq2::new(fq(&c[0]), fq(&c[1])), Fq2::new(fq(&c[2]), fq(&c[3])), Fq2::one())
    }

    /// The key once in substrate-bn form, with -gamma, -delta and
    /// e(alpha, beta) computed once (what arkworks' prepared key holds).
    pub struct Key {
        pub gamma_abc: Vec<G1>,
        pub neg_gamma: G2,
        pub neg_delta: G2,
        pub alpha_beta: Gt,
    }

    pub fn key(vk: &BnVk) -> Key {
        Key {
            gamma_abc: vk.gamma_abc_g1.iter().map(g1).collect(),
            neg_gamma: -g2(&vk.gamma_g2),
            neg_delta: -g2(&vk.delta_g2),
            alpha_beta: pairing_batch(&[(g1(&vk.alpha_g1), g2(&vk.beta_g2))]),
        }
    }

    /// arkworks' sign rule: the flag (bit 7) is set when y > -y, comparing
    /// canonical integers (for Fq2, c1 first, then c0).
    fn fq_gt(a: Fq, b: Fq) -> bool {
        a.into_u256() > b.into_u256()
    }

    fn fq2_gt(a: Fq2, b: Fq2) -> bool {
        let (a1, b1) = (a.imaginary().into_u256(), b.imaginary().into_u256());
        if a1 != b1 {
            a1 > b1
        } else {
            a.real().into_u256() > b.real().into_u256()
        }
    }

    /// arkworks `deserialize_compressed` for G1 (cofactor 1: on curve is in
    /// the group). The point at infinity is refused, as no valid PoL proof
    /// has one.
    pub fn g1_decompress(b: &[u8]) -> Option<G1> {
        let flags = b[31] >> 6;
        if flags & 1 != 0 {
            return None;
        }
        let mut x = [0u8; 32];
        x.copy_from_slice(b);
        x[31] &= 0x3f;
        let x = Fq::from_slice(&le_to_be(&x)).ok()?;
        let y = (x * x * x + G1::b()).sqrt()?;
        let y = if fq_gt(y, -y) == (flags & 2 != 0) { y } else { -y };
        AffineG1::new(x, y).ok().map(Into::into)
    }

    /// arkworks `deserialize_compressed` for G2: x = c0 ‖ c1, the flags in
    /// c1's last byte; `AffineG2::new` checks the curve and the subgroup.
    pub fn g2_decompress(b: &[u8]) -> Option<G2> {
        let flags = b[63] >> 6;
        if flags & 1 != 0 {
            return None;
        }
        let mut c1 = [0u8; 32];
        c1.copy_from_slice(&b[32..64]);
        c1[31] &= 0x3f;
        let x = Fq2::new(Fq::from_slice(&le_to_be(&b[0..32])).ok()?, Fq::from_slice(&le_to_be(&c1)).ok()?);
        let y = (x * x * x + G2::b()).sqrt()?;
        let y = if fq2_gt(y, -y) == (flags & 2 != 0) { y } else { -y };
        AffineG2::new(x, y).ok().map(Into::into)
    }

    pub fn expand(p: &[u8]) -> Option<(G1, G2, G1)> {
        Some((g1_decompress(&p[0..32])?, g2_decompress(&p[32..96])?, g1_decompress(&p[96..128])?))
    }

    pub fn prepare_inputs(k: &Key, inputs: &[Fr]) -> G1 {
        assert_eq!(inputs.len() + 1, k.gamma_abc.len());
        let mut acc = k.gamma_abc[0];
        for (i, x) in inputs.iter().enumerate() {
            acc = acc + k.gamma_abc[i + 1] * *x;
        }
        acc
    }

    /// e(A, B) · e(L, -gamma) · e(C, -delta) == e(alpha, beta): three
    /// Miller loops and one final exponentiation, as arkworks does it.
    pub fn check(k: &Key, a: G1, b: G2, l: G1, c: G1) -> bool {
        pairing_batch(&[(a, b), (l, k.neg_gamma), (c, k.neg_delta)]) == k.alpha_beta
    }

    pub fn verify(k: &Key, proof: &[u8], inputs: &[Fr]) -> bool {
        let Some((a, b, c)) = expand(proof) else { return false };
        let l = prepare_inputs(k, inputs);
        check(k, a, b, l, c)
    }

    #[allow(unused)]
    fn _zero() -> G1 {
        G1::zero()
    }
}

// ---------------------------------------------------------------- Poseidon2 --

/// Poseidon2, width 3, x^5, 8 external and 56 internal rounds, over BN254 Fr:
/// jf-poseidon2's `permute_mut::<Poseidon2ParamsBn3, 3>` step for step
/// (initial external matrix; 4 external rounds; 56 internal; 4 external),
/// generic over the field so it runs on substrate-bn's accelerated Fr.
pub mod p2 {
    use core::ops::{Add, Mul};

    pub struct Consts<F> {
        pub ext: Vec<[F; 3]>,
        pub int: Vec<F>,
    }

    #[inline(always)]
    fn sbox<F: Copy + Mul<Output = F>>(x: F) -> F {
        let x2 = x * x;
        let x4 = x2 * x2;
        x4 * x
    }

    #[inline(always)]
    fn ext_mat<F: Copy + Add<Output = F>>(s: &mut [F; 3]) {
        let sum = s[0] + s[1] + s[2];
        s[0] = s[0] + sum;
        s[1] = s[1] + sum;
        s[2] = s[2] + sum;
    }

    #[inline(always)]
    fn int_mat<F: Copy + Add<Output = F>>(s: &mut [F; 3]) {
        let sum = s[0] + s[1] + s[2];
        s[0] = s[0] + sum;
        s[1] = s[1] + sum;
        s[2] = s[2] + s[2] + sum;
    }

    pub fn permute<F: Copy + Add<Output = F> + Mul<Output = F>>(c: &Consts<F>, s: &mut [F; 3]) {
        ext_mat(s);
        let half = c.ext.len() / 2;
        for rc in &c.ext[..half] {
            for i in 0..3 {
                s[i] = sbox(s[i] + rc[i]);
            }
            ext_mat(s);
        }
        for rc in &c.int {
            s[0] = sbox(s[0] + *rc);
            int_mat(s);
        }
        for rc in &c.ext[half..] {
            for i in 0..3 {
                s[i] = sbox(s[i] + rc[i]);
            }
            ext_mat(s);
        }
    }

    /// `Poseidon2Hasher::compress`: state [l, r, 0], one permutation, s[0].
    pub fn compress<F: Copy + Add<Output = F> + Mul<Output = F>>(c: &Consts<F>, zero: F, l: F, r: F) -> F {
        let mut s = [l, r, zero];
        permute(c, &mut s);
        s[0]
    }

    /// `Poseidon2Hasher::digest`: absorb one element per permutation into
    /// s[0], then the padding element one.
    pub fn digest<F: Copy + Add<Output = F> + Mul<Output = F>>(c: &Consts<F>, zero: F, one: F, xs: &[F]) -> F {
        let mut s = [zero, zero, zero];
        for x in xs.iter().chain(core::iter::once(&one)) {
            s[0] = s[0] + *x;
            permute(c, &mut s);
        }
        s[0]
    }

    /// The jf constants (arkworks Fr) converted to another field.
    pub fn consts_from_jf<F>(conv: impl Fn(&ark_bn254::Fr) -> F) -> Consts<F> {
        use jf_poseidon2::{constants::bn254::Poseidon2ParamsBn3, Poseidon2Params};
        let ext = <Poseidon2ParamsBn3 as Poseidon2Params<ark_bn254::Fr, 3>>::external_rc();
        let int = <Poseidon2ParamsBn3 as Poseidon2Params<ark_bn254::Fr, 3>>::internal_rc();
        let diag = <Poseidon2ParamsBn3 as Poseidon2Params<ark_bn254::Fr, 3>>::internal_mat_diag_m_1();
        // The internal matrix above is diag(1, 1, 2) + J; check jf agrees.
        use ark_ff::{AdditiveGroup, One};
        assert!(diag[0].is_one() && diag[1].is_one() && diag[2] == ark_bn254::Fr::one().double());
        assert_eq!(ext.len(), 8);
        assert_eq!(int.len(), 56);
        Consts { ext: ext.iter().map(|r| [conv(&r[0]), conv(&r[1]), conv(&r[2])]).collect(), int: int.iter().map(conv).collect() }
    }
}

/// The root a leaf opens to at `position` with `siblings` (leaf to root).
pub fn root_from<F: Copy>(leaf: F, position: u32, siblings: &[F], compress: &impl Fn(F, F) -> F) -> F {
    let mut h = leaf;
    for (i, s) in siblings.iter().enumerate() {
        h = if (position >> i) & 1 == 0 { compress(h, *s) } else { compress(*s, h) };
    }
    h
}

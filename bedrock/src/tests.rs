extern crate std;

use alloc::{vec, vec::Vec};

use ed25519_dalek::{Signer, SigningKey};

use super::*;

fn hexv(v: &serde_json::Value) -> Vec<u8> {
    hex::decode(v.as_str().unwrap()).unwrap()
}

fn h(v: &serde_json::Value) -> H256 {
    hexv(v).try_into().unwrap()
}

/// A real anchor: LEZ testnet block 36401, inscribed on channel 0101..01 in
/// public-testnet Bedrock block 5c6d484f… (tx 5 of 29), and 30 headers above
/// it, as the node returned them (vectors/bedrock, checked by bedrock-tools
/// against the node's own types).
fn testnet() -> (serde_json::Value, Params, AnchorBlock) {
    let v: serde_json::Value = serde_json::from_str(include_str!("../../vectors/bedrock/testnet-lez-36401.json")).unwrap();
    let a = &v["anchor_block"];
    let block = AnchorBlock {
        header: hexv(&a["header"]),
        uncles: hexv(&a["uncles"]),
        ops: hexv(&a["ops"]),
        tx_index: a["tx_index"].as_u64().unwrap() as u32,
        path: a["path"].as_array().unwrap().iter().map(h).collect(),
        signature: hexv(&a["signature"]),
        descendants: a["descendants"].as_array().unwrap().iter().map(hexv).collect(),
    };
    let p = Params { channel_id: h(&v["channel_id"]), signer: h(&v["signer"]) };
    (v, p, block)
}

#[test]
fn a_real_testnet_inscription_anchors_30_headers_deep() {
    let (v, p, a) = testnet();
    let r = anchor(&p, &a, 30).unwrap();
    assert_eq!(hex::encode(r.block_id), v["bedrock_block"]["id"].as_str().unwrap());
    assert_eq!(hex::encode(r.tx_hash), v["bedrock_block"]["tx_hash"].as_str().unwrap());
    assert_eq!(hex::encode(r.top_id), v["top"]["id"].as_str().unwrap());
    assert_eq!((r.block_slot, r.top_slot, r.depth), (852_908, 854_075, 30));
    // The inscription is the LEZ block: id 36401 and the hash the vector names.
    assert_eq!(u64::from_le_bytes(r.inscription.data[..8].try_into().unwrap()), 36401);
    assert_eq!(hex::encode(&r.inscription.data[40..72]), v["lez_hash"].as_str().unwrap());
    // Asking for more depth than there is.
    assert_eq!(anchor(&p, &a, 31), Err(Error::Depth));
}

fn flip(b: &mut [u8], i: usize) {
    b[i] ^= 1;
}

#[test]
fn every_check_of_the_real_anchor_fires() {
    let (_, p, a) = testnet();
    let try_ = |f: &dyn Fn(&mut AnchorBlock, &mut Params)| {
        let (mut a, mut p) = (a.clone(), p);
        f(&mut a, &mut p);
        anchor(&p, &a, 30).map(|_| ())
    };
    assert_eq!(try_(&|a, _| a.header.truncate(295)), Err(Error::HeaderLength));
    assert_eq!(try_(&|a, _| a.header[..8].copy_from_slice(&[0; 8])), Err(Error::GenesisSlot));
    assert_eq!(try_(&|a, _| a.uncles.push(0)), Err(Error::Uncles));
    assert_eq!(try_(&|a, _| a.uncles[0] = 1), Err(Error::Uncles));
    // Any byte of the header's body root, of the operations (the inscription
    // included), of a path node, or another position.
    assert_eq!(try_(&|a, _| flip(&mut a.header, 40)), Err(Error::BodyRoot));
    assert_eq!(try_(&|a, _| flip(&mut a.header, 71)), Err(Error::BodyRoot));
    for i in [0, 1, 2, 40, 100, 500, 890] {
        assert_eq!(try_(&|a, _| flip(&mut a.ops, i)), Err(Error::BodyRoot), "ops byte {i}");
    }
    assert_eq!(try_(&|a, _| a.ops.push(0)), Err(Error::BodyRoot));
    for k in 0..5 {
        assert_eq!(try_(&|a, _| flip(&mut a.path[k], 31)), Err(Error::BodyRoot), "path node {k}");
    }
    assert_eq!(try_(&|a, _| a.tx_index = 4), Err(Error::BodyRoot));
    assert_eq!(try_(&|a, _| a.tx_index = 32), Err(Error::Path));
    assert_eq!(try_(&|a, _| a.path.truncate(4)), Err(Error::BodyRoot));
    assert_eq!(try_(&|a, _| a.path.extend([[0; 32]; 6])), Err(Error::Path));
    // Another channel or signer pinned, another signature.
    assert_eq!(try_(&|_, p| p.channel_id[0] ^= 1), Err(Error::Channel));
    assert_eq!(try_(&|_, p| p.signer[0] ^= 1), Err(Error::Signer));
    for i in [0, 31, 32, 63] {
        assert_eq!(try_(&|a, _| flip(&mut a.signature, i)), Err(Error::Signature), "signature byte {i}");
    }
    assert_eq!(try_(&|a, _| a.signature.truncate(63)), Err(Error::Signature));
    // The descendants: out of order, one missing, one tampered.
    assert_eq!(try_(&|a, _| a.descendants.swap(3, 4)), Err(Error::Parent));
    assert_eq!(try_(&|a, _| { a.descendants.remove(0); }), Err(Error::Parent));
    assert_eq!(try_(&|a, _| flip(&mut a.descendants[10], 200)), Err(Error::Parent));
    assert_eq!(try_(&|a, _| flip(&mut a.descendants[0], 8)), Err(Error::Parent));
    assert_eq!(try_(&|a, _| a.descendants[29].truncate(10)), Err(Error::HeaderLength));
    assert_eq!(try_(&|a, _| { a.descendants.pop(); }), Err(Error::Depth));
}

// A synthetic block, to reach what a real one cannot: a slot that goes back,
// a single transaction, an inscription that is not the first operation.

const CHANNEL: H256 = [0x42; 32];

fn key() -> SigningKey {
    SigningKey::from_bytes(&[0x17; 32])
}

fn inscribe_ops(data: &[u8], signer: &H256, more: &[u8]) -> Vec<u8> {
    let mut o = vec![if more.is_empty() { 1 } else { 2 }, OP_CHANNEL_INSCRIBE];
    o.extend_from_slice(&CHANNEL);
    o.extend_from_slice(&(data.len() as u32).to_le_bytes());
    o.extend_from_slice(data);
    o.extend_from_slice(&[0x33; 32]);
    o.extend_from_slice(signer);
    o.extend_from_slice(more);
    o
}

fn make_header(slot: u64, parent: &H256, body_root: &H256) -> Vec<u8> {
    let mut hd = Vec::new();
    hd.extend_from_slice(&slot.to_le_bytes());
    hd.extend_from_slice(parent);
    hd.extend_from_slice(body_root);
    hd.extend_from_slice(&[0x5a; 128 + 32 + 32 + 32]);
    hd
}

/// `ops` as transaction `index` of `n`, the others made up, and `depth` headers on top.
fn synthetic(ops: Vec<u8>, sig: [u8; 64], n: usize, index: usize, depth: usize) -> AnchorBlock {
    let mut leaves: Vec<H256> = (0..n).map(|i| tx_hash(&[i as u8; 70])).collect();
    leaves[index] = tx_hash(&ops);
    let size = n.next_power_of_two();
    leaves.resize(size, [0; 32]);
    let mut path = Vec::new();
    let (mut level, mut i) = (leaves, index);
    while level.len() > 1 {
        path.push(level[i ^ 1]);
        level = level.chunks(2).map(|p| node(&p[0], &p[1])).collect();
        i /= 2;
    }
    let uncles = vec![0u8];
    let header = make_header(100, &[7; 32], &body_root(&uncles, &level[0]));
    let mut descendants = Vec::new();
    let mut top = Header::parse(&header).unwrap().id();
    for k in 0..depth {
        let d = make_header(101 + k as u64, &top, &[k as u8; 32]);
        top = Header::parse(&d).unwrap().id();
        descendants.push(d);
    }
    AnchorBlock { header, uncles, ops, tx_index: index as u32, path, signature: sig.to_vec(), descendants }
}

fn signed(data: &[u8], more: &[u8]) -> (Vec<u8>, [u8; 64], Params) {
    let k = key();
    let pk = k.verifying_key().to_bytes();
    let ops = inscribe_ops(data, &pk, more);
    let sig = k.sign(&tx_hash(&ops)).to_bytes();
    (ops, sig, Params { channel_id: CHANNEL, signer: pk })
}

#[test]
fn a_synthetic_inscription_anchors_alone_and_among_others() {
    let (ops, sig, p) = signed(b"a LEZ block", b"\x00rest-of-a-transfer");
    for (n, i) in [(1, 0), (2, 1), (5, 3), (1024, 1023)] {
        let a = synthetic(ops.clone(), sig, n, i, 3);
        let r = anchor(&p, &a, 3).unwrap();
        assert_eq!((r.inscription.data, r.depth, r.top_slot), (&b"a LEZ block"[..], 3, 103));
        assert_eq!(r.top_id, Header::parse(&a.descendants[2]).unwrap().id());
    }
    // No descendants: the block itself is the top.
    let a = synthetic(ops, sig, 2, 0, 0);
    let r = anchor(&p, &a, 0).unwrap();
    assert_eq!((r.top_id, r.depth), (r.block_id, 0));
}

#[test]
fn a_descendant_must_be_at_a_later_slot() {
    let (ops, sig, p) = signed(b"x", b"");
    let mut a = synthetic(ops, sig, 1, 0, 2);
    let parent = Header::parse(&a.descendants[0]).unwrap().id();
    a.descendants[1] = make_header(101, &parent, &[9; 32]); // the same slot as its parent
    assert_eq!(anchor(&p, &a, 2), Err(Error::Slot));
}

#[test]
fn a_signature_by_another_key_or_over_another_hash_is_refused() {
    let (ops, _, p) = signed(b"x", b"");
    let other = SigningKey::from_bytes(&[0x18; 32]).sign(&tx_hash(&ops)).to_bytes();
    assert_eq!(anchor(&p, &synthetic(ops.clone(), other, 1, 0, 0), 0), Err(Error::Signature));
    let over_ops = key().sign(&ops).to_bytes();
    assert_eq!(anchor(&p, &synthetic(ops, over_ops, 1, 0, 0), 0), Err(Error::Signature));
    // A small-order key (the identity point) with R the identity and s = 0:
    // cofactorless verify accepts it for any message; Bedrock does not.
    let mut weak = [0u8; 32];
    weak[0] = 1;
    let mut sig = [0u8; 64];
    sig[0] = 1;
    let vk = ed25519_dalek::VerifyingKey::from_bytes(&weak).unwrap();
    assert!(vk.is_weak() && ed25519_dalek::Verifier::verify(&vk, &[0; 32], &ed25519_dalek::Signature::from_bytes(&sig)).is_ok());
    assert_eq!(verify_signature(&weak, &[0; 32], &sig), Err(Error::Signature));
}

/// The difference `verify_strict` makes: a signature whose R is the identity
/// (small order), by a key that is not weak, `s = k·a`. Cofactorless `verify`
/// accepts it; `verify_strict` (and Bedrock) refuse it.
#[test]
fn a_signature_only_non_strict_verification_accepts_is_refused() {
    use sha2::{Digest as _, Sha512};
    let seed = [0x21u8; 32];
    let k = SigningKey::from_bytes(&seed);
    let a = ed25519_dalek::hazmat::ExpandedSecretKey::from(&seed).scalar;
    let pk = k.verifying_key().to_bytes();
    let msg = [0x5eu8; 32];
    let mut r = [0u8; 32];
    r[0] = 1; // the identity point, compressed
    let h = curve25519_dalek::Scalar::from_hash(Sha512::new().chain_update(r).chain_update(pk).chain_update(msg));
    let sig: [u8; 64] = [r, (h * a).to_bytes()].concat().try_into().unwrap();
    let vk = ed25519_dalek::VerifyingKey::from_bytes(&pk).unwrap();
    assert!(!vk.is_weak());
    assert!(ed25519_dalek::Verifier::verify(&vk, &msg, &ed25519_dalek::Signature::from_bytes(&sig)).is_ok(), "cofactorless verify accepts it");
    assert_eq!(verify_signature(&pk, &msg, &sig), Err(Error::Signature));
}

#[test]
fn uncles_are_in_the_body_root_and_bounded() {
    let (ops, sig, p) = signed(b"x", b"");
    let mut a = synthetic(ops, sig, 1, 0, 1);
    // One uncle: a signed header at a non-zero slot; the body root over it.
    let mut uncles = vec![1u8];
    uncles.extend_from_slice(&make_header(99, &[8; 32], &[9; 32]));
    uncles.extend_from_slice(&[0x44; 64]);
    let root = merkle_root(&tx_hash(&a.ops), 0, &[]).unwrap();
    a.header = make_header(100, &[7; 32], &body_root(&uncles, &root));
    let id = Header::parse(&a.header).unwrap().id();
    a.descendants = vec![make_header(101, &id, &[0; 32])];
    a.uncles = uncles.clone();
    assert!(anchor(&p, &a, 1).is_ok());
    // The same block claimed without its uncle: another body root.
    let mut none = a.clone();
    none.uncles = vec![0];
    assert_eq!(anchor(&p, &none, 1), Err(Error::BodyRoot));
    // An uncle at slot 0, a count past MAX_UNCLES (with as many headers).
    let mut genesis = uncles.clone();
    genesis[1..9].copy_from_slice(&[0; 8]);
    assert_eq!(check_uncles(&genesis), Err(Error::Uncles));
    let mut five = vec![5u8];
    for _ in 0..5 {
        five.extend_from_slice(&uncles[1..]);
    }
    assert_eq!(check_uncles(&five), Err(Error::Uncles));
    assert_eq!(check_uncles(&[&[4u8][..], &five[1..1 + 4 * SIGNED_HEADER_LEN]].concat()), Ok(()));
}

#[test]
fn the_inscription_length_cap() {
    let at = |n: usize| {
        let mut o = vec![1u8, OP_CHANNEL_INSCRIBE];
        o.extend_from_slice(&CHANNEL);
        o.extend_from_slice(&(n as u32).to_le_bytes());
        o.resize(o.len() + n + 64, 0x61);
        first_inscription(&o).map(|i| i.data.len())
    };
    assert_eq!(at(MAX_INSCRIPTION), Ok(MAX_INSCRIPTION));
    assert_eq!(at(MAX_INSCRIPTION + 1), Err(Error::NotAnInscription));
}

#[test]
fn only_a_first_operation_inscription_is_read() {
    let (ops, _, _) = signed(b"data", b"");
    assert!(first_inscription(&ops).is_ok());
    // An inscription after another operation, a count of 0, another opcode.
    let mut second = vec![2u8, 0x00];
    second.extend_from_slice(&ops[1..]);
    assert_eq!(first_inscription(&second), Err(Error::NotAnInscription));
    let mut zero = ops.clone();
    zero[0] = 0;
    assert_eq!(first_inscription(&zero), Err(Error::NotAnInscription));
    let mut other = ops.clone();
    other[1] = 0x10;
    assert_eq!(first_inscription(&other), Err(Error::NotAnInscription));
    // Truncated anywhere, a length past the end or past the cap, trailing bytes when alone.
    for n in 0..ops.len() {
        assert_eq!(first_inscription(&ops[..n]), Err(Error::NotAnInscription), "truncated at {n}");
    }
    let mut long = ops.clone();
    long[34..38].copy_from_slice(&5u32.to_le_bytes());
    assert_eq!(first_inscription(&long), Err(Error::NotAnInscription));
    let mut huge = ops.clone();
    huge[34..38].copy_from_slice(&((MAX_INSCRIPTION + 1) as u32).to_le_bytes());
    assert_eq!(first_inscription(&huge), Err(Error::NotAnInscription));
    let mut trailing = ops.clone();
    trailing.push(0);
    assert_eq!(first_inscription(&trailing), Err(Error::NotAnInscription));
    // With a second operation, its bytes are not read.
    let mut two = ops.clone();
    two[0] = 2;
    two.extend_from_slice(b"anything");
    assert_eq!(first_inscription(&two).unwrap().data, b"data");
}

#[test]
fn the_merkle_path_bounds() {
    let leaf = [1u8; 32];
    assert_eq!(merkle_root(&leaf, 0, &[]), Ok(leaf));
    assert_eq!(merkle_root(&leaf, 1, &[]), Err(Error::Path));
    assert_eq!(merkle_root(&leaf, 1023, &[[0; 32]; 10]).is_ok(), true);
    assert_eq!(merkle_root(&leaf, 1024, &[[0; 32]; 10]), Err(Error::Path));
    assert_eq!(merkle_root(&leaf, 0, &[[0; 32]; 11]), Err(Error::Path));
}

#[cfg(feature = "release")]
mod compose_tests {
    use alloc::{vec, vec::Vec};

    use super::*;
    use crate::compose::*;
    use lez_chain::block::*;
    use crate::H256;

    const PROGRAM: H256 = [0xb1; 32];
    const BURNS: H256 = [0xb2; 32];
    const CHAIN_PARAMS: lez_chain::Params = lez_chain::Params { program_id: PROGRAM, burns_account: BURNS };

    fn private_burn(leaf: H256) -> LeeTransaction {
        LeeTransaction::PrivacyPreserving(PrivacyPreservingTransaction {
            message: PrivacyMessage {
                public_actions: vec![PublicActionWithId {
                    account_id: BURNS,
                    effects: vec![DeferredPublicEffect {
                        program_account_id: PROGRAM,
                        shard_program_account_id: PROGRAM,
                        data: [&[lez_chain::APPEND_VARIANT][..], &leaf].concat(),
                    }],
                }],
                nonces: vec![],
                private_actions: vec![],
                block_validity_window: ValidityWindow { from: None, to: None },
                timestamp_validity_window: ValidityWindow { from: None, to: None },
                program_image_claims: vec![],
            },
            witness_set: PrivacyWitnessSet { signatures_and_public_keys: vec![], proof: vec![8; 100] },
        })
    }

    fn make_block(id: u64, prev: H256, txs: Vec<LeeTransaction>) -> Vec<u8> {
        let mut b = Block {
            header: BlockHeader { block_id: id, prev_block_hash: prev, hash: [0; 32], timestamp: 1_000 * id },
            body: BlockBody { transactions: txs },
            bedrock_status: BedrockStatus::Pending,
        };
        let raw = borsh::to_vec(&b).unwrap();
        b.header.hash = lez_chain::content_hash(&raw);
        borsh::to_vec(&b).unwrap()
    }

    fn a20(b: u8) -> [u8; 20] {
        let mut x = [0u8; 20];
        x[19] = b;
        x
    }

    /// A LEZ chain of one block with one private burn, its journal, the
    /// block inscribed with 3 headers on top, and the release opening.
    fn setup() -> (Vec<u8>, AnchorInput, wrap_core::zk::ZkReleaseInput) {
        let (pairing, secret) = ([0x11; 32], [0x55; 32]);
        let me = wrap_core::burn_leaf(&pairing, &a20(2), 7, &wrap_core::eth_commitment(&a20(3), 1, &secret));
        let lez = make_block(5, [0xaa; 32], vec![private_burn(me)]);
        let ci = lez_chain::ChainInput { self_image_id: [0x1d; 32], params: CHAIN_PARAMS, anchor_id: 4, anchor_hash: [0xaa; 32], prior: None, blocks: vec![lez.clone()] };
        let chain_journal = lez_chain::chain_journal(&ci).unwrap();
        let (ops, sig, p) = signed(&lez, b"\x00fee");
        let block = synthetic(ops, sig, 3, 2, 3);
        let input = AnchorInput { self_image_id: [0xa1; 32], channel_id: p.channel_id, signer: p.signer, chain_journal, block };
        let release = wrap_core::zk::ZkReleaseInput {
            chain_id: 31337, vault: a20(1), pairing, relayer: a20(4), token: a20(2), amount: 7, recipient: a20(3), fee: 1,
            secret, index: 0, path: wrap_core::path(&[me], 0),
        };
        (lez, input, release)
    }

    #[test]
    fn the_anchor_journal_binds_the_chain_tip_to_its_inscription() {
        let (lez, i, _) = setup();
        let j = anchor_journal(&i).unwrap();
        assert_eq!(j.len(), JOURNAL_LEN);
        let (head, chain) = parse_anchor_journal(&j).unwrap();
        assert_eq!(chain, &i.chain_journal[..]);
        assert_eq!((head.self_image_id, head.params.channel_id, head.params.signer, head.depth), ([0xa1; 32], i.channel_id, i.signer, 3));
        assert_eq!((head.block_slot, head.top_slot), (100, 103));
        assert_eq!(head.block_id, Header::parse(&i.block.header).unwrap().id());
        assert_eq!(head.top_id, Header::parse(&i.block.descendants[2]).unwrap().id());
        assert_eq!(&j[32 * HEAD_WORDS + 32 * 7..32 * HEAD_WORDS + 32 * 8], &lez[40..72]);
        assert_eq!(parse_anchor_journal(&j[..j.len() - 1]), None);
        let mut other_tag = j.clone();
        other_tag[0] ^= 1;
        assert_eq!(parse_anchor_journal(&other_tag), None);
    }

    #[test]
    fn the_inscription_must_be_the_chains_tip_and_a_lez_block() {
        let (lez, i, _) = setup();
        // The chain ends at another block (its journal says another tip).
        let other = make_block(5, [0xaa; 32], vec![]);
        let ci = lez_chain::ChainInput { self_image_id: [0x1d; 32], params: CHAIN_PARAMS, anchor_id: 4, anchor_hash: [0xaa; 32], prior: None, blocks: vec![other] };
        let j2 = AnchorInput { chain_journal: lez_chain::chain_journal(&ci).unwrap(), ..i.clone() };
        assert_eq!(anchor_journal(&j2), Err(ComposeError::NotTheTip));
        // A chain one block further: its tip is not the inscribed block either.
        let next = make_block(6, lez_chain::content_hash(&lez), vec![]);
        let ci = lez_chain::ChainInput { blocks: vec![lez.clone(), next], ..ci };
        let j3 = AnchorInput { chain_journal: lez_chain::chain_journal(&ci).unwrap(), ..i.clone() };
        assert_eq!(anchor_journal(&j3), Err(ComposeError::NotTheTip));
        // Not a chain journal.
        let j4 = AnchorInput { chain_journal: i.chain_journal[1..].to_vec(), ..i.clone() };
        assert_eq!(anchor_journal(&j4), Err(ComposeError::ChainJournal));
        // A signed inscription whose bytes are not a LEZ block, or whose hash lies.
        for data in [b"not a block".to_vec(), { let mut l = lez.clone(); l[50] ^= 1; l }, { let mut l = lez.clone(); l.push(0); l }] {
            let (ops, sig, _) = signed(&data, b"\x00fee");
            let j5 = AnchorInput { block: synthetic(ops, sig, 3, 2, 3), ..i.clone() };
            assert_eq!(anchor_journal(&j5), Err(ComposeError::NotALezBlock));
        }
        // Each Bedrock check still fires through it.
        let mut j6 = i.clone();
        j6.signer[0] ^= 1;
        assert_eq!(anchor_journal(&j6), Err(ComposeError::Bedrock(Error::Signer)));
    }

    #[test]
    fn the_release_journal_is_lez_chains_then_the_bedrock_tail() {
        let (_, i, r) = setup();
        let aj = anchor_journal(&i).unwrap();
        let j = release_journal(&r, &aj).unwrap();
        assert_eq!(j.len(), RELEASE_JOURNAL_LEN);
        assert_eq!(&j[..352 + 32 * lez_chain::RELEASE_TAIL_WORDS], &lez_chain::release_journal(&r, &i.chain_journal).unwrap()[..]);
        let w = |k: usize| &j[32 * k..32 * (k + 1)];
        let (head, _) = parse_anchor_journal(&aj).unwrap();
        assert_eq!((w(18), w(19), w(20)), (&[0xa1; 32][..], &i.channel_id[..], &i.signer[..]));
        assert_eq!((w(21), w(22)), (&head.block_id[..], &head.top_id[..]));
        assert_eq!((w(23), w(24)), (&wrap_core::word_u64(103)[..], &wrap_core::word_u64(3)[..]));
        // A burn that is not in the tree, or not an anchor journal.
        let other = wrap_core::zk::ZkReleaseInput { amount: 8, ..r.clone() };
        assert_eq!(release_journal(&other, &aj), None);
        assert_eq!(release_journal(&r, &i.chain_journal), None);
    }

    #[test]
    fn the_real_testnet_block_anchors_a_chain_journal_ending_at_it() {
        let (v, p, block) = testnet();
        let a = anchor(&p, &block, 30).unwrap();
        let lez = a.inscription.data.to_vec();
        let prev: H256 = lez[8..40].try_into().unwrap();
        let ci = lez_chain::ChainInput { self_image_id: [0x1d; 32], params: CHAIN_PARAMS, anchor_id: 36400, anchor_hash: prev, prior: None, blocks: vec![lez] };
        let chain_journal = lez_chain::chain_journal(&ci).unwrap();
        let i = AnchorInput { self_image_id: [0xa1; 32], channel_id: p.channel_id, signer: p.signer, chain_journal, block };
        let j = anchor_journal(&i).unwrap();
        let (head, _) = parse_anchor_journal(&j).unwrap();
        assert_eq!(hex::encode(head.top_id), v["top"]["id"].as_str().unwrap());
        assert_eq!(head.depth, 30);
    }
}

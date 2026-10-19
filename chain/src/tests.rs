use super::block::*;
use super::*;

const PROGRAM: H256 = [0xb1; 32];
const BURNS: H256 = [0xb2; 32];
const PARAMS: Params = Params { program_id: PROGRAM, burns_account: BURNS };

fn effect(program: H256, shard: H256, data: Vec<u8>) -> DeferredPublicEffect {
    DeferredPublicEffect { program_account_id: program, shard_program_account_id: shard, data }
}

fn append(leaf: H256) -> Vec<u8> {
    [&[APPEND_VARIANT][..], &leaf].concat()
}

fn private_tx(actions: Vec<PublicActionWithId>) -> LeeTransaction {
    LeeTransaction::PrivacyPreserving(PrivacyPreservingTransaction {
        message: PrivacyMessage {
            public_actions: actions,
            nonces: vec![],
            private_actions: vec![PrivateAction {
                nullifier: [1; 32],
                root: [2; 32],
                commitment: [3; 32],
                encrypted_post_state: EncryptedAccountData { ciphertext: vec![4; 40], epk: vec![5; 33], view_tag: 6 },
            }],
            block_validity_window: ValidityWindow { from: None, to: Some(9) },
            timestamp_validity_window: ValidityWindow { from: None, to: None },
            program_image_claims: vec![ProgramImageClaim::Disclosed { account_id: PROGRAM, image_id: [7; 8] }],
        },
        witness_set: PrivacyWitnessSet { signatures_and_public_keys: vec![], proof: vec![8; 1000] },
    })
}

fn private_burn(leaf: H256) -> LeeTransaction {
    private_tx(vec![
        PublicActionWithId { account_id: [0xc0; 32], effects: vec![effect(PROGRAM, PROGRAM, vec![1, 0])] },
        PublicActionWithId { account_id: BURNS, effects: vec![effect(PROGRAM, PROGRAM, append(leaf))] },
    ])
}

/// A public `Burn` carrying the same leaf: left out, since a public
/// transaction may have reverted and still be in the block.
fn public_tx(data: Vec<u8>) -> LeeTransaction {
    LeeTransaction::Public(PublicTransaction {
        message: PublicMessage {
            program_account_id: PROGRAM,
            shard_selectors: vec![ProgramShardSelector { account_id: BURNS, program_account_id: PROGRAM }],
            nonces: vec![1],
            instruction_data: data,
            fee: Some(FeeDeclaration { payer: [9; 32], gas_limit: 10, tip: 0, max_fee: 100 }),
        },
        witness_set: PublicWitnessSet { signatures_and_public_keys: vec![(Signature { value: [1; 64] }, PublicKey([2; 32]))] },
    })
}

fn make_block(id: u64, prev: H256, txs: Vec<LeeTransaction>) -> Vec<u8> {
    let mut b = Block {
        header: BlockHeader { block_id: id, prev_block_hash: prev, hash: [0; 32], timestamp: 1_000 * id },
        body: BlockBody { transactions: txs },
        bedrock_status: BedrockStatus::Pending,
    };
    let raw = borsh::to_vec(&b).unwrap();
    b.header.hash = content_hash(&raw);
    borsh::to_vec(&b).unwrap()
}

fn hash_of(raw: &[u8]) -> H256 {
    borsh::from_slice::<Block>(raw).unwrap().header.hash
}

fn leaf(i: u8) -> H256 {
    wrap_core::sha256(&[b"leaf", &[i]])
}

/// Three blocks after the anchor: a public burn (left out), two private burns
/// (one per block), an unrelated private transaction.
fn chain() -> (H256, Vec<Vec<u8>>) {
    let anchor = [0xaa; 32];
    let b1 = make_block(5, anchor, vec![public_tx(append(leaf(0))), private_burn(leaf(1))]);
    let b2 = make_block(6, hash_of(&b1), vec![private_tx(vec![PublicActionWithId { account_id: [0xc1; 32], effects: vec![effect(PROGRAM, PROGRAM, append(leaf(9)))] }])]);
    let b3 = make_block(7, hash_of(&b2), vec![private_burn(leaf(2)), public_tx(vec![])]);
    (anchor, vec![b1, b2, b3])
}

#[test]
fn the_tree_holds_the_private_burns_in_chain_order() {
    let (anchor, blocks) = chain();
    let mut s = ChainState::at_anchor(4, anchor);
    assert_eq!(apply_block(&mut s, &PARAMS, &blocks[0]), Ok(vec![leaf(1)]));
    assert_eq!(apply_block(&mut s, &PARAMS, &blocks[1]), Ok(vec![]));
    assert_eq!(apply_block(&mut s, &PARAMS, &blocks[2]), Ok(vec![leaf(2)]));
    assert_eq!(s.tree.root, wrap_core::root(&[leaf(1), leaf(2)]));
    assert_eq!((s.tip_id, s.tip_hash), (7, hash_of(&blocks[2])));
}

#[test]
fn the_hash_is_the_contents_hash_and_the_status_byte_is_not_in_it() {
    let (_, blocks) = chain();
    let mut b: Block = borsh::from_slice(&blocks[0]).unwrap();
    let h = b.header.hash;
    b.bedrock_status = BedrockStatus::Finalized;
    let raw = borsh::to_vec(&b).unwrap();
    assert_eq!(content_hash(&raw), h);
    // The hash is sha256(PREFIX ‖ borsh(HashableBlockData)), recomputed field by field.
    let hashable = [
        &b.header.block_id.to_le_bytes()[..],
        &b.header.prev_block_hash,
        &b.header.timestamp.to_le_bytes(),
        &borsh::to_vec(&b.body.transactions).unwrap(),
    ]
    .concat();
    assert_eq!(wrap_core::sha256(&[BLOCK_PREFIX, &hashable]), h);
}

#[test]
fn a_tampered_block_is_refused_and_leaves_the_state_unchanged() {
    let (anchor, blocks) = chain();
    let start = ChainState::at_anchor(4, anchor);
    // A changed transaction under the old header hash.
    let mut b: Block = borsh::from_slice(&blocks[0]).unwrap();
    b.body.transactions[1] = private_burn(leaf(7));
    let mut s = start.clone();
    assert_eq!(apply_block(&mut s, &PARAMS, &borsh::to_vec(&b).unwrap()), Err(ChainError::HashMismatch { block_id: 5 }));
    assert_eq!(s, start);
    // A flipped byte anywhere in the hashed part.
    for at in [0usize, 20, 75, 90, blocks[0].len() - 2] {
        let mut raw = blocks[0].clone();
        raw[at] ^= 1;
        let mut s = start.clone();
        assert!(apply_block(&mut s, &PARAMS, &raw).is_err(), "byte {at}");
        assert_eq!(s, start);
    }
    // Trailing bytes, a truncated block.
    let mut s = start.clone();
    assert_eq!(apply_block(&mut s, &PARAMS, &[&blocks[0][..], &[0]].concat()), Err(ChainError::Undecodable));
    assert_eq!(apply_block(&mut s, &PARAMS, &blocks[0][..blocks[0].len() - 1]), Err(ChainError::Undecodable));
    assert_eq!(s, start);
}

#[test]
fn a_block_must_extend_the_tip() {
    let (anchor, blocks) = chain();
    let mut s = ChainState::at_anchor(4, anchor);
    // Skipping a block, replaying one, or another parent.
    assert_eq!(apply_block(&mut s, &PARAMS, &blocks[1]), Err(ChainError::NotNext { expected_id: 5, got_id: 6 }));
    let fork = make_block(5, [0xbb; 32], vec![private_burn(leaf(1))]);
    assert_eq!(apply_block(&mut s, &PARAMS, &fork), Err(ChainError::BrokenLink { block_id: 5 }));
    apply_block(&mut s, &PARAMS, &blocks[0]).unwrap();
    assert_eq!(apply_block(&mut s, &PARAMS, &blocks[0]), Err(ChainError::NotNext { expected_id: 6, got_id: 5 }));
    // A block of the right height whose parent is not the tip.
    let other = make_block(6, [0xcc; 32], vec![]);
    assert_eq!(apply_block(&mut s, &PARAMS, &other), Err(ChainError::BrokenLink { block_id: 6 }));
}

#[test]
fn only_the_bridges_append_on_its_burn_tree_counts() {
    let anchor = [0xaa; 32];
    let wrong = |program, shard, account, data| {
        make_block(5, anchor, vec![private_tx(vec![PublicActionWithId { account_id: account, effects: vec![effect(program, shard, data)] }])])
    };
    for raw in [
        wrong([0xdd; 32], PROGRAM, BURNS, append(leaf(1))), // another program
        wrong(PROGRAM, [0xdd; 32], BURNS, append(leaf(1))), // another shard
        wrong(PROGRAM, PROGRAM, [0xdd; 32], append(leaf(1))), // another account
        wrong(PROGRAM, PROGRAM, BURNS, [&[APPEND_VARIANT - 1][..], &leaf(1)].concat()), // another effect
        wrong(PROGRAM, PROGRAM, BURNS, [&append(leaf(1))[..], &[0]].concat()), // not an Append's encoding
    ] {
        let mut s = ChainState::at_anchor(4, anchor);
        assert_eq!(apply_block(&mut s, &PARAMS, &raw), Ok(vec![]));
        assert_eq!(s.tree.next_index, 0);
    }
}

#[test]
fn the_journal_round_trips_and_a_prior_continues_the_chain() {
    let (anchor, blocks) = chain();
    let me = [0x1d; 32];
    let all = ChainInput { self_image_id: me, params: PARAMS, anchor_id: 4, anchor_hash: anchor, prior: None, blocks: blocks.clone() };
    let j = chain_journal(&all).unwrap();
    assert_eq!(j.len(), JOURNAL_LEN);
    let (id, p, s) = parse_journal(&j).unwrap();
    assert_eq!((id, p, s.tip_id, s.tree.root), (me, PARAMS, 7, wrap_core::root(&[leaf(1), leaf(2)])));
    assert_eq!(&j[32 * 9..32 * 10], &s.tree.root);
    // In two steps: the first journal is the second's prior.
    let first = chain_journal(&ChainInput { blocks: blocks[..1].to_vec(), ..all.clone() }).unwrap();
    let second = chain_journal(&ChainInput { prior: Some(first.clone()), blocks: blocks[1..].to_vec(), ..all.clone() }).unwrap();
    assert_eq!(second, j);
    // A prior of another guest, deployment or anchor does not continue this chain.
    for bad in [
        ChainInput { self_image_id: [0x1e; 32], ..all.clone() },
        ChainInput { params: Params { program_id: [0; 32], ..PARAMS }, ..all.clone() },
        ChainInput { anchor_hash: [0; 32], ..all.clone() },
    ] {
        let step = ChainInput { prior: Some(first.clone()), blocks: blocks[1..].to_vec(), ..bad };
        assert_eq!(chain_journal(&step), Err(ChainError::ForeignPrior));
    }
    let mut torn = first.clone();
    torn[0] ^= 1;
    assert_eq!(chain_journal(&ChainInput { prior: Some(torn), blocks: blocks[1..].to_vec(), ..all.clone() }), Err(ChainError::ForeignPrior));
    // Blocks out of order fail.
    let mut swapped = blocks.clone();
    swapped.swap(1, 2);
    assert!(chain_journal(&ChainInput { blocks: swapped, ..all }).is_err());
}

#[test]
fn the_release_tail_is_seven_words() {
    let s = ChainState::at_anchor(4, [0xaa; 32]);
    let t = release_tail(&[0x1d; 32], &PARAMS, &s);
    assert_eq!(t.len(), 32 * RELEASE_TAIL_WORDS);
    assert_eq!(&t[32 * 3..32 * 4], &wrap_core::word_u64(4));
}

/// The guest's allocation-free scan and the typed decoding agree: on every
/// block of the test chain, on every truncation of one, and on every
/// single-byte change of one (to a few values each): both accept or both
/// refuse, and when they accept they find the same header and leaves.
#[test]
fn the_scan_agrees_with_the_typed_decoding() {
    let (_, blocks) = chain();
    let small = make_block(
        9,
        [1; 32],
        vec![
            private_tx(vec![PublicActionWithId { account_id: BURNS, effects: vec![effect(PROGRAM, PROGRAM, append(leaf(3)))] }]),
            public_tx(vec![1, 2, 3]),
        ],
    );
    let agree = |raw: &[u8]| {
        let typed = borsh::from_slice::<Block>(raw).ok().map(|b| (b.header.block_id, b.header.prev_block_hash, b.header.hash, private_burn_leaves(&b, &PARAMS)));
        let scanned = scan::scan_block(raw, &PARAMS).ok().map(|(h, l)| (h.block_id, h.prev_block_hash, h.hash, l));
        assert_eq!(typed, scanned, "{}", hex::encode(raw));
    };
    for raw in blocks.iter().chain([&small]) {
        agree(raw);
        assert!(scan::scan_block(raw, &PARAMS).is_ok());
    }
    for n in 0..small.len() {
        agree(&small[..n]);
    }
    agree(&[&small[..], &[0]].concat());
    for at in 0..small.len() {
        for v in [0u8, 1, 2, 3, 0x21, 0xff] {
            let mut raw = small.clone();
            raw[at] = v;
            agree(&raw);
        }
    }
}

#[test]
fn the_guest_stream_is_the_input() {
    let (anchor, blocks) = chain();
    let i = ChainInput { self_image_id: [1; 32], params: PARAMS, anchor_id: 4, anchor_hash: anchor, prior: None, blocks: blocks.clone() };
    let s = encode_stream(&i);
    let n = u32::from_le_bytes(s[..4].try_into().unwrap()) as usize;
    let head: ChainHead = borsh::from_slice(&s[4..4 + n]).unwrap();
    assert_eq!(head, i.head());
    let mut at = 4 + n;
    for b in &blocks {
        let len = u32::from_le_bytes(s[at..at + 4].try_into().unwrap()) as usize;
        assert_eq!(&s[at + 4..at + 4 + len], &b[..]);
        at += 4 + len;
    }
    assert_eq!(at, s.len());
}

#[cfg(feature = "release")]
#[test]
fn the_release_journal_is_the_preimage_then_the_chain_tail() {
    use wrap_core::zk::ZkReleaseInput;
    let a = |b: u8| {
        let mut x = [0u8; 20];
        x[19] = b;
        x
    };
    let pairing = [0x11; 32];
    let secret = [0x55; 32];
    let me = wrap_core::burn_leaf(&pairing, &a(2), 7, &wrap_core::eth_commitment(&a(3), 1, &secret));
    let anchor = [0xaa; 32];
    let b1 = make_block(5, anchor, vec![private_burn(leaf(1)), private_burn(me)]);
    let i = ChainInput { self_image_id: [0x1d; 32], params: PARAMS, anchor_id: 4, anchor_hash: anchor, prior: None, blocks: vec![b1.clone()] };
    let cj = chain_journal(&i).unwrap();
    let input = ZkReleaseInput {
        chain_id: 31337, vault: a(1), pairing, relayer: a(4), token: a(2), amount: 7, recipient: a(3), fee: 1,
        secret, index: 1, path: wrap_core::path(&[leaf(1), me], 1),
    };
    let j = release_journal(&input, &cj).unwrap();
    assert_eq!(j.len(), 352 + 32 * RELEASE_TAIL_WORDS);
    assert_eq!(&j[..352], &wrap_core::zk::zk_release_journal(&input).unwrap()[..]);
    assert_eq!(&j[352..384], &[0x1d; 32]);
    assert_eq!(&j[32 * 17..], &hash_of(&b1));
    // The opening of a burn that is not in the chain's tree: no journal.
    let other = ZkReleaseInput { index: 0, path: wrap_core::path(&[me], 0), ..input.clone() };
    assert_eq!(release_journal(&other, &cj), None);
    // Not a chain journal.
    assert_eq!(release_journal(&input, &cj[..cj.len() - 1]), None);
}

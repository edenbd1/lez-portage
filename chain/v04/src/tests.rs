use std::collections::{BTreeMap, BTreeSet};

use crate::block::*;
use super::*;

const PROGRAM: H256 = [0xb1; 32];
const BURNS: H256 = [0xb2; 32];
const PAIRING: H256 = [0xb3; 32];
const PARAMS: Params = Params { program_id: PROGRAM, burns_account: BURNS, pairing: PAIRING };

fn actor(a: H256, p: H256) -> Actor {
    Actor { account_id: a, program_account_id: p }
}

/// `Message::Burn`'s first fields, then whatever follows (the program decodes the rest).
fn burn_message(eth_token: [u8; 20], amount: u128, eth_commitment: H256) -> Vec<u8> {
    [&[BURN_VARIANT][..], &eth_token, &amount.to_le_bytes(), &eth_commitment, &[0xee; 120]].concat()
}

fn leaf(eth_token: [u8; 20], amount: u128, eth_commitment: H256) -> H256 {
    wrap_core::burn_leaf(&PAIRING, &eth_token, amount, &eth_commitment)
}

fn context() -> PublicExecutionContext {
    PublicExecutionContext {
        actors: BTreeSet::from([actor(BURNS, PROGRAM), actor([0xc0; 32], PROGRAM), actor([0xc1; 32], [0xd0; 32])]),
        authorized_accounts: BTreeSet::from([[0x11; 32]]),
        promotions: BTreeSet::from([0, 3]),
    }
}

fn public_tx(to: Actor, message: Vec<u8>) -> LeeTransaction {
    LeeTransaction::Public(PublicTransaction {
        message: PublicMessage {
            public_context: context(),
            execution: PublicExecution { root: RootCall { to, message }, fee: Some(FeeDeclaration { payer: [0x11; 32], gas_limit: 10, tip: 0, max_fee: 100 }) },
            nonces: BTreeMap::from([([0x11; 32], 7)]),
            admission_evidence: vec![PublicAccountEvidence::Key([0x12; 32]), PublicAccountEvidence::Pda { program: PROGRAM, seed: [0x13; 32] }],
        },
        witness_set: PublicWitnessSet { signatures_and_public_keys: vec![(Signature { value: [1; 64] }, [2; 32])] },
    })
}

fn note() -> EncryptedNote {
    EncryptedNote { ciphertext: vec![4; 40], epk: vec![5; 33] }
}

fn delivery() -> Delivery {
    Delivery {
        from: actor([0x21; 32], PROGRAM),
        to: actor(BURNS, PROGRAM),
        message: vec![9, 9],
        inherited_authorizations: BTreeSet::from([[0x22; 32], [0x23; 32]]),
        inherits_entry_authorizations: true,
        pda_seeds: BTreeSet::from([[0x24; 32]]),
    }
}

fn private_tx(public_root: Option<RootCall>) -> LeeTransaction {
    let action = PrivateAction { nullifier: [1; 32], root: [2; 32], commitment: [3; 32], encrypted_post_state: note() };
    LeeTransaction::PrivacyPreserving(PrivacyPreservingTransaction {
        message: PrivacyMessage {
            public_context: context(),
            execution: ProvenExecution {
                boundary: vec![BoundaryStep::PrivateToPublic(delivery()), BoundaryStep::EndPublicSubtree, BoundaryStep::PublicToPrivate(delivery()), BoundaryStep::EndPrivateSubtree],
                async_messages: vec![SealedMessage { commitment: [6; 32], note: note() }],
                recovery_bindings: vec![RecoveryBinding { address: [7; 32], note: note() }],
                public_root,
                private_actions: vec![action.clone()],
                validity: ValidityWindows { blocks: ValidityWindow { from: None, to: Some(9) }, timestamps: ValidityWindow { from: Some(1), to: None } },
                program_image_claims: vec![ProgramImageClaim::Disclosed { account_id: PROGRAM, image_id: [7; 8] }, ProgramImageClaim::Undisclosed { root: [8; 32] }],
                fee: Some(ProvenFee { refund_to: Some([0x31; 32]), fallback: Some(FeeFallback { action, boundary: vec![BoundaryStep::EndPublicSubtree] }) }),
            },
            nonces: BTreeMap::new(),
            admission_evidence: vec![],
        },
        witness_set: PrivacyWitnessSet { signatures_and_public_keys: vec![], proof: vec![8; 1000] },
    })
}

/// The forced fee and clock transactions every block ends with (any public shape here).
fn system() -> Vec<LeeTransaction> {
    vec![public_tx(actor([0xf0; 32], [0xf1; 32]), vec![0]), public_tx(actor([0xf2; 32], [0xf3; 32]), vec![1])]
}

fn make_block(id: u64, prev: H256, mut txs: Vec<LeeTransaction>, reverted: Vec<u64>) -> Vec<u8> {
    txs.extend(system());
    let mut b = Block {
        header: BlockHeader { block_id: id, prev_block_hash: prev, hash: [0; 32], timestamp: 1_000 * id },
        body: BlockBody { transactions: txs, reverted },
        bedrock_status: BedrockStatus::Pending,
    };
    let raw = borsh::to_vec(&b).unwrap();
    b.header.hash = lez_chain::content_hash(&raw);
    borsh::to_vec(&b).unwrap()
}

#[test]
fn the_hash_is_over_the_hashable_data_including_the_reverted_list() {
    #[derive(BorshSerialize)]
    struct Hashable<'a> {
        block_id: u64,
        prev: H256,
        timestamp: u64,
        transactions: &'a Vec<LeeTransaction>,
        reverted: &'a Vec<u64>,
    }
    let raw = make_block(5, [9; 32], vec![public_tx(actor(BURNS, PROGRAM), burn_message([1; 20], 5, [2; 32]))], vec![0]);
    let b: Block = borsh::from_slice(&raw).unwrap();
    let h = Hashable { block_id: 5, prev: [9; 32], timestamp: 5_000, transactions: &b.body.transactions, reverted: &b.body.reverted };
    let want = wrap_core::sha256(&[lez_chain::BLOCK_PREFIX, &borsh::to_vec(&h).unwrap()]);
    assert_eq!(b.header.hash, want);
    // The list is hashed: un-reverting the burn changes the hash.
    let mut forged = b.clone();
    forged.body.reverted.clear();
    assert_ne!(lez_chain::content_hash(&borsh::to_vec(&forged).unwrap()), want);
}

#[test]
fn a_public_burn_counts_unless_it_reverted() {
    let (a, b) = (leaf([1; 20], 5, [2; 32]), leaf([3; 20], 6, [4; 32]));
    let txs = vec![
        public_tx(actor(BURNS, PROGRAM), burn_message([1; 20], 5, [2; 32])),
        public_tx(actor(BURNS, PROGRAM), burn_message([3; 20], 6, [4; 32])),
        public_tx(actor(BURNS, PROGRAM), burn_message([5; 20], 7, [6; 32])),
    ];
    let raw = make_block(1, [0; 32], txs, vec![2]);
    let (_, leaves) = scan::scan_block(&raw, &PARAMS).unwrap();
    assert_eq!(leaves, vec![a, b]);
    assert_eq!(burn_leaves(&borsh::from_slice(&raw).unwrap(), &PARAMS), leaves);
}

#[test]
fn only_a_burn_root_call_to_this_deployments_tree_counts() {
    let txs = vec![
        public_tx(actor([0xc0; 32], PROGRAM), burn_message([1; 20], 5, [2; 32])), // another account
        public_tx(actor(BURNS, [0xd0; 32]), burn_message([1; 20], 5, [2; 32])),   // another program
        public_tx(actor(BURNS, PROGRAM), [&[4u8][..], &[0; 80]].concat()),        // another variant
        public_tx(actor(BURNS, PROGRAM), vec![BURN_VARIANT; 10]),                 // too short to be a Burn
        private_tx(None),                                                         // a private root
    ];
    let raw = make_block(1, [0; 32], txs, vec![]);
    assert_eq!(scan::scan_block(&raw, &PARAMS).unwrap().1, Vec::<H256>::new());
}

#[test]
fn a_private_transactions_public_root_burn_counts_unless_it_reverted() {
    let root = || Some(RootCall { to: actor(BURNS, PROGRAM), message: burn_message([1; 20], 5, [2; 32]) });
    let raw = make_block(1, [0; 32], vec![private_tx(root()), private_tx(root())], vec![1]);
    let (_, leaves) = scan::scan_block(&raw, &PARAMS).unwrap();
    assert_eq!(leaves, vec![leaf([1; 20], 5, [2; 32])]);
    assert_eq!(burn_leaves(&borsh::from_slice(&raw).unwrap(), &PARAMS), leaves);
}

#[test]
fn blocks_chain_and_the_tree_grows() {
    let b1 = make_block(11, [7; 32], vec![public_tx(actor(BURNS, PROGRAM), burn_message([1; 20], 5, [2; 32]))], vec![]);
    let h1 = borsh::from_slice::<Block>(&b1).unwrap().header.hash;
    let b2 = make_block(12, h1, vec![private_tx(None), public_tx(actor(BURNS, PROGRAM), burn_message([3; 20], 6, [4; 32]))], vec![]);
    let mut s = ChainState::at_anchor(10, [7; 32]);
    assert_eq!(apply_block(&mut s, &PARAMS, &b1).unwrap().len(), 1);
    assert_eq!(apply_block(&mut s, &PARAMS, &b2).unwrap().len(), 1);
    assert_eq!(s.tree.root, wrap_core::root(&[leaf([1; 20], 5, [2; 32]), leaf([3; 20], 6, [4; 32])]));
    assert_eq!(s.tip_id, 12);
    // Replayed, out of order, or on another tip: refused, state unchanged.
    let before = s.clone();
    assert!(matches!(apply_block(&mut s, &PARAMS, &b2), Err(ChainError::NotNext { .. })));
    let mut other = ChainState::at_anchor(10, [8; 32]);
    assert!(matches!(apply_block(&mut other, &PARAMS, &b1), Err(ChainError::BrokenLink { .. })));
    assert_eq!(s, before);
    let head = ChainHead { self_image_id: [0x77; 32], params: PARAMS, anchor_id: 10, anchor_hash: [7; 32], blocks: 2 };
    let j = chain_journal(&head, &[b1, b2]).unwrap();
    assert_eq!(j.len(), JOURNAL_LEN);
    assert_eq!(&j[32 * 10..32 * 11], &s.tree.root);
}

/// Every malformation the typed decoder refuses, the scan refuses too.
#[test]
fn the_scan_refuses_what_borsh_refuses() {
    let good = make_block(1, [0; 32], vec![public_tx(actor(BURNS, PROGRAM), burn_message([1; 20], 5, [2; 32])), private_tx(None)], vec![0]);
    assert!(scan::scan_block(&good, &PARAMS).is_ok());
    let both = |raw: &[u8]| (borsh::from_slice::<Block>(raw).is_err(), scan::scan_block(raw, &PARAMS).is_err());
    // Trailing byte; truncation at every length.
    assert_eq!(both(&[&good[..], &[0]].concat()), (true, true));
    for n in 0..good.len() {
        assert_eq!(both(&good[..n]), (true, true), "truncated at {n}");
    }
    // A flipped byte anywhere: the scan never accepts what borsh refuses.
    for i in 0..good.len().min(4000) {
        for x in [0x01u8, 0x80, 0xff] {
            let mut bad = good.clone();
            bad[i] ^= x;
            let (typed, scan) = both(&bad);
            if typed {
                assert!(scan, "byte {i} ^ {x:#x}: borsh refuses, the scan accepts");
            }
        }
    }
}

#[test]
fn unsorted_sets_and_bad_reverted_lists_are_refused() {
    let tx = public_tx(actor(BURNS, PROGRAM), burn_message([1; 20], 5, [2; 32]));
    let mut b: Block = borsh::from_slice(&make_block(1, [0; 32], vec![tx], vec![])).unwrap();
    let ok = borsh::to_vec(&b).unwrap();
    assert!(scan::scan_block(&ok, &PARAMS).is_ok());
    // reverted: a system transaction's index, out of range, or not ascending.
    for r in [vec![1], vec![2], vec![9], vec![0, 0]] {
        b.body.reverted = r.clone();
        assert!(scan::scan_block(&borsh::to_vec(&b).unwrap(), &PARAMS).is_err(), "{r:?}");
    }
    // An actor set out of order: swap the first two actors' bytes in place.
    let raw = make_block(1, [0; 32], vec![public_tx(actor(BURNS, PROGRAM), vec![0])], vec![]);
    let at = 80 + 4 + 1 + 4; // header, tx count, tag, actor count
    let mut bad = raw.clone();
    let (x, y) = (raw[at..at + 64].to_vec(), raw[at + 64..at + 128].to_vec());
    bad[at..at + 64].copy_from_slice(&y);
    bad[at + 64..at + 128].copy_from_slice(&x);
    // borsh (without `de_strict_order`, as LEZ builds it) collects such a set
    // into its sorted form, so it re-encodes, and hashes, to other bytes than
    // these: the scan, which hashes the bytes it read, accepts the canonical
    // (ascending) encoding only, the one LEZ itself writes.
    let reread = borsh::to_vec(&borsh::from_slice::<Block>(&bad).unwrap()).unwrap();
    assert_ne!(reread, bad);
    assert!(scan::scan_block(&bad, &PARAMS).is_err());
}

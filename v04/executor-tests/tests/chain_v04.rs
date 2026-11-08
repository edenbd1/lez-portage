//! chain/src/v04 on real LEZ v0.4.0-rc1 blocks: blocks 18 to 28 of a local
//! standalone v0.4.0-rc1 sequencer (v04/scripts/e2e-seq.sh), saved as the
//! sequencer's `getBlock` returned them (vectors/lez-blocks-v04, run.json:
//! what each transaction was and whether the sequencer reverted it).
//!
//! What it shows: the block mirror is LEZ's layout byte for byte (the
//! reverted list included), the hash formula is LEZ's, and the burns the
//! program actually applied, and only those, are derivable from the blocks
//! alone: two public burns applied, two public burns included in blocks but
//! reverted, and the tree rebuilt from the blocks is the program's own.

use std::str::FromStr;

use lee::AccountId;
use lez_chain::v04;
use serde_json::Value;
use wrap_executor_tests_v04::root_dir;

fn run() -> Value {
    serde_json::from_slice(&std::fs::read(root_dir().join("vectors/lez-blocks-v04/run.json")).unwrap()).unwrap()
}

fn blocks() -> Vec<(u64, Vec<u8>)> {
    let r = run();
    (r["first"].as_u64().unwrap()..=r["last"].as_u64().unwrap())
        .map(|id| (id, std::fs::read(root_dir().join(format!("vectors/lez-blocks-v04/{id}.borsh"))).unwrap()))
        .collect()
}

fn id(v: &Value) -> [u8; 32] {
    *AccountId::from_str(v.as_str().unwrap()).unwrap().value()
}

fn h32(v: &Value) -> [u8; 32] {
    hex::decode(v.as_str().unwrap().trim_start_matches("0x")).unwrap().try_into().unwrap()
}

fn params() -> v04::Params {
    let r = run();
    v04::Params { program_id: id(&r["programs"]["wrap_bridge"]), burns_account: id(&r["burns"]), pairing: h32(&r["pairing"]) }
}

#[test]
fn the_mirror_is_lezs_v04_block_layout_on_real_blocks() {
    for (n, raw) in blocks() {
        let theirs: common::block::Block = borsh::from_slice(&raw).expect("LEZ's own type");
        let mine: v04::block::Block = borsh::from_slice(&raw).expect("the mirror");
        assert_eq!(borsh::to_vec(&theirs).unwrap(), raw, "block {n}: LEZ re-encodes to the same bytes");
        assert_eq!(borsh::to_vec(&mine).unwrap(), raw, "block {n}: the mirror re-encodes to the same bytes");
        // The hash: LEZ's own recomputation, the header, and the raw-bytes formula agree.
        assert_eq!(theirs.recompute_hash().0, theirs.header.hash.0, "block {n}");
        assert_eq!(lez_chain::content_hash(&raw), theirs.header.hash.0, "block {n}");
        assert_eq!(mine.body.reverted, theirs.body.reverted);
        // The scan agrees with the typed decode.
        let (h, leaves) = v04::scan::scan_block(&raw, &params()).expect("the scan reads it");
        assert_eq!((h.block_id, h.hash), (n, theirs.header.hash.0));
        assert_eq!(leaves, v04::burn_leaves(&mine, &params()));
        // Every block ends with the forced fee and clock transactions.
        assert!(theirs.body.transactions.len() >= 2);
    }
}

#[test]
fn the_reverted_list_is_what_the_sequencer_reverted() {
    let r = run();
    let blocks = blocks();
    for t in r["transactions"].as_array().unwrap() {
        let (n, i) = (t["block"].as_u64().unwrap(), t["tx"].as_u64().unwrap());
        let raw = &blocks.iter().find(|(id, _)| *id == n).unwrap().1;
        let b: common::block::Block = borsh::from_slice(raw).unwrap();
        assert_eq!(hex::encode(b.body.transactions[i as usize].hash().0), t["hash"].as_str().unwrap());
        assert_eq!(b.body.is_reverted(i as usize), t["verdict"] == "reverted", "{}", t["label"]);
    }
}

#[test]
fn applied_public_burns_and_only_those_are_derived_from_the_blocks() {
    let r = run();
    let blocks = blocks();
    let p = params();
    // Every Burn the run sent is in a block as a root call to the burn tree, reverted or not.
    let mut sent = 0;
    for (_, raw) in &blocks {
        let b: v04::block::Block = borsh::from_slice(raw).unwrap();
        for tx in &b.body.transactions {
            if tx.root_call().is_some_and(|c| c.to.account_id == p.burns_account && c.message[0] == v04::BURN_VARIANT) {
                sent += 1;
            }
        }
    }
    assert_eq!(sent, 4, "four burns in the blocks");
    // From the anchor (block 17, which block 18 names), the blocks give the applied two.
    let first: common::block::Block = borsh::from_slice(&blocks[0].1).unwrap();
    let mut s = v04::ChainState::at_anchor(blocks[0].0 - 1, first.header.prev_block_hash.0);
    let mut leaves = Vec::new();
    for (_, raw) in &blocks {
        leaves.extend(v04::apply_block(&mut s, &p, raw).expect("the next block"));
    }
    let want: Vec<[u8; 32]> = r["applied_burn_leaves"].as_array().unwrap().iter().map(h32).collect();
    assert_eq!(leaves, want);
    assert_eq!(s.tree.root, h32(&r["root"]), "the program's own tree, read back from the sequencer");
    assert_eq!(s.tip_id, r["last"].as_u64().unwrap());
}

#[test]
fn a_block_whose_reverted_list_is_tampered_does_not_hash() {
    let p = params();
    let blocks = blocks();
    // Block 26 holds a reverted burn: un-list it, and the block no longer has its hash.
    let raw = &blocks.iter().find(|(id, _)| *id == 26).unwrap().1;
    let mut b: v04::block::Block = borsh::from_slice(raw).unwrap();
    assert_eq!(b.body.reverted, vec![0]);
    b.body.reverted.clear();
    let forged = borsh::to_vec(&b).unwrap();
    let (_, leaves) = v04::scan::scan_block(&forged, &p).unwrap();
    assert_eq!(leaves.len(), 1, "un-listed, the reverted burn would count");
    let mut s = v04::ChainState::at_anchor(25, b.header.prev_block_hash);
    assert_eq!(v04::apply_block(&mut s, &p, &forged), Err(lez_chain::ChainError::HashMismatch { block_id: 26 }));
}

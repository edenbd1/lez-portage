//! The chain guest's view of LEZ blocks (crate lez-chain) against LEE's own
//! types and the bridge program's own effect: the `Append` variant index it
//! scans for is wrap_bridge's, and every transaction of the committed
//! sequencer blocks (vectors/lez-blocks, from the local v0.3.1 run of
//! evidence/local-chain) re-encodes from the mirror into bytes LEE's
//! `PublicTransaction` / `PrivacyPreservingTransaction` decode completely
//! and re-encode identically.

use lez_chain::block::{Block, LeeTransaction};

#[test]
fn the_append_variant_is_wrap_bridges() {
    let leaf = [7u8; 32];
    let data = borsh::to_vec(&wrap_bridge::Effect::Append(leaf)).unwrap();
    assert_eq!(data, [&[lez_chain::APPEND_VARIANT][..], &leaf].concat());
}

#[test]
fn the_committed_blocks_decode_with_lees_types() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../vectors/lez-blocks");
    let mut seen = (0, 0);
    for e in std::fs::read_dir(&dir).expect("vectors/lez-blocks") {
        let p = e.unwrap().path();
        if p.extension().and_then(|x| x.to_str()) != Some("borsh") {
            continue;
        }
        let raw = std::fs::read(&p).unwrap();
        let b: Block = borsh::from_slice(&raw).unwrap();
        assert_eq!(lez_chain::content_hash(&raw), b.header.hash, "{}", p.display());
        assert_eq!(borsh::to_vec(&b).unwrap(), raw);
        for tx in &b.body.transactions {
            match tx {
                LeeTransaction::Public(t) => {
                    let bytes = borsh::to_vec(t).unwrap();
                    let lee_tx: lee::PublicTransaction = borsh::from_slice(&bytes).unwrap();
                    assert_eq!(borsh::to_vec(&lee_tx).unwrap(), bytes);
                    seen.0 += 1;
                }
                LeeTransaction::PrivacyPreserving(t) => {
                    let bytes = borsh::to_vec(t).unwrap();
                    let lee_tx: lee::PrivacyPreservingTransaction = borsh::from_slice(&bytes).unwrap();
                    assert_eq!(borsh::to_vec(&lee_tx).unwrap(), bytes);
                    seen.1 += 1;
                }
            }
        }
    }
    assert!(seen.0 > 0 && seen.1 > 0, "public and private transactions: {seen:?}");
}

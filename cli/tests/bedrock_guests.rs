//! The two Bedrock guests, run in RISC Zero's executor on the committed
//! binaries (artifacts/zk/bedrock_anchor.bin, bedrock_release.bin): the
//! journal each commits is lez-bedrock's statement, a tampered input makes the
//! guest panic (no receipt), and the cycle counts on a real public-testnet
//! anchor (vectors/bedrock/testnet-lez-36401.json: LEZ block 36401, its
//! Bedrock block and 30 headers above it). The receipts the guests verify
//! inside are given as unresolved assumptions (claims), which is what the
//! executor checks `env::verify` against; proving resolves them.

use lez_bedrock::{compose, AnchorBlock};
use risc0_zkvm::{default_executor, ExecutorEnv, ReceiptClaim, SessionInfo};
use serde_json::Value;

type H256 = [u8; 32];

fn elf(name: &str) -> Vec<u8> {
    std::fs::read(format!("../artifacts/zk/{name}.bin")).unwrap()
}

fn image(e: &[u8]) -> H256 {
    risc0_binfmt::compute_image_id(e).unwrap().into()
}

fn hexv(v: &Value) -> Vec<u8> {
    hex::decode(v.as_str().unwrap()).unwrap()
}

fn testnet() -> (H256, H256, AnchorBlock) {
    let v: Value = serde_json::from_str(&std::fs::read_to_string("../vectors/bedrock/testnet-lez-36401.json").unwrap()).unwrap();
    let a = &v["anchor_block"];
    let b = AnchorBlock {
        header: hexv(&a["header"]),
        uncles: hexv(&a["uncles"]),
        ops: hexv(&a["ops"]),
        tx_index: a["tx_index"].as_u64().unwrap() as u32,
        path: a["path"].as_array().unwrap().iter().map(|p| hexv(p).try_into().unwrap()).collect(),
        signature: hexv(&a["signature"]),
        descendants: a["descendants"].as_array().unwrap().iter().map(hexv).collect(),
    };
    (hexv(&v["channel_id"]).try_into().unwrap(), hexv(&v["signer"]).try_into().unwrap(), b)
}

const PARAMS: lez_chain::Params = lez_chain::Params { program_id: [0xb1; 32], burns_account: [0xb2; 32] };

/// The chain journal of the one LEZ block the anchor inscribes, from its parent.
fn chain_journal(lez: &[u8]) -> Vec<u8> {
    let ci = lez_chain::ChainInput {
        self_image_id: image(&elf("lez_chain")),
        params: PARAMS,
        anchor_id: u64::from_le_bytes(lez[..8].try_into().unwrap()) - 1,
        anchor_hash: lez[8..40].try_into().unwrap(),
        prior: None,
        blocks: vec![lez.to_vec()],
    };
    lez_chain::chain_journal(&ci).unwrap()
}

fn run_anchor(i: &compose::AnchorInput) -> anyhow::Result<SessionInfo> {
    let b = borsh::to_vec(i).unwrap();
    let stream = [&(b.len() as u32).to_le_bytes()[..], &b].concat();
    let (cid, _, _) = lez_chain::parse_journal(&i.chain_journal).unwrap();
    let env = ExecutorEnv::builder()
        .add_assumption(ReceiptClaim::ok(cid, i.chain_journal.clone()))
        .write_slice(&stream)
        .build()
        .unwrap();
    default_executor().execute(env, &elf("bedrock_anchor"))
}

fn testnet_input() -> compose::AnchorInput {
    let (channel_id, signer, block) = testnet();
    let p = lez_bedrock::Params { channel_id, signer };
    let lez = lez_bedrock::anchor(&p, &block, 30).unwrap().inscription.data.to_vec();
    compose::AnchorInput { self_image_id: image(&elf("bedrock_anchor")), channel_id, signer, chain_journal: chain_journal(&lez), block }
}

#[test]
fn the_anchor_guest_commits_lez_bedrocks_journal_on_a_real_testnet_anchor() {
    let i = testnet_input();
    let s = run_anchor(&i).unwrap();
    assert_eq!(s.journal.bytes, compose::anchor_journal(&i).unwrap());
    let user: u64 = s.segments.iter().map(|g| g.cycles as u64).sum();
    eprintln!(
        "bedrock_anchor on testnet LEZ block 36401 (Bedrock block, tx 5 of 29, path 5, 30 headers above; ops {} bytes): user cycles {}, segments {}",
        i.block.ops.len(),
        user,
        s.segments.len()
    );
    // The same anchor 0 deep: the cost of one header more.
    let mut shallow = i.clone();
    shallow.block.descendants.clear();
    let s0 = run_anchor(&shallow).unwrap();
    let user0: u64 = s0.segments.iter().map(|g| g.cycles as u64).sum();
    eprintln!("  the same 0 deep: user cycles {user0}; per header {}", (user - user0) / 30);
}

#[test]
fn the_anchor_guest_panics_on_each_tampering() {
    let i = testnet_input();
    let cases: Vec<(&str, Box<dyn Fn(&mut compose::AnchorInput)>)> = vec![
        ("a header byte", Box::new(|i| i.block.header[45] ^= 1)),
        ("an ops byte", Box::new(|i| i.block.ops[300] ^= 1)),
        ("a path node", Box::new(|i| i.block.path[2][0] ^= 1)),
        ("the signature", Box::new(|i| i.block.signature[10] ^= 1)),
        ("the pinned signer", Box::new(|i| i.signer[0] ^= 1)),
        ("the pinned channel", Box::new(|i| i.channel_id[0] ^= 1)),
        ("a descendant", Box::new(|i| i.block.descendants[7][100] ^= 1)),
        ("the chain journal's tip", Box::new(|i| i.chain_journal[32 * 7] ^= 1)),
    ];
    for (what, f) in cases {
        let mut t = i.clone();
        f(&mut t);
        assert!(run_anchor(&t).is_err(), "the guest accepted a tampered {what}");
    }
    // Without the chain receipt's claim: env::verify fails.
    let b = borsh::to_vec(&i).unwrap();
    let env = ExecutorEnv::builder().write_slice(&[&(b.len() as u32).to_le_bytes()[..], &b].concat()).build().unwrap();
    assert!(default_executor().execute(env, &elf("bedrock_anchor")).is_err(), "the guest ran without the chain receipt");
}

#[test]
fn the_release_guest_commits_the_bedrock_release_journal() {
    // A chain journal whose tree holds one burn, anchored on the testnet block:
    // not a real chain (its tip is the testnet block, its tree is made up), but
    // the release guest only reads the anchor journal, which it verifies as an
    // assumption; what it proves is the burn under that journal's root.
    let a20 = |b: u8| {
        let mut x = [0u8; 20];
        x[19] = b;
        x
    };
    let (pairing, secret) = ([0x11; 32], [0x55; 32]);
    let me = wrap_core::burn_leaf(&pairing, &a20(2), 7, &wrap_core::eth_commitment(&a20(3), 1, &secret));
    let mut i = testnet_input();
    let (cid, params, mut s) = lez_chain::parse_journal(&i.chain_journal).unwrap();
    s.tree.insert(&me).unwrap();
    i.chain_journal = lez_chain::journal(&cid, &params, &s);
    let aj = compose::anchor_journal(&i).unwrap();
    let r = wrap_core::zk::ZkReleaseInput {
        chain_id: 31337, vault: a20(1), pairing, relayer: a20(4), token: a20(2), amount: 7, recipient: a20(3), fee: 1,
        secret, index: 0, path: wrap_core::path(&[me], 0),
    };
    let run = |r: &wrap_core::zk::ZkReleaseInput, aj: &[u8], claim: bool| {
        let mut env = ExecutorEnv::builder();
        if claim {
            env.add_assumption(ReceiptClaim::ok(image(&elf("bedrock_anchor")), aj.to_vec()));
        }
        let env = env.write(&borsh::to_vec(&(r.clone(), aj.to_vec())).unwrap()).unwrap().build().unwrap();
        default_executor().execute(env, &elf("bedrock_release"))
    };
    let s = run(&r, &aj, true).unwrap();
    assert_eq!(s.journal.bytes, compose::release_journal(&r, &aj).unwrap());
    assert_eq!(s.journal.bytes.len(), 800);
    eprintln!("bedrock_release: user cycles {}", s.segments.iter().map(|g| g.cycles as u64).sum::<u64>());
    // Another amount, the anchor receipt missing, an anchor journal naming another image id.
    assert!(run(&wrap_core::zk::ZkReleaseInput { amount: 8, ..r.clone() }, &aj, true).is_err());
    assert!(run(&r, &aj, false).is_err());
    let mut other = aj.clone();
    other[32] ^= 1;
    assert!(run(&r, &other, true).is_err());
}

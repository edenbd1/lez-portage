//! Cycles of the v0.4 chain guest (v04/artifacts/lez_chain_v04.bin, zk/lez_chain_v04)
//! in the RISC Zero executor (no proving): its journal is checked against
//! `lez_chain_v04::chain_journal` on the host, then user and total cycles
//! (total = the segments' padded power-of-two sizes) are printed, for:
//!
//! 1. the real blocks of vectors/lez-blocks-v04 (a local v0.4.0-rc3 sequencer);
//! 2. synthetic chains in the exact v0.4 layout (chain/v04/src/block.rs), linked and
//!    hashed: blocks of 18 public burns (a full 20-transaction block), and blocks
//!    of 4 private transactions with a 223,551-byte proof each (`PROOF_BYTES`, at
//!    the per-block private cap set by `MAX_GAS_STOR / PRIVATE_GAS_STOR`).
//!
//! `cargo run --release --bin measure_chain` (prints a markdown table).

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use lez_chain_v04::{self as v04, block::*};
use risc0_zkvm::{default_executor, ExecutorEnv};
use wrap_executor_tests_v04::{bin, root_dir};

fn exec(elf: &[u8], head: &v04::ChainHead, blocks: &[Vec<u8>]) -> (u64, u64, usize) {
    let env = ExecutorEnv::builder().write_slice(&v04::encode_stream(head, blocks)).build().unwrap();
    let info = default_executor().execute(env, elf).expect("the guest runs");
    assert_eq!(info.journal.bytes, v04::chain_journal(head, blocks).unwrap(), "the guest's journal is the host's");
    let user: u64 = info.segments.iter().map(|s| u64::from(s.cycles)).sum();
    let total: u64 = info.segments.iter().map(|s| 1u64 << s.po2).sum();
    (user, total, info.segments.len())
}

const PROGRAM: H256 = [0xb1; 32];
const BURNS: H256 = [0xb2; 32];
const PAIRING: H256 = [0xb3; 32];

fn actor(a: H256, p: H256) -> Actor {
    Actor { account_id: a, program_account_id: p }
}

fn ctx(n: u8) -> PublicExecutionContext {
    PublicExecutionContext {
        actors: (0..5).map(|i| actor([n.wrapping_add(i); 32], PROGRAM)).collect(),
        authorized_accounts: BTreeSet::from([[n; 32], [0xfe; 32]]),
        promotions: BTreeSet::new(),
    }
}

/// A public Burn the size of the real ones (config included in the message).
fn public_burn(n: u8) -> LeeTransaction {
    let mut message = vec![v04::BURN_VARIANT];
    message.extend_from_slice(&[n; 20]);
    message.extend_from_slice(&100_000_000_000_000_000u128.to_le_bytes());
    message.extend_from_slice(&[n; 32]);
    message.extend(std::iter::repeat_n(0x11, 32 + 32 + 300)); // salt, holder, config
    LeeTransaction::Public(PublicTransaction {
        message: PublicMessage {
            public_context: ctx(n),
            execution: PublicExecution { root: RootCall { to: actor(BURNS, PROGRAM), message }, fee: Some(FeeDeclaration { payer: [0xfe; 32], gas_limit: 2_000_000, tip: 0, max_fee: 1 }) },
            nonces: BTreeMap::from([([n; 32], 1), ([0xfe; 32], 2)]),
            admission_evidence: vec![],
        },
        witness_set: PublicWitnessSet { signatures_and_public_keys: vec![(Signature { value: [1; 64] }, [2; 32]); 2] },
    })
}

fn note() -> EncryptedNote {
    EncryptedNote { ciphertext: vec![4; 1200], epk: vec![5; 1088] }
}

fn private_tx(n: u8) -> LeeTransaction {
    let action = PrivateAction { nullifier: [n; 32], root: [2; 32], commitment: [3; 32], encrypted_post_state: note() };
    LeeTransaction::PrivacyPreserving(PrivacyPreservingTransaction {
        message: PrivacyMessage {
            public_context: ctx(n),
            execution: ProvenExecution {
                boundary: vec![],
                async_messages: vec![],
                recovery_bindings: vec![],
                public_root: None,
                private_actions: vec![action.clone(), action],
                validity: ValidityWindows { blocks: ValidityWindow { from: None, to: None }, timestamps: ValidityWindow { from: None, to: None } },
                program_image_claims: vec![ProgramImageClaim::Disclosed { account_id: PROGRAM, image_id: [7; 8] }],
                fee: Some(ProvenFee { refund_to: None, fallback: None }),
            },
            nonces: BTreeMap::new(),
            admission_evidence: vec![],
        },
        witness_set: PrivacyWitnessSet { signatures_and_public_keys: vec![], proof: vec![8; 223_551] },
    })
}

fn system() -> [LeeTransaction; 2] {
    let t = |p: u8| LeeTransaction::Public(PublicTransaction {
        message: PublicMessage {
            public_context: PublicExecutionContext { actors: BTreeSet::from([actor([p; 32], [p; 32])]), authorized_accounts: BTreeSet::new(), promotions: BTreeSet::new() },
            execution: PublicExecution { root: RootCall { to: actor([p; 32], [p; 32]), message: vec![0; 40] }, fee: None },
            nonces: BTreeMap::new(),
            admission_evidence: vec![],
        },
        witness_set: PublicWitnessSet { signatures_and_public_keys: vec![] },
    });
    [t(0xf0), t(0xf1)]
}

fn chain(n: usize, txs: impl Fn(usize) -> Vec<LeeTransaction>) -> (v04::ChainHead, Vec<Vec<u8>>) {
    let mut prev = [7u8; 32];
    let mut out = Vec::new();
    for i in 0..n {
        let mut t = txs(i);
        let reverted = if t.len() > 1 { vec![1] } else { vec![] };
        t.extend(system());
        let mut b = Block {
            header: BlockHeader { block_id: 101 + i as u64, prev_block_hash: prev, hash: [0; 32], timestamp: 1_000 * i as u64 },
            body: BlockBody { transactions: t, reverted },
            bedrock_status: BedrockStatus::Pending,
        };
        b.header.hash = lez_chain::content_hash(&borsh::to_vec(&b).unwrap());
        prev = b.header.hash;
        out.push(borsh::to_vec(&b).unwrap());
    }
    let head = v04::ChainHead { self_image_id: [0x77; 32], params: v04::Params { program_id: PROGRAM, burns_account: BURNS, pairing: PAIRING }, anchor_id: 100, anchor_hash: [7; 32], blocks: n as u32 };
    (head, out)
}

fn main() {
    let elf = bin("lez_chain_v04.bin");
    println!("| input | blocks | bytes | burns counted | user cycles | total cycles | segments | user cycles/byte |");
    println!("|---|---:|---:|---:|---:|---:|---:|---:|");
    let mut row = |what: &str, head: v04::ChainHead, blocks: Vec<Vec<u8>>| {
        let bytes: usize = blocks.iter().map(Vec::len).sum();
        let mut s = v04::ChainState::at_anchor(head.anchor_id, head.anchor_hash);
        let burns: usize = blocks.iter().map(|b| v04::apply_block(&mut s, &head.params, b).unwrap().len()).sum();
        let (user, total, segs) = exec(&elf, &head, &blocks);
        println!("| {what} | {} | {bytes} | {burns} | {user} | {total} | {segs} | {:.2} |", blocks.len(), user as f64 / bytes as f64);
    };

    // 1. The real blocks.
    let run: serde_json::Value = serde_json::from_slice(&std::fs::read(root_dir().join("vectors/lez-blocks-v04/run.json")).unwrap()).unwrap();
    let (first, last) = (run["first"].as_u64().unwrap(), run["last"].as_u64().unwrap());
    let blocks: Vec<Vec<u8>> = (first..=last).map(|i| std::fs::read(root_dir().join(format!("vectors/lez-blocks-v04/{i}.borsh"))).unwrap()).collect();
    let b0: Block = borsh::from_slice(&blocks[0]).unwrap();
    let acct = |v: &serde_json::Value| *lee::AccountId::from_str(v.as_str().unwrap()).unwrap().value();
    let pairing: [u8; 32] = hex::decode(run["pairing"].as_str().unwrap().trim_start_matches("0x")).unwrap().try_into().unwrap();
    let params = v04::Params { program_id: acct(&run["programs"]["wrap_bridge"]), burns_account: acct(&run["burns"]), pairing };
    let head = v04::ChainHead { self_image_id: [0x77; 32], params, anchor_id: first - 1, anchor_hash: b0.header.prev_block_hash, blocks: blocks.len() as u32 };
    row(&format!("real v0.4.0-rc3 blocks {first}-{last} (sequencer)"), head, blocks);

    // 2. Synthetic chains in the v0.4 layout.
    for n in [1usize, 10, 50] {
        let (h, b) = chain(n, |i| (0..18).map(|k| public_burn((i * 18 + k) as u8)).collect());
        row(&format!("{n} x (18 public burns, 1 reverted)"), h, b);
    }
    for n in [1usize, 5] {
        let (h, b) = chain(n, |i| (0..4).map(|k| private_tx((i * 4 + k) as u8)).collect());
        row(&format!("{n} x (4 private txs, 223,551-byte proofs)"), h, b);
    }
    let (h, b) = chain(10, |_| vec![]);
    row("10 x empty (fee + clock only)", h, b);
}

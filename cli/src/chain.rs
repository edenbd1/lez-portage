//! The chain-derived release: `zk/lez_chain` proven over LEZ blocks read from
//! the sequencer, and `zk/burn_release` proven against its journal, the chain
//! receipt resolved as an assumption, then compressed to Groth16.
//!
//! The chain receipt is kept succinct (`<out>.receipt`, bincode): a later
//! chain step verifies it as its prior, and a release verifies it as the
//! source of its root. Only the release is wrapped in Groth16, so Ethereum
//! checks one seal for both.

use std::{path::Path, time::Instant};

use anyhow::{anyhow, ensure, Result};
use lez_chain::{ChainInput, ChainState, Params};
use risc0_zkvm::{default_prover, ExecutorEnv, ProverOpts, Receipt};
use serde_json::{json, Value};
use wrap_core::H256;

use crate::zk::{encode_seal, hx, image_id};

/// One block as the sequencer returns it, re-encoded: `borsh(Block)` of the
/// v0.3.1 layout. The caller checks that LEZ's types and the guest's mirror
/// decode it to the same bytes (`check_layout`).
pub fn raw_block(b: &common::block::Block, params: &Params) -> Result<Vec<u8>> {
    let raw = borsh::to_vec(b)?;
    check_layout(&raw, params)?;
    Ok(raw)
}

/// LEZ's own `Block` and the guest's mirror both decode `raw` completely and
/// re-encode it byte for byte, the mirror's content hash is the header's, and
/// the guest's scan finds the same header and burn leaves as the typed mirror.
pub fn check_layout(raw: &[u8], params: &Params) -> Result<()> {
    let lez: common::block::Block = borsh::from_slice(raw).map_err(|e| anyhow!("LEZ's Block does not decode: {e}"))?;
    let mirror: lez_chain::block::Block = borsh::from_slice(raw).map_err(|e| anyhow!("the mirror does not decode: {e}"))?;
    ensure!(borsh::to_vec(&lez)? == raw, "LEZ's Block re-encodes differently");
    ensure!(borsh::to_vec(&mirror)? == raw, "the mirror re-encodes differently");
    ensure!(lez.recompute_hash().0 == lez.header.hash.0, "LEZ's own hash check fails");
    ensure!(lez_chain::content_hash(raw) == lez.header.hash.0, "the mirror's hash is not LEZ's");
    ensure!(lez.body.transactions.len() == mirror.body.transactions.len());
    let (h, leaves) = lez_chain::scan::scan_block(raw, params).map_err(|_| anyhow!("the guest's scan refuses the block"))?;
    ensure!(h.hash == mirror.header.hash && h.block_id == mirror.header.block_id && h.prev_block_hash == mirror.header.prev_block_hash);
    ensure!(leaves == lez_chain::private_burn_leaves(&mirror, params), "the scan and the typed mirror find different leaves");
    Ok(())
}

/// Save a succinct receipt next to its JSON.
pub fn save_receipt(p: &Path, r: &Receipt) -> Result<()> {
    std::fs::write(p, bincode::serialize(r)?)?;
    Ok(())
}

pub fn load_receipt(p: &Path) -> Result<Receipt> {
    Ok(bincode::deserialize(&std::fs::read(p)?)?)
}

pub fn receipt_path(json_path: &Path) -> std::path::PathBuf {
    json_path.with_extension("receipt")
}

/// Prove the chain guest over `input` (its `prior`, if any, resolved with
/// `prior_receipt`). Returns the succinct receipt and the public record.
pub fn prove_chain(elf: &[u8], input: &ChainInput, prior_receipt: Option<Receipt>) -> Result<(Receipt, Value)> {
    let expected = lez_chain::chain_journal(input).map_err(|e| anyhow!("the blocks do not apply: {e:?}"))?;
    let image = image_id(elf)?;
    ensure!(image == input.self_image_id, "the input names another image id than the guest's");
    let bytes = lez_chain::encode_stream(input);
    let mut env = ExecutorEnv::builder();
    if let Some(r) = prior_receipt {
        ensure!(Some(&r.journal.bytes) == input.prior.as_ref(), "the prior receipt is not the prior journal's");
        env.add_assumption(r);
    }
    let env = env.write_slice(&bytes).build()?;
    let t0 = Instant::now();
    let info = default_prover().prove_with_opts(env, elf, &ProverOpts::succinct())?;
    let ms = t0.elapsed().as_millis();
    let r = info.receipt;
    ensure!(r.journal.bytes == expected, "the chain guest's journal is not the expected one");
    r.verify(image).map_err(|e| anyhow!("chain receipt: {e}"))?;
    let (_, _, s) = lez_chain::parse_journal(&r.journal.bytes).expect("a chain journal");
    let block_bytes: usize = input.blocks.iter().map(Vec::len).sum();
    let rec = json!({
        "image_id": hx(&image),
        "journal": hx(&r.journal.bytes),
        "anchor": {"id": s.anchor_id, "hash": hx(&s.anchor_hash)},
        "tip": {"id": s.tip_id, "hash": hx(&s.tip_hash)},
        "root": hx(&s.tree.root),
        "next_index": s.tree.next_index,
        "blocks": input.blocks.len(),
        "block_bytes": block_bytes,
        "prior": input.prior.is_some(),
        "stats": {"segments": info.stats.segments, "total_cycles": info.stats.total_cycles, "user_cycles": info.stats.user_cycles,
                  "paging_cycles": info.stats.paging_cycles, "reserved_cycles": info.stats.reserved_cycles, "stark_succinct_ms": ms},
        "dev_mode": ProverOpts::default().dev_mode(),
    });
    Ok((r, rec))
}

/// Replay `blocks` on the host from `state`, recording each leaf appended with
/// its block id and transaction index.
pub fn replay(state: &mut ChainState, params: &Params, blocks: &[Vec<u8>]) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for raw in blocks {
        let b: lez_chain::block::Block = borsh::from_slice(raw)?;
        let before = state.tree.next_index;
        let leaves = lez_chain::apply_block(state, params, raw).map_err(|e| anyhow!("block {}: {e:?}", b.header.block_id))?;
        // Which transaction appended each leaf: the same scan, one transaction at a time.
        let mut k = 0usize;
        for (t, tx) in b.body.transactions.iter().enumerate() {
            let one = lez_chain::block::Block { body: lez_chain::block::BlockBody { transactions: vec![tx.clone()] }, ..b.clone() };
            for leaf in lez_chain::private_burn_leaves(&one, params) {
                ensure!(leaves.get(k) == Some(&leaf));
                out.push(json!({"leaf": hx(&leaf), "block": b.header.block_id, "tx_index": t, "index": before + k as u64}));
                k += 1;
            }
        }
        ensure!(k == leaves.len());
    }
    Ok(out)
}

/// Prove the release guest for `input` against `chain_journal` (its receipt
/// `chain_receipt` resolved as an assumption), then compress to Groth16.
pub fn prove_release(elf: &[u8], input: &wrap_core::zk::ZkReleaseInput, chain_journal: &[u8], chain_receipt: Receipt) -> Result<Value> {
    let expected = lez_chain::release_journal(input, chain_journal).ok_or_else(|| anyhow!("the opening is not a burn of the chain's tree"))?;
    let image = image_id(elf)?;
    let bytes = borsh::to_vec(&(input.clone(), chain_journal.to_vec()))?;
    let env = ExecutorEnv::builder().add_assumption(chain_receipt).write(&bytes)?.build()?;
    let prover = default_prover();
    let t0 = Instant::now();
    let info = prover.prove_with_opts(env, elf, &ProverOpts::succinct())?;
    let stark_ms = t0.elapsed().as_millis();
    let succinct = info.receipt;
    ensure!(succinct.journal.bytes == expected, "the release guest's journal is not the expected one");
    succinct.verify(image).map_err(|e| anyhow!("succinct release receipt: {e}"))?;
    let t1 = Instant::now();
    let receipt = prover.compress(&ProverOpts::groth16(), &succinct)?;
    let groth16_ms = t1.elapsed().as_millis();
    receipt.verify(image).map_err(|e| anyhow!("Groth16 release receipt: {e}"))?;
    ensure!(receipt.journal.bytes == expected);
    let seal = encode_seal(&receipt)?;
    let j = &receipt.journal.bytes;
    let w = |k: usize| hx(&j[32 * k..32 * (k + 1)]);
    Ok(json!({
        "image_id": hx(&image),
        "journal": hx(j),
        "journal_bytes": j.len(),
        "journal_digest": hx(&wrap_core::sha256(&[j])),
        "release_digest": hx(&wrap_core::sha256(&[&j[..352]])),
        "fields": {"tag": w(0), "chain_id": w(1), "vault": w(2), "pairing": w(3), "token": w(4), "amount": w(5),
                   "recipient": w(6), "relayer": w(7), "fee": w(8), "root": w(9), "nullifier": w(10),
                   "chain_image_id": w(11), "program_id": w(12), "burns_account": w(13), "anchor_id": w(14), "anchor_hash": w(15),
                   "tip_id": w(16), "tip_hash": w(17)},
        "seal": hx(&seal),
        "seal_bytes": seal.len(),
        "stats": {"segments": info.stats.segments, "total_cycles": info.stats.total_cycles, "user_cycles": info.stats.user_cycles,
                  "stark_succinct_ms": stark_ms, "groth16_ms": groth16_ms},
        "dev_mode": ProverOpts::default().dev_mode(),
        "docker_default_platform": std::env::var("DOCKER_DEFAULT_PLATFORM").ok(),
    }))
}

pub fn h(v: &Value) -> Result<H256> {
    let s = v.as_str().ok_or_else(|| anyhow!("not hex: {v}"))?;
    hex::decode(s.trim_start_matches("0x"))?.try_into().map_err(|_| anyhow!("not 32 bytes: {s}"))
}

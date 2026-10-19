//! The Bedrock-anchored release: `zk/bedrock_anchor` proven over a chain-prove
//! record and the Bedrock block that inscribes its tip (bedrock-tools anchor),
//! the chain receipt resolved as an assumption; then `zk/bedrock_release`
//! proven against the anchor journal, the anchor receipt resolved as an
//! assumption, and compressed to Groth16. One seal for the three guests.

use std::time::Instant;

use anyhow::{anyhow, ensure, Result};
use lez_bedrock::{compose, AnchorBlock};
use risc0_zkvm::{default_prover, ExecutorEnv, ProverOpts, Receipt};
use serde_json::{json, Value};
use wrap_core::H256;

use crate::zk::{encode_seal, hx, image_id};

fn bytes(v: &Value) -> Result<Vec<u8>> {
    Ok(hex::decode(v.as_str().ok_or_else(|| anyhow!("not hex: {v}"))?.trim_start_matches("0x"))?)
}

fn h(v: &Value) -> Result<H256> {
    bytes(v)?.try_into().map_err(|_| anyhow!("not 32 bytes: {v}"))
}

/// The AnchorBlock and pinned parameters of a `bedrock-tools anchor` record.
pub fn anchor_block(rec: &Value) -> Result<(H256, H256, AnchorBlock)> {
    let a = &rec["anchor_block"];
    let block = AnchorBlock {
        header: bytes(&a["header"])?,
        uncles: bytes(&a["uncles"])?,
        ops: bytes(&a["ops"])?,
        tx_index: a["tx_index"].as_u64().ok_or_else(|| anyhow!("no tx_index"))? as u32,
        path: a["path"].as_array().ok_or_else(|| anyhow!("no path"))?.iter().map(h).collect::<Result<_>>()?,
        signature: bytes(&a["signature"])?,
        descendants: a["descendants"].as_array().ok_or_else(|| anyhow!("no descendants"))?.iter().map(bytes).collect::<Result<_>>()?,
    };
    Ok((h(&rec["channel_id"])?, h(&rec["signer"])?, block))
}

/// The guest's stdin: `u32 len ‖ borsh(AnchorInput)`.
fn stream(i: &compose::AnchorInput) -> Result<Vec<u8>> {
    let b = borsh::to_vec(i)?;
    Ok([&(b.len() as u32).to_le_bytes()[..], &b].concat())
}

/// Prove the anchor guest. Returns the succinct receipt and the public record.
pub fn prove_anchor(elf: &[u8], input: &compose::AnchorInput, chain_receipt: Receipt) -> Result<(Receipt, Value)> {
    let expected = compose::anchor_journal(input).map_err(|e| anyhow!("the chain's tip is not anchored: {e:?}"))?;
    let image = image_id(elf)?;
    ensure!(image == input.self_image_id, "the input names another image id than the guest's");
    ensure!(chain_receipt.journal.bytes == input.chain_journal, "the chain receipt is not the chain journal's");
    let env = ExecutorEnv::builder().add_assumption(chain_receipt).write_slice(&stream(input)?).build()?;
    let t0 = Instant::now();
    let info = default_prover().prove_with_opts(env, elf, &ProverOpts::succinct())?;
    let ms = t0.elapsed().as_millis();
    let r = info.receipt;
    ensure!(r.journal.bytes == expected, "the anchor guest's journal is not the expected one");
    r.verify(image).map_err(|e| anyhow!("anchor receipt: {e}"))?;
    let (a, chain) = compose::parse_anchor_journal(&r.journal.bytes).expect("an anchor journal");
    let (chain_image, _, s) = lez_chain::parse_journal(chain).expect("a chain journal");
    let rec = json!({
        "op": "anchor-prove",
        "image_id": hx(&image),
        "journal": hx(&r.journal.bytes),
        "channel_id": hx(&a.params.channel_id), "signer": hx(&a.params.signer),
        "bedrock_block": {"id": hx(&a.block_id), "slot": a.block_slot},
        "top": {"id": hx(&a.top_id), "slot": a.top_slot, "depth": a.depth},
        "chain_image_id": hx(&chain_image),
        "tip": {"id": s.tip_id, "hash": hx(&s.tip_hash)},
        "root": hx(&s.tree.root),
        "input": {"ops_bytes": input.block.ops.len(), "path_len": input.block.path.len(), "descendants": input.block.descendants.len(),
                  "uncles_bytes": input.block.uncles.len(), "stream_bytes": stream(input)?.len()},
        "stats": {"segments": info.stats.segments, "total_cycles": info.stats.total_cycles, "user_cycles": info.stats.user_cycles,
                  "paging_cycles": info.stats.paging_cycles, "reserved_cycles": info.stats.reserved_cycles, "stark_succinct_ms": ms},
        "dev_mode": ProverOpts::default().dev_mode(),
    });
    Ok((r, rec))
}

/// Prove the release guest against `anchor_journal` (its receipt resolved as
/// an assumption), then compress to Groth16.
pub fn prove_release(elf: &[u8], input: &wrap_core::zk::ZkReleaseInput, anchor_journal: &[u8], anchor_receipt: Receipt) -> Result<Value> {
    let expected = compose::release_journal(input, anchor_journal).ok_or_else(|| anyhow!("the opening is not a burn of the anchored chain's tree"))?;
    ensure!(anchor_receipt.journal.bytes == anchor_journal, "the anchor receipt is not the anchor journal's");
    let image = image_id(elf)?;
    let bytes = borsh::to_vec(&(input.clone(), anchor_journal.to_vec()))?;
    let env = ExecutorEnv::builder().add_assumption(anchor_receipt).write(&bytes)?.build()?;
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
                   "tip_id": w(16), "tip_hash": w(17),
                   "anchor_image_id": w(18), "channel_id": w(19), "signer": w(20), "bedrock_block_id": w(21),
                   "top_header_id": w(22), "top_slot": w(23), "depth": w(24)},
        "seal": hx(&seal),
        "seal_bytes": seal.len(),
        "stats": {"segments": info.stats.segments, "total_cycles": info.stats.total_cycles, "user_cycles": info.stats.user_cycles,
                  "stark_succinct_ms": stark_ms, "groth16_ms": groth16_ms},
        "dev_mode": ProverOpts::default().dev_mode(),
        "docker_default_platform": std::env::var("DOCKER_DEFAULT_PLATFORM").ok(),
    }))
}

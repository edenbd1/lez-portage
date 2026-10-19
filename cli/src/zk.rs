//! The zero-knowledge release proof: `zk/burn_membership` (committed as
//! `artifacts/zk/burn_membership.bin`) proven with RISC Zero, its receipt
//! compressed to Groth16, and its seal encoded the way RISC Zero's
//! `RiscZeroGroth16Verifier` reads it (4-byte selector ‖ 256-byte proof).
//!
//! Proving goes through `r0vm` (the `client` feature: an external prover).
//! The Groth16 step (`shrink_wrap`) runs RISC Zero's x86_64 Docker image
//! `risczero/risc0-groth16-prover`; on Apple Silicon it runs emulated, with
//! `DOCKER_DEFAULT_PLATFORM=linux/amd64` in the environment.
//!
//! The two steps are timed apart: `prove_with_opts(.., succinct)` (the STARK,
//! lifted and joined into one succinct receipt) then `compress(.., groth16)`
//! (identity_p254 on the host, then the Groth16 prover in Docker).

use std::time::Instant;

use anyhow::{anyhow, ensure, Result};
use risc0_zkvm::{
    default_prover, sha::Digestible, ExecutorEnv, Groth16ReceiptVerifierParameters, InnerReceipt, ProverOpts, Receipt,
};
use serde_json::{json, Value};
use wrap_core::{zk::ZkReleaseInput, H256};

pub fn hx(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

/// The image id of the committed guest.
pub fn image_id(elf: &[u8]) -> Result<H256> {
    let id = risc0_binfmt::compute_image_id(elf).map_err(|e| anyhow!("{e}"))?;
    Ok(id.as_bytes().try_into().expect("32 bytes"))
}

/// The seal as `RiscZeroGroth16Verifier.verify` takes it: the first four bytes
/// of the verifier parameters' digest (its `SELECTOR`), then the proof.
pub fn encode_seal(receipt: &Receipt) -> Result<Vec<u8>> {
    let InnerReceipt::Groth16(g) = &receipt.inner else { return Err(anyhow!("not a Groth16 receipt")) };
    Ok([&g.verifier_parameters.as_bytes()[..4], g.seal.as_slice()].concat())
}

fn addr(v: &Value) -> Result<[u8; 20]> {
    let s = v.as_str().ok_or_else(|| anyhow!("not an address: {v}"))?;
    hex::decode(s.trim_start_matches("0x"))?.try_into().map_err(|_| anyhow!("not 20 bytes: {s}"))
}
fn word(v: &Value) -> Result<H256> {
    let s = v.as_str().ok_or_else(|| anyhow!("not a word: {v}"))?;
    hex::decode(s.trim_start_matches("0x"))?.try_into().map_err(|_| anyhow!("not 32 bytes: {s}"))
}
fn num<T: std::str::FromStr>(v: &Value) -> Result<T> {
    match v {
        Value::String(s) => s.parse().map_err(|_| anyhow!("not a number: {s}")),
        Value::Number(n) => n.to_string().parse().map_err(|_| anyhow!("not a number: {n}")),
        _ => Err(anyhow!("not a number: {v}")),
    }
}

pub fn input_from_json(v: &Value) -> Result<ZkReleaseInput> {
    Ok(ZkReleaseInput {
        chain_id: num(&v["chain_id"])?,
        vault: addr(&v["vault"])?,
        pairing: word(&v["pairing"])?,
        relayer: addr(&v["relayer"])?,
        token: addr(&v["token"])?,
        amount: num(&v["amount"])?,
        recipient: addr(&v["recipient"])?,
        fee: num(&v["fee"])?,
        secret: word(&v["secret"])?,
        index: num(&v["index"])?,
        path: v["path"].as_array().ok_or_else(|| anyhow!("no path"))?.iter().map(word).collect::<Result<_>>()?,
    })
}

/// Prove `input` with the guest `elf`: a succinct receipt, then its Groth16
/// compression. Returns the public record: the journal and its fields, the
/// seal, the image id, the verifier parameters, the timings and sizes. The
/// opening (secret, index, path) is not in it.
pub fn prove(elf: &[u8], input: &ZkReleaseInput) -> Result<Value> {
    let expected = wrap_core::zk::zk_release_journal(input).ok_or_else(|| anyhow!("the opening is malformed"))?;
    let image = image_id(elf)?;
    let bytes = borsh::to_vec(input)?;
    let env = ExecutorEnv::builder().write(&bytes)?.build()?;
    let prover = default_prover();

    let t0 = Instant::now();
    let info = prover.prove_with_opts(env, elf, &ProverOpts::succinct())?;
    let stark_ms = t0.elapsed().as_millis();
    let succinct = info.receipt;
    ensure!(succinct.journal.bytes == expected, "the guest's journal is not the release preimage");
    succinct.verify(image).map_err(|e| anyhow!("succinct receipt: {e}"))?;
    let succinct_bytes = succinct_seal_bytes(&succinct);

    let t1 = Instant::now();
    let receipt = prover.compress(&ProverOpts::groth16(), &succinct)?;
    let groth16_ms = t1.elapsed().as_millis();
    receipt.verify(image).map_err(|e| anyhow!("Groth16 receipt: {e}"))?;
    ensure!(receipt.journal.bytes == expected, "the Groth16 receipt's journal changed");
    let seal = encode_seal(&receipt)?;
    let params = Groth16ReceiptVerifierParameters::default();
    let j = &receipt.journal.bytes;
    let w = |k: usize| hx(&j[32 * k..32 * (k + 1)]);
    Ok(json!({
        "image_id": hx(&image),
        "journal": hx(j),
        "journal_digest": hx(wrap_core::sha256(&[j]).as_slice()),
        "fields": {"tag": w(0), "chain_id": w(1), "vault": w(2), "pairing": w(3), "token": w(4), "amount": w(5),
                   "recipient": w(6), "relayer": w(7), "fee": w(8), "root": w(9), "nullifier": w(10)},
        "seal": hx(&seal),
        "seal_bytes": seal.len(),
        "verifier_parameters": {"digest": hx(params.digest().as_bytes()), "selector": hx(&seal[..4]),
                                "control_root": hx(params.control_root.as_bytes()),
                                "bn254_control_id": hx(params.bn254_control_id.as_bytes())},
        "stats": {"segments": info.stats.segments, "total_cycles": info.stats.total_cycles, "user_cycles": info.stats.user_cycles,
                  "stark_succinct_ms": stark_ms, "groth16_ms": groth16_ms, "succinct_receipt_bytes": succinct_bytes},
        "dev_mode": ProverOpts::default().dev_mode(),
        "docker_default_platform": std::env::var("DOCKER_DEFAULT_PLATFORM").ok(),
    }))
}

fn succinct_seal_bytes(r: &Receipt) -> usize {
    // The succinct seal: u32 words.
    match &r.inner {
        InnerReceipt::Succinct(s) => s.seal.len() * 4,
        _ => 0,
    }
}

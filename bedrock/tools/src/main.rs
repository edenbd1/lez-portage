//! bedrock-tools: Bedrock blocks, as the node's API returns them, decoded with
//! the node's own types and checked against lez-bedrock.
//!
//!   bedrock-tools check DIR
//!       every block in DIR/chain.json: lez-bedrock's header id, body root,
//!       transaction hashes, first-operation inscriptions and signature
//!       verdicts against the node's types; prints a JSON summary.
//!   bedrock-tools anchor DIR --channel HEX --signer HEX --lez-hash HEX --depth K --out FILE
//!       the guest's AnchorBlock for the LEZ block with that hash: the Bedrock
//!       block whose transaction inscribes it on the channel, and the K headers
//!       above it in DIR's chain; checked by lez_bedrock::anchor and the node's
//!       types, written as JSON (hex).
//!
//!   bedrock-tools forge --key FILE --lez-block FILE --channel HEX --depth K --out FILE
//!       a Bedrock block no node produced: one transaction whose first operation
//!       inscribes the LEZ block (borsh) on the channel, signed by the Ed25519
//!       key in FILE (32-byte seed, a sequencer's `bedrock_signing_key`), under
//!       a made-up header, and K made-up headers above it. lez_bedrock::anchor
//!       accepts it: nothing in it proves a header is Bedrock's. Same JSON as
//!       `anchor`.
//!
//! DIR/chain.json lists header ids, oldest first, each the parent of the next;
//! DIR/<id>.json is the block (scripts/bedrock_fetch.py).

use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, anyhow, bail, ensure};
use lb_binary_codec::canonical::BinaryEncode as _;
use lb_core::{
    block::{SignedHeader, UncleHeaders, body_root},
    header::{ContentId, Header, HeaderId},
    mantle::{
        OpProofRef, OpRef, SignedOps,
        ledger::verification_mode::StandardMode,
        traits::Hashable as _,
        transactions::states::Unverified,
    },
    proofs::leader_proof::Groth16LeaderProof,
};
use lb_cryptarchia_engine::Slot;
use lb_kms_keys::keys::Ed25519Signature;
use lez_bedrock::{AnchorBlock, H256, Params};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct ApiHeader {
    id: HeaderId,
    parent_block: HeaderId,
    slot: Slot,
    body_root: ContentId,
    proof_of_leadership: Groth16LeaderProof,
}

#[derive(Deserialize)]
struct ApiSignedHeader {
    header: ApiHeader,
    signature: Ed25519Signature,
}

/// The node's `ApiBlock` (nodes/node/http-client): the block without the
/// leader's signature, which the API does not return.
#[derive(Deserialize)]
struct ApiBlock {
    header: ApiHeader,
    uncle_headers: Vec<ApiSignedHeader>,
    transactions: Vec<SignedOps<Unverified, StandardMode>>,
}

fn header(h: &ApiHeader) -> Header {
    Header::new(h.parent_block, h.body_root, h.slot, h.proof_of_leadership.clone())
}

fn h256(s: &str) -> Result<H256> {
    hex::decode(s.trim_start_matches("0x"))?.try_into().map_err(|_| anyhow!("not 32 bytes: {s}"))
}

fn hx(b: &[u8]) -> String {
    hex::encode(b)
}

struct Decoded {
    id: H256,
    header: Vec<u8>,
    uncles: Vec<u8>,
    /// Per transaction: canonical ops, hash, the first operation's inscription
    /// (channel, signer, data) and its Ed25519 proof, if it opens with one.
    txs: Vec<Tx>,
}

struct Tx {
    ops: Vec<u8>,
    hash: H256,
    inscription: Option<(H256, H256, Vec<u8>, [u8; 64])>,
}

/// Decode one API block with the node's types, re-encode it canonically, and
/// check lez-bedrock against the node on everything it computes.
fn decode(raw: &str) -> Result<Decoded> {
    let b: ApiBlock = serde_json::from_str(raw).context("the node's types do not decode the block")?;
    let hdr = header(&b.header);
    let id: H256 = hdr.id().into();
    ensure!(id == <[u8; 32]>::from(b.header.id), "the node's Header::id is not the API's id");
    let header_bytes = hdr.encode().to_vec();
    ensure!(header_bytes.len() == lez_bedrock::HEADER_LEN, "a {}-byte header", header_bytes.len());
    let mine = lez_bedrock::Header::parse(&header_bytes).map_err(|e| anyhow!("{e:?}"))?;
    ensure!(mine.id() == id, "lez-bedrock's header id differs from the node's");
    ensure!(mine.slot() == b.header.slot.into_inner() && mine.parent() == <[u8; 32]>::from(b.header.parent_block));
    ensure!(mine.body_root() == <[u8; 32]>::from(b.header.body_root));

    let list: Vec<SignedHeader> = b.uncle_headers.iter().map(|u| SignedHeader::new(header(&u.header), u.signature)).collect();
    let bounded: lb_utils::bounded::UpperBoundedVec<SignedHeader, { lb_cryptarchia_engine::MAX_UNCLES }> =
        list.try_into().map_err(|_| anyhow!("more uncles than a block holds"))?;
    let uncles = UncleHeaders::new(bounded);
    let uncle_bytes = uncles.encode().to_vec();
    lez_bedrock::check_uncles(&uncle_bytes).map_err(|e| anyhow!("uncles: {e:?}"))?;
    for u in &b.uncle_headers {
        ensure!(<[u8; 32]>::from(header(&u.header).id()) == <[u8; 32]>::from(u.header.id));
    }
    let node_root: [u8; 32] = body_root(&uncles, &b.transactions).into();
    ensure!(node_root == <[u8; 32]>::from(b.header.body_root), "the node's body_root is not the header's");

    let mut txs = Vec::new();
    for tx in &b.transactions {
        let ops = tx.op_refs().encode().to_vec();
        let hash: H256 = *tx.hash().as_signing_bytes();
        ensure!(lez_bedrock::tx_hash(&ops) == hash, "lez-bedrock's tx hash differs from the node's");
        let first = tx.op_refs_iter().next();
        let inscription = match (first, tx.op_proof_refs_iter().next()) {
            (Some(OpRef::ChannelInscribe(i)), Some(proof)) => {
                let OpProofRef::Ed25519Sig(sig) = proof else { bail!("an inscription without an Ed25519 proof") };
                let sig: [u8; 64] = sig.to_bytes();
                let m = lez_bedrock::first_inscription(&ops).map_err(|e| anyhow!("lez-bedrock refuses an inscription the node decodes: {e:?}"))?;
                let channel: H256 = *i.channel_id.as_ref();
                let signer: H256 = i.signer.to_bytes();
                ensure!(m.channel_id == channel && m.signer == signer && m.data == &i.inscription[..] && m.parent == *i.parent.as_ref(),
                    "lez-bedrock reads another inscription than the node");
                let node_ok = i.signer.verify(&hash, &Ed25519Signature::from_bytes(&sig)).is_ok();
                let mine_ok = lez_bedrock::verify_signature(&signer, &hash, &sig).is_ok();
                ensure!(node_ok == mine_ok, "the signature verdicts differ (node {node_ok}, lez-bedrock {mine_ok})");
                ensure!(node_ok, "an inscription in a block whose signature does not verify");
                Some((channel, signer, i.inscription.to_vec(), sig))
            }
            _ => {
                ensure!(lez_bedrock::first_inscription(&ops).is_err(), "lez-bedrock finds an inscription the node does not");
                None
            }
        };
        txs.push(Tx { ops, hash, inscription });
    }
    // lez-bedrock's own body root, from its Merkle tree over the same hashes.
    let leaves: Vec<H256> = txs.iter().map(|t| t.hash).collect();
    let root = tree_root(&leaves);
    ensure!(lez_bedrock::body_root(&uncle_bytes, &root) == <[u8; 32]>::from(b.header.body_root), "lez-bedrock's body root differs");
    for k in 0..leaves.len() {
        let (index, path) = path(&leaves, k);
        ensure!(lez_bedrock::merkle_root(&leaves[k], index, &path).ok() == Some(root), "a path that does not lead to the root");
    }
    Ok(Decoded { id, header: header_bytes, uncles: uncle_bytes, txs })
}

fn padded(leaves: &[H256]) -> Vec<H256> {
    let mut l = leaves.to_vec();
    l.resize(leaves.len().max(1).next_power_of_two(), [0u8; 32]);
    l
}

fn tree_root(leaves: &[H256]) -> H256 {
    let mut l = padded(leaves);
    while l.len() > 1 {
        l = l.chunks(2).map(|p| lez_bedrock::node(&p[0], &p[1])).collect();
    }
    l[0]
}

/// Leaf `k`'s index and siblings, leaf level first.
fn path(leaves: &[H256], k: usize) -> (u32, Vec<H256>) {
    let mut l = padded(leaves);
    let (mut i, mut out) = (k, Vec::new());
    while l.len() > 1 {
        out.push(l[i ^ 1]);
        l = l.chunks(2).map(|p| lez_bedrock::node(&p[0], &p[1])).collect();
        i /= 2;
    }
    (k as u32, out)
}

fn chain(dir: &Path) -> Result<Vec<String>> {
    let ids: Vec<String> = serde_json::from_str(&std::fs::read_to_string(dir.join("chain.json"))?)?;
    ensure!(!ids.is_empty(), "an empty chain");
    Ok(ids)
}

fn check(dir: &Path) -> Result<Value> {
    let ids = chain(dir)?;
    let (mut txs, mut ins, mut uncles, mut bytes) = (0usize, BTreeMap::<String, usize>::new(), 0usize, 0usize);
    let mut lez = BTreeMap::<String, (usize, u64, u64)>::new();
    let mut prev: Option<H256> = None;
    let mut slots = (u64::MAX, 0u64);
    for id in &ids {
        let raw = std::fs::read_to_string(dir.join(format!("{id}.json")))?;
        bytes += raw.len();
        let d = decode(&raw).with_context(|| format!("block {id}"))?;
        ensure!(hx(&d.id) == *id, "block {id} is another block");
        let h = lez_bedrock::Header::parse(&d.header).unwrap();
        if let Some(p) = prev {
            ensure!(h.parent() == p, "block {id} is not the child of the one before it");
        }
        prev = Some(d.id);
        slots = (slots.0.min(h.slot()), slots.1.max(h.slot()));
        uncles += (d.uncles[0]) as usize;
        txs += d.txs.len();
        for t in &d.txs {
            if let Some((c, _, data, _)) = &t.inscription {
                *ins.entry(hx(c)).or_default() += 1;
                // A LEZ v0.3.1 block, as lez-chain reads one: strict layout, hash of its contents.
                let p = lez_chain::Params { program_id: [0; 32], burns_account: [0; 32] };
                if let Ok((h, _)) = lez_chain::scan::scan_block(data, &p) {
                    if lez_chain::content_hash(data) == h.hash {
                        let e = lez.entry(hx(c)).or_insert((0, u64::MAX, 0));
                        *e = (e.0 + 1, e.1.min(h.block_id), e.2.max(h.block_id));
                    }
                }
            }
        }
    }
    Ok(json!({"blocks": ids.len(), "slots": [slots.0, slots.1], "transactions": txs, "uncle_headers": uncles,
              "inscriptions_by_channel": ins,
              "lez_v031_blocks_by_channel": lez.iter().map(|(c, (n, lo, hi))| (c.clone(), json!({"blocks": n, "ids": [lo, hi]}))).collect::<BTreeMap<_, _>>(), "json_bytes": bytes, "identical": true}))
}

fn anchor(dir: &Path, channel: H256, signer: H256, lez_hash: H256, depth: usize, out: &Path) -> Result<Value> {
    let ids = chain(dir)?;
    let read = |id: &String| -> Result<Decoded> { decode(&std::fs::read_to_string(dir.join(format!("{id}.json")))?).with_context(|| format!("block {id}")) };
    for (k, id) in ids.iter().enumerate() {
        let d = read(id)?;
        let Some(t) = d.txs.iter().position(|t| matches!(&t.inscription, Some((c, _, data, _)) if *c == channel && data.get(40..72) == Some(&lez_hash[..])))
        else { continue };
        ensure!(ids.len() > k + depth, "only {} headers above the inscription's block {id}, {depth} asked for", ids.len() - k - 1);
        let tx = &d.txs[t];
        let (_, s, _, sig) = tx.inscription.as_ref().unwrap();
        ensure!(*s == signer, "the inscription is signed by {}, not the pinned key", hx(s));
        let leaves: Vec<H256> = d.txs.iter().map(|t| t.hash).collect();
        let (tx_index, path) = path(&leaves, t);
        let descendants = ids[k + 1..=k + depth].iter().map(|i| Ok(read(i)?.header)).collect::<Result<Vec<_>>>()?;
        let a = AnchorBlock { header: d.header.clone(), uncles: d.uncles.clone(), ops: tx.ops.clone(), tx_index, path, signature: sig.to_vec(), descendants };
        let r = lez_bedrock::anchor(&Params { channel_id: channel, signer }, &a, depth as u32).map_err(|e| anyhow!("lez_bedrock::anchor: {e:?}"))?;
        ensure!(r.block_id == d.id && hx(&r.top_id) == ids[k + depth], "lez_bedrock::anchor ends elsewhere than the node's chain");
        let lez = r.inscription.data;
        let rec = json!({
            "channel_id": hx(&channel), "signer": hx(&signer), "lez_hash": hx(&lez_hash),
            "lez_block_id": u64::from_le_bytes(lez[0..8].try_into().unwrap()),
            "bedrock_block": {"id": id, "slot": r.block_slot, "tx_index": tx_index, "txs": leaves.len(), "path_len": a.path.len(),
                              "tx_hash": hx(&r.tx_hash), "ops_bytes": a.ops.len(), "inscription_bytes": lez.len(), "uncles": a.uncles[0]},
            "top": {"id": hx(&r.top_id), "slot": r.top_slot, "depth": r.depth},
            "anchor_block": {"header": hx(&a.header), "uncles": hx(&a.uncles), "ops": hx(&a.ops), "tx_index": a.tx_index,
                             "path": a.path.iter().map(|p| hx(p)).collect::<Vec<_>>(), "signature": hx(&a.signature),
                             "descendants": a.descendants.iter().map(|h| hx(h)).collect::<Vec<_>>()},
        });
        std::fs::write(out, serde_json::to_string_pretty(&rec)?)?;
        return Ok(json!({"bedrock_block": rec["bedrock_block"], "top": rec["top"], "lez_block_id": rec["lez_block_id"], "out": out}));
    }
    bail!("no block in {} inscribes a LEZ block with hash {} on channel {}", dir.display(), hx(&lez_hash), hx(&channel))
}

fn forge(key: &Path, lez_block: &Path, channel: H256, depth: usize, out: &Path) -> Result<Value> {
    let seed: [u8; 32] = std::fs::read(key)?.try_into().map_err(|_| anyhow!("{} is not a 32-byte key", key.display()))?;
    let k = lb_kms_keys::keys::Ed25519Key::from_bytes(&seed);
    let signer: H256 = k.public_key().to_bytes();
    let lez = std::fs::read(lez_block)?;
    let mut ops = vec![1u8, lez_bedrock::OP_CHANNEL_INSCRIBE];
    ops.extend_from_slice(&channel);
    ops.extend_from_slice(&(lez.len() as u32).to_le_bytes());
    ops.extend_from_slice(&lez);
    ops.extend_from_slice(&[0u8; 32]);
    ops.extend_from_slice(&signer);
    let txh = lez_bedrock::tx_hash(&ops);
    let sig = k.sign_payload(&txh).to_bytes().to_vec();
    let made_up = |slot: u64, parent: &H256, body: &H256| -> Vec<u8> {
        [&slot.to_le_bytes()[..], parent, body, &[0x5a; 128 + 32 + 32 + 32][..]].concat()
    };
    let uncles = vec![0u8];
    let header = made_up(1 << 40, &[0x77; 32], &lez_bedrock::body_root(&uncles, &txh));
    let mut top = lez_bedrock::Header::parse(&header).unwrap().id();
    let mut descendants = Vec::new();
    for i in 0..depth {
        let d = made_up((1 << 40) + 1 + i as u64, &top, &[i as u8; 32]);
        top = lez_bedrock::Header::parse(&d).unwrap().id();
        descendants.push(d);
    }
    let a = AnchorBlock { header, uncles, ops, tx_index: 0, path: vec![], signature: sig, descendants };
    let r = lez_bedrock::anchor(&Params { channel_id: channel, signer }, &a, depth as u32).map_err(|e| anyhow!("lez_bedrock::anchor: {e:?}"))?;
    let rec = json!({
        "forged": true, "channel_id": hx(&channel), "signer": hx(&signer), "lez_hash": hx(&lez[40..72]),
        "lez_block_id": u64::from_le_bytes(lez[0..8].try_into()?),
        "bedrock_block": {"id": hx(&r.block_id), "slot": r.block_slot, "tx_index": 0, "txs": 1, "path_len": 0,
                          "tx_hash": hx(&r.tx_hash), "ops_bytes": a.ops.len(), "inscription_bytes": lez.len(), "uncles": 0},
        "top": {"id": hx(&r.top_id), "slot": r.top_slot, "depth": r.depth},
        "anchor_block": {"header": hx(&a.header), "uncles": hx(&a.uncles), "ops": hx(&a.ops), "tx_index": 0, "path": Vec::<String>::new(),
                         "signature": hx(&a.signature), "descendants": a.descendants.iter().map(|h| hx(h)).collect::<Vec<_>>()},
    });
    std::fs::write(out, serde_json::to_string_pretty(&rec)?)?;
    Ok(json!({"forged": true, "signer": hx(&signer), "bedrock_block": rec["bedrock_block"], "top": rec["top"], "out": out}))
}

fn arg(args: &[String], name: &str) -> Result<String> {
    let i = args.iter().position(|a| a == name).ok_or_else(|| anyhow!("missing {name}"))?;
    args.get(i + 1).cloned().ok_or_else(|| anyhow!("{name} needs a value"))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let out = match args.get(1).map(String::as_str) {
        Some("check") => check(Path::new(args.get(2).ok_or_else(|| anyhow!("check DIR"))?))?,
        Some("anchor") => anchor(
            Path::new(args.get(2).ok_or_else(|| anyhow!("anchor DIR ..."))?),
            h256(&arg(&args, "--channel")?)?,
            h256(&arg(&args, "--signer")?)?,
            h256(&arg(&args, "--lez-hash")?)?,
            arg(&args, "--depth")?.parse()?,
            Path::new(&arg(&args, "--out")?),
        )?,
        Some("forge") => forge(
            Path::new(&arg(&args, "--key")?),
            Path::new(&arg(&args, "--lez-block")?),
            h256(&arg(&args, "--channel")?)?,
            arg(&args, "--depth")?.parse()?,
            Path::new(&arg(&args, "--out")?),
        )?,
        _ => bail!("usage: bedrock-tools check DIR | anchor DIR --channel HEX --signer HEX --lez-hash HEX --depth K --out FILE | forge --key FILE --lez-block FILE --channel HEX --depth K --out FILE"),
    };
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}

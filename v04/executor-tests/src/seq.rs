//! A LEZ v0.4.0-rc3 sequencer over its JSON-RPC (`lez/sequencer/service/rpc`):
//! the calls the v0.4 client (`wrap`, src/bin/wrap.rs) makes, a signed public
//! transaction that declares its fee, and a program loaded through the
//! program loader. Plain HTTP/1.1 on a local socket: the sequencer is local.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use base64::Engine as _;
use borsh::BorshSerialize;
use common::{block::Block, transaction::LeeTransaction, HashType};
use lee::{
    program::Program,
    public_transaction::{Message, WitnessSet},
    Account, AccountId, Actor, FeeDeclaration, PrivateKey, PublicAccountEvidence, PublicTransaction,
};
use lee_core::program::{PdaSeed, PROGRAM_LOADER_ACCOUNT_ID};
use serde_json::{json, Value};

use crate::Key;

/// The execution gas every transaction declares (one block holds 10M).
pub const GAS_LIMIT: u64 = 4_000_000;
/// The wallet's `max_fee_for`: `(gas_limit + 100_000) * 64`.
pub const MAX_FEE: u128 = (GAS_LIMIT as u128 + 100_000) * 64;
/// The program loader's segment size.
pub const SEGMENT: usize = 96 * 1024;

/// The key that pays every fee: funded at genesis (v04/scripts/e2e-seq.sh).
#[must_use]
pub fn payer() -> Key {
    Key::new(900)
}

pub struct Rpc {
    addr: String,
    id: u64,
}

impl Rpc {
    #[must_use]
    pub fn new(url: &str) -> Self {
        Self { addr: url.trim_start_matches("http://").trim_end_matches('/').to_string(), id: 0 }
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.id += 1;
        let body = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params}).to_string();
        let mut s = TcpStream::connect(&self.addr).map_err(|e| format!("connect {}: {e}", self.addr))?;
        s.set_read_timeout(Some(Duration::from_secs(120))).map_err(|e| e.to_string())?;
        write!(s, "POST / HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", self.addr, body.len()).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        s.read_to_end(&mut out).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out);
        let (head, rest) = text.split_once("\r\n\r\n").ok_or("no HTTP body")?;
        let payload = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") { dechunk(rest) } else { rest.to_string() };
        let v: Value = serde_json::from_str(&payload).map_err(|e| format!("{e}: {payload}"))?;
        if let Some(e) = v.get("error") {
            return Err(e.to_string());
        }
        Ok(v["result"].clone())
    }

    pub fn nonce(&mut self, id: AccountId) -> lee_core::account::Nonce {
        let v = self.call("getAccountsNonces", json!([[id]])).expect("getAccountsNonces");
        serde_json::from_value::<Vec<lee_core::account::Nonce>>(v).expect("nonces")[0]
    }

    /// The whole account (every actor state), or `None` if the chain has none.
    pub fn account(&mut self, id: AccountId) -> Option<Account> {
        let v = self.call("getAccount", json!([id])).expect("getAccount");
        serde_json::from_value(v).expect("an account")
    }

    /// One actor's state as the chain holds it now (empty if none).
    pub fn actor_state(&mut self, actor: Actor) -> Vec<u8> {
        let v = self.call("getAccountView", json!([actor])).expect("getAccountView");
        let a: Option<Account> = serde_json::from_value(v).expect("an account view");
        a.map(|a| a.data.actor_state(actor.program_account_id).to_vec()).unwrap_or_default()
    }

    pub fn block(&mut self, id: u64) -> Option<(Vec<u8>, Block)> {
        let v = self.call("getBlock", json!([id])).expect("getBlock");
        if v.is_null() {
            return None;
        }
        let raw = base64::engine::general_purpose::STANDARD.decode(v.as_str().expect("a base64 block")).expect("base64");
        let b: Block = borsh::from_slice(&raw).expect("a v0.4 block");
        Some((raw, b))
    }

    pub fn last_block(&mut self) -> u64 {
        self.call("getLastBlockId", json!([])).expect("getLastBlockId").as_u64().expect("a block id")
    }
}

fn dechunk(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some((len, tail)) = rest.split_once("\r\n") {
        let n = usize::from_str_radix(len.trim(), 16).unwrap_or(0);
        if n == 0 {
            break;
        }
        out.push_str(&tail[..n]);
        rest = &tail[n + 2..];
    }
    out
}

/// A transaction the sequencer included, and whether its block lists it as reverted.
pub struct Landed {
    pub block: u64,
    pub index: usize,
    pub hash: HashType,
    pub reverted: bool,
}

/// The public transaction `to ← message`, declaring `actors`, signed by
/// `signers` and the payer, with admission evidence for `pdas`, and the fee
/// the payer pays (`fee: false` builds the same call without one, for the
/// local pre-check).
pub fn transaction(rpc: &mut Rpc, to: Actor, actors: &[Actor], signers: &[&Key], pdas: &[(AccountId, PdaSeed)], message: &impl BorshSerialize, fee: bool) -> PublicTransaction {
    let p = payer();
    let mut keys: Vec<&Key> = signers.to_vec();
    if !keys.iter().any(|k| k.id == p.id) {
        keys.push(&p);
    }
    let nonces = keys.iter().map(|k| (k.id, rpc.nonce(k.id))).collect();
    let evidence = pdas.iter().map(|(program, seed)| PublicAccountEvidence::Pda { program: *program, seed: *seed }).collect();
    let data = Program::serialize_message(message).expect("serializes");
    let mut all = actors.to_vec();
    all.push(to);
    let fee = fee.then(|| FeeDeclaration::new(p.id, GAS_LIMIT, 0, MAX_FEE));
    let msg = Message::new(to, data, all, nonces, fee, evidence);
    let sks: Vec<&PrivateKey> = keys.iter().map(|k| &k.sk).collect();
    PublicTransaction::new(msg.clone(), WitnessSet::for_message(&msg, &sks))
}

/// Submit and wait for the block that includes it.
pub fn submit(rpc: &mut Rpc, label: &str, tx: PublicTransaction) -> Result<Landed, String> {
    let tx = LeeTransaction::Public(tx);
    let want = tx.hash();
    let h = rpc.call("sendTransaction", json!([tx])).map_err(|e| format!("{label}: sendTransaction refused: {e}"))?;
    let h: HashType = serde_json::from_value(h).map_err(|e| e.to_string())?;
    if h != want {
        return Err(format!("{label}: the sequencer returned another hash"));
    }
    let start = Instant::now();
    loop {
        let v = rpc.call("getTransaction", json!([h]))?;
        if !v.is_null() {
            let block = v[1].as_u64().ok_or("a block id")?;
            let (_, b) = rpc.block(block).ok_or("the block")?;
            let index = b.body.transactions.iter().position(|t| t.hash() == h).ok_or("in its block")?;
            return Ok(Landed { block, index, hash: h, reverted: b.body.is_reverted(index) });
        }
        if start.elapsed() > Duration::from_secs(180) {
            return Err(format!("{label}: not included after 180 s"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// `program_loader_core::Message` (lez/programs/program_loader/core, v0.4.0-rc3), mirrored.
#[derive(BorshSerialize)]
pub enum LoaderMessage {
    WriteSegment { bytecode: Vec<u8>, next_segment: Option<AccountId> },
    CreateHeader { first_segment: AccountId, immutable: bool },
}

/// The program loader's transactions for `bin` at the header account of
/// `Key::new(seed)`: segments tail to head (segment `i` at `Key::new(seed + 1 + i)`),
/// then an immutable header. Each is `(label, to, signer, message)`.
#[must_use]
pub fn deploy_steps(name: &str, bin: &[u8], seed: u16) -> (AccountId, Vec<(String, Actor, Key, LoaderMessage)>) {
    let pb = risc0_binfmt::ProgramBinary::decode(bin).expect("a program binary");
    assert_eq!(pb.kernel_elf, risc0_zkos_v1compat::V1COMPAT_ELF, "the v1compat kernel");
    let chunks: Vec<&[u8]> = pb.user_elf.chunks(SEGMENT).collect();
    let segs: Vec<Key> = (0..chunks.len()).map(|i| Key::new(seed + 1 + i as u16)).collect();
    let header = Key::new(seed);
    let header_id = header.id;
    let first = segs[0].id;
    let mut steps = Vec::new();
    for i in (0..chunks.len()).rev() {
        let next = segs.get(i + 1).map(|k| k.id);
        let k = Key::new(seed + 1 + i as u16);
        steps.push((format!("deploy {name} segment {i}"), Actor::new(k.id, PROGRAM_LOADER_ACCOUNT_ID), k, LoaderMessage::WriteSegment { bytecode: chunks[i].to_vec(), next_segment: next }));
    }
    steps.push((format!("deploy {name} header"), Actor::new(header_id, PROGRAM_LOADER_ACCOUNT_ID), header, LoaderMessage::CreateHeader { first_segment: first, immutable: true }));
    (header_id, steps)
}

// ---- the private claim (src/private.rs): what it needs from the sequencer ----

impl Rpc {
    /// The base fees the sequencer quotes now (execution, storage).
    pub fn fee_quote(&mut self) -> (u64, u64) {
        let q = self.call("getFeeState", json!([])).expect("getFeeState");
        (q["base_fee_exec"].as_u64().expect("base_fee_exec"), q["base_fee_stor"].as_u64().expect("base_fee_stor"))
    }

    /// The genesis credit pending for the private `payer`: the message, its
    /// amount and its path in the message tree.
    pub fn genesis_credit(&mut self, payer: AccountId) -> (lee_core::program::MessageBody, u128, lee_core::MembershipProof) {
        let v = self.call("getPublications", json!([0, 1000])).expect("getPublications");
        let pubs: Vec<(u64, lee::Publication)> = serde_json::from_value(v).expect("publications");
        let (pos, body, amount) = pubs
            .into_iter()
            .find_map(|(pos, p)| match p {
                lee::Publication::Clear { body, .. } if body.to == Actor::native_balance(payer) => match borsh::from_slice(&body.message) {
                    Ok(lee_core::native_token::Message::Credit(a)) => Some((pos, body, a)),
                    _ => None,
                },
                _ => None,
            })
            .expect("a genesis credit for the private fee payer (networks/local.json funds it)");
        let path: Option<lee_core::MembershipProof> = serde_json::from_value(self.call("getMessagePath", json!([pos])).expect("getMessagePath")).expect("a path");
        (body, amount, path.expect("the credit is in the message tree"))
    }

    /// One public actor's state, as the proof's simulation assumes it.
    pub fn actor_view(&mut self, a: Actor) -> lee_core::account::ActorState {
        let v = self.call("getAccountView", json!([a])).expect("getAccountView");
        if v.is_null() {
            return Vec::<u8>::new().into();
        }
        let acct: Account = serde_json::from_value(v).expect("an account");
        acct.data.actor_state(a.program_account_id).clone()
    }
}

/// Submit a proven privacy-preserving transaction and wait for its block.
pub fn submit_private(rpc: &mut Rpc, label: &str, tx: &lee::PrivacyPreservingTransaction) -> Result<Landed, String> {
    let tx = LeeTransaction::PrivacyPreserving(tx.clone());
    let want = tx.hash();
    let h = rpc.call("sendTransaction", json!([tx])).map_err(|e| format!("{label}: sendTransaction refused: {e}"))?;
    let h: HashType = serde_json::from_value(h).map_err(|e| e.to_string())?;
    if h != want {
        return Err(format!("{label}: the sequencer returned another hash"));
    }
    let start = Instant::now();
    loop {
        let v = rpc.call("getTransaction", json!([h]))?;
        if !v.is_null() {
            let block = v[1].as_u64().ok_or("a block id")?;
            let (_, b) = rpc.block(block).ok_or("the block")?;
            let index = b.body.transactions.iter().position(|t| t.hash() == h).ok_or("in its block")?;
            return Ok(Landed { block, index, hash: h, reverted: b.body.is_reverted(index) });
        }
        if start.elapsed() > Duration::from_secs(600) {
            return Err(format!("{label}: not included after 600 s"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// `fee_core::Message::Distribute`, the block's forced fee transaction, mirrored.
#[derive(borsh::BorshDeserialize)]
#[allow(dead_code)]
enum FeeMessage {
    Distribute { summary: FeeSummary, payout: u128, producer: AccountId },
}

#[derive(borsh::BorshDeserialize, Debug, Clone, Copy, Default)]
pub struct FeeSummary {
    pub gas_used_exec: u64,
    pub gas_used_stor: u64,
    pub revenue_base: u128,
    pub revenue_tip: u128,
}

/// The fee summary a block's forced fee transaction carries.
#[must_use]
pub fn block_fees(b: &Block) -> Option<FeeSummary> {
    b.body.transactions.iter().find_map(|t| match t {
        LeeTransaction::Public(p) => match borsh::from_slice::<FeeMessage>(&p.message.execution.root.message) {
            Ok(FeeMessage::Distribute { summary, .. }) => Some(summary),
            Err(_) => None,
        },
        LeeTransaction::PrivacyPreserving(_) => None,
    })
}

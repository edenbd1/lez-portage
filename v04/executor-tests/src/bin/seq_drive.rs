//! Drive the v0.4 port on a local LEZ v0.4.0-rc3 standalone sequencer
//! (v04/scripts/e2e-seq.sh), over its JSON-RPC, with signed public
//! transactions that each declare a fee paid by one funded key (`payer`,
//! supplied at genesis):
//!
//! ```text
//! seq_drive pubkey                      the payer's public key, hex (for the genesis)
//! seq_drive deploy RPC DIR              load token, sextant_lc, wrap_bridge through the
//!                                       program loader; DIR/programs.json, DIR/plan.json
//! seq_drive run RPC DIR                 on DIR/anvil (scripts/anvil_deposits.py): Sextant's Init on
//!                                       the devnet header of the anvil block, the bridge's Init,
//!                                       Register, a holding, a claim, burns; DIR/manifest.tsv,
//!                                       DIR/blocks/<id>.borsh for every block from the first
//! ```
//!
//! Nothing here reaches a public network: RPC is a local sequencer.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine as _;
use borsh::BorshSerialize;
use common::{block::Block, transaction::LeeTransaction, HashType};
use lee::{
    program::Program,
    public_transaction::{Message, WitnessSet},
    AccountId, Actor, FeeDeclaration, PrivateKey, PublicAccountEvidence, PublicTransaction,
};
use lee_core::program::{PdaSeed, PROGRAM_LOADER_ACCOUNT_ID};
use serde_json::{json, Value};
use wrap_bridge_v04::{Config, Message as Bm};
use wrap_executor_tests_v04::*;

const GAS_LIMIT: u64 = 4_000_000;
/// The wallet's `max_fee_for`: `(gas_limit + 100_000) * 64`.
const MAX_FEE: u128 = (GAS_LIMIT as u128 + 100_000) * 64;
const SEGMENT: usize = 96 * 1024;

fn payer() -> Key {
    Key::new(900)
}

struct Rpc {
    addr: String,
    id: u64,
}

impl Rpc {
    fn new(url: &str) -> Self {
        Self { addr: url.trim_start_matches("http://").trim_end_matches('/').to_string(), id: 0 }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.id += 1;
        let body = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params}).to_string();
        let mut s = TcpStream::connect(&self.addr).map_err(|e| format!("connect {}: {e}", self.addr))?;
        s.set_read_timeout(Some(Duration::from_secs(120))).unwrap();
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

    fn nonce(&mut self, id: AccountId) -> lee_core::account::Nonce {
        let v = self.call("getAccountsNonces", json!([[id]])).expect("getAccountsNonces");
        serde_json::from_value::<Vec<lee_core::account::Nonce>>(v).expect("nonces")[0]
    }

    fn block(&mut self, id: u64) -> Option<(Vec<u8>, Block)> {
        let v = self.call("getBlock", json!([id])).expect("getBlock");
        if v.is_null() {
            return None;
        }
        let raw = base64::engine::general_purpose::STANDARD.decode(v.as_str().expect("a base64 block")).expect("base64");
        let b: Block = borsh::from_slice(&raw).expect("a v0.4 block");
        Some((raw, b))
    }

    fn last_block(&mut self) -> u64 {
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

/// One submitted transaction's verdict.
struct Landed {
    reverted: bool,
}

struct Driver {
    rpc: Rpc,
    manifest: Vec<String>,
}

impl Driver {
    /// Sign and submit `message` to `to`, declaring `actors`, signed by
    /// `signers` and the payer, and wait for its block.
    fn send(&mut self, label: &str, expect_applied: bool, to: Actor, actors: &[Actor], signers: &[&Key], pdas: &[(AccountId, PdaSeed)], message: &impl BorshSerialize) -> Landed {
        let p = payer();
        let mut keys: Vec<&Key> = signers.to_vec();
        if !keys.iter().any(|k| k.id == p.id) {
            keys.push(&p);
        }
        let nonces = keys.iter().map(|k| (k.id, self.rpc.nonce(k.id))).collect();
        let evidence = pdas.iter().map(|(program, seed)| PublicAccountEvidence::Pda { program: *program, seed: *seed }).collect();
        let data = Program::serialize_message(message).expect("serializes");
        let mut all = actors.to_vec();
        all.push(to);
        let msg = Message::new(to, data, all, nonces, Some(FeeDeclaration::new(p.id, GAS_LIMIT, 0, MAX_FEE)), evidence);
        let sks: Vec<&PrivateKey> = keys.iter().map(|k| &k.sk).collect();
        let tx = LeeTransaction::Public(PublicTransaction::new(msg.clone(), WitnessSet::for_message(&msg, &sks)));
        let want = tx.hash();
        let h = self.rpc.call("sendTransaction", json!([tx])).unwrap_or_else(|e| panic!("{label}: sendTransaction refused: {e}"));
        let h: HashType = serde_json::from_value(h).expect("a tx hash");
        assert_eq!(h, want);
        let start = Instant::now();
        loop {
            let v = self.rpc.call("getTransaction", json!([h])).expect("getTransaction");
            if !v.is_null() {
                let block = v[1].as_u64().expect("a block id");
                let (_, b) = self.rpc.block(block).expect("the block");
                let index = b.body.transactions.iter().position(|t| t.hash() == h).expect("in its block");
                let reverted = b.body.is_reverted(index);
                let verdict = if reverted { "reverted" } else { "applied" };
                println!("{label}: block {block} tx {index} {verdict}");
                assert_eq!(!reverted, expect_applied, "{label}: expected {}", if expect_applied { "applied" } else { "reverted" });
                self.manifest.push(format!("{label}\t{}\t{verdict}\t{block}\t{index}\t{h}", if expect_applied { "applied" } else { "reverted" }));
                return Landed { reverted };
            }
            assert!(start.elapsed() < Duration::from_secs(180), "{label}: not included after 180 s");
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Load a program binary through the program loader at a fresh header
    /// account: segments tail to head, then an immutable header.
    fn deploy(&mut self, name: &str, seed: u16) -> AccountId {
        let bin = bin(name);
        let pb = risc0_binfmt::ProgramBinary::decode(&bin).expect("a program binary");
        assert_eq!(pb.kernel_elf, risc0_zkos_v1compat::V1COMPAT_ELF, "the v1compat kernel");
        let chunks: Vec<&[u8]> = pb.user_elf.chunks(SEGMENT).collect();
        let segs: Vec<Key> = (0..chunks.len()).map(|i| Key::new(seed + 1 + i as u16)).collect();
        let header = Key::new(seed);
        for i in (0..chunks.len()).rev() {
            let next = segs.get(i + 1).map(|k| k.id);
            let m = program_loader_core_msg::Message::WriteSegment { bytecode: chunks[i].to_vec(), next_segment: next };
            self.send(&format!("deploy {name} segment {i}"), true, Actor::new(segs[i].id, PROGRAM_LOADER_ACCOUNT_ID), &[], &[&segs[i]], &[], &m);
        }
        let m = program_loader_core_msg::Message::CreateHeader { first_segment: segs[0].id, immutable: true };
        self.send(&format!("deploy {name} header"), true, Actor::new(header.id, PROGRAM_LOADER_ACCOUNT_ID), &[], &[&header], &[], &m);
        header.id
    }
}

/// `program_loader_core::Message` (lez/programs/program_loader/core, v0.4.0-rc3), mirrored.
mod program_loader_core_msg {
    use borsh::BorshSerialize;
    use lee::AccountId;
    #[derive(BorshSerialize)]
    pub enum Message {
        WriteSegment { bytecode: Vec<u8>, next_segment: Option<AccountId> },
        CreateHeader { first_segment: AccountId, immutable: bool },
    }
}

fn hx(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

#[derive(serde::Deserialize)]
struct Programs {
    token: AccountId,
    sextant_lc: AccountId,
    wrap_bridge: AccountId,
}

fn run_config(p: &Programs, vault: [u8; 20]) -> Config {
    Config {
        deployment_id: DEPLOYMENT_ID,
        lez_chain_id: LEZ_CHAIN_ID,
        zone_id: ZONE_ID,
        sextant: p.sextant_lc,
        instance_id: INSTANCE,
        chain_id: ANVIL_CHAIN_ID,
        vault,
        token_program: p.token,
        admin: admin().id,
        freezer: freezer().id,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("pubkey") => println!("{}", hex::encode(payer().pk.value())),
        Some("deploy") => {
            let dir = PathBuf::from(&args[3]);
            std::fs::create_dir_all(&dir).unwrap();
            let mut d = Driver { rpc: Rpc::new(&args[2]), manifest: vec![] };
            let token = d.deploy("token.bin", 2000);
            let sa = d.deploy("sextant_lc.bin", 2100);
            let br = d.deploy("wrap_bridge.bin", 2200);
            std::fs::write(dir.join("programs.json"), json!({"token": token, "sextant_lc": sa, "wrap_bridge": br}).to_string()).unwrap();
            // The vault is deployed with this deployment's pairing; alice's deposits commit to her account.
            let pairing = wrap_core::pairing(&LEZ_CHAIN_ID, &ZONE_ID, br.value(), &DEPLOYMENT_ID);
            let dep = |n: u64, amount: &str| {
                let c = wrap_core::commitment(&pairing, alice().id.value(), &secret(&ALICE_SEED, n));
                json!({"from": "alice", "token": "eth", "amount": amount, "commitment": hx(&c)})
            };
            let phantom = wrap_core::commitment(&pairing, alice().id.value(), &secret(&ALICE_SEED, 9));
            let plan = json!({"pairing": hx(&pairing), "phantom": hx(&phantom), "deposits": [dep(0, "1000000000000000000"), dep(1, "1000000000000000000")]});
            std::fs::write(dir.join("plan.json"), plan.to_string()).unwrap();
            std::fs::write(dir.join("deploy.tsv"), d.manifest.join("\n") + "\n").unwrap();
            println!("token {token}\nsextant_lc {sa}\nwrap_bridge {br}\npairing {}", hx(&pairing));
        }
        Some("run") => run(&args[2], Path::new(&args[3])),
        _ => {
            eprintln!("usage: seq_drive pubkey | deploy RPC DIR | run RPC DIR");
            std::process::exit(2);
        }
    }
}

fn run(rpc: &str, dir: &Path) {
    let p: Programs = serde_json::from_str(&std::fs::read_to_string(dir.join("programs.json")).unwrap()).unwrap();
    let a = anvil_at(&dir.join("anvil"));
    let cfg = run_config(&p, a.vault);
    let (tp, sa, me) = (p.token, p.sextant_lc, p.wrap_bridge);
    assert_eq!(wrap_bridge_v04::pairing(&me, &cfg), a.pairing, "the vault's pairing is this deployment's");
    let mut d = Driver { rpc: Rpc::new(rpc), manifest: vec![] };
    let first = d.rpc.last_block() + 1;
    let br = |id| Actor::new(id, me);
    let tk = |id| Actor::new(id, tp);
    let ca = wrap_bridge_v04::config_account(&me, &cfg);
    let entry = wrap_bridge_v04::entry_account(&me, &ca, &ETH);
    let def = wrap_bridge_v04::definition_account(&me, &ca, &ETH, &SALT).0;
    let burns = wrap_bridge_v04::burns_account(&me, &ca);

    // Sextant's Init on the devnet beacon header of the anvil block: the light
    // client verifies the bootstrap and records the header claims attest against.
    // The devnet's slot starts now, so the sequencer's clock accepts the checkpoint.
    let slot = SLOT;
    let now_s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let dn = devnet_at(&a.block, slot, now_s - 60);
    let inst = sextant_program_v04::instance_account(&sa, &INSTANCE);
    let hdr = sextant_program_v04::header_account(&sa, &INSTANCE, slot);
    d.send("Sextant Init (devnet header of the anvil block)", true, Actor::new(inst, sa), &[Actor::new(hdr, sa)], &[], &[(sa, sextant_program_v04::instance_seed(&INSTANCE))], &init_message(&dn));
    let (_, cseed) = wrap_bridge_v04::config_pda(&me, &cfg);
    d.send("Init", true, br(ca), &[br(burns)], &[], &[(me, cseed)], &Bm::Init { config: cfg.clone() });
    let reg = Bm::Register { config: cfg.clone(), eth_token: ETH, name: "Wrapped Ether (anvil)".into(), salt: SALT, policy: eth_policy() };
    d.send("Register ETH", true, br(admin().id), &[br(ca), br(entry), tk(def), tk(entry)], &[&admin()], &[], &reg);
    d.send("alice InitializeHolding", true, tk(alice().id), &[tk(def)], &[&alice()], &[], &wrap_bridge_v04::token::Message::InitializeHolding { definition: def });

    // Claims: both deposits, by the payer as relayer; then a replay (reverted).
    for (n, dep) in a.deposits.iter().enumerate() {
        let c = claim_message(&a, &cfg, dep, a.deposits.len(), alice().id, secret(&ALICE_SEED, n as u64), slot);
        let (to, actors, pda) = claim_actors_with(&me, &cfg, &c);
        d.send(&format!("Claim deposit {n}"), true, to, &actors, &[], &[pda], &Bm::Claim(Box::new(c)));
    }
    let c = claim_message(&a, &cfg, &a.deposits[0], a.deposits.len(), alice().id, secret(&ALICE_SEED, 0), slot);
    let (to, actors, _) = claim_actors_with(&me, &cfg, &c);
    d.send("Claim deposit 0 again (replay)", false, to, &actors, &[], &[], &Bm::Claim(Box::new(c)));
    // A tree the vault never had: the path is consistent, but Sextant refuses the
    // statement against the header it recorded (the slot is not in the proof, E7207).
    let d1 = &a.deposits[1];
    let mut c = claim_message(&a, &cfg, d1, a.deposits.len(), alice().id, secret(&ALICE_SEED, 1), slot);
    let fake = vec![a.deposits[0].leaf, d1.leaf, [0xab; 32]];
    c.index = 1;
    c.path = wrap_core::path(&fake, 1);
    c.root = wrap_core::root(&fake);
    let mut p = a.unknown.1.clone();
    p.slot = wrap_core::known_root_slot(&c.root);
    c.proof = p;
    let (to, actors, pda) = claim_actors_with(&me, &cfg, &c);
    d.send("Claim against a root the vault never had (Sextant refuses)", false, to, &actors, &[], &[pda], &Bm::Claim(Box::new(c)));

    // Burns: two applied, two reverted (not a denomination; more than alice holds after the others).
    let burn_actors = vec![br(ca), br(entry), tk(alice().id), tk(entry), tk(def)];
    let burn = |amount: u128, n: u8| Bm::Burn { eth_token: ETH, amount, eth_commitment: wrap_core::eth_commitment(&[0x42; 20], 0, &[n; 32]), salt: SALT, holder: alice().id, config: cfg.clone() };
    let mut leaves = Vec::new();
    let mut emit = |d: &mut Driver, label: &str, ok: bool, m: Bm| {
        if let Bm::Burn { eth_token, amount, eth_commitment, .. } = &m {
            let l = d.send(label, ok, br(burns), &burn_actors, &[&alice()], &[], &m);
            if !l.reverted {
                leaves.push(wrap_core::burn_leaf(&a.pairing, eth_token, *amount, eth_commitment));
            }
        }
    };
    emit(&mut d, "Burn 0.1 ETH", true, burn(100_000_000_000_000_000, 1));
    emit(&mut d, "Burn 12345 wei (not a denomination)", false, burn(12345, 2));
    emit(&mut d, "Burn 1 ETH", true, burn(1_000_000_000_000_000_000, 3));
    emit(&mut d, "Burn 1 ETH (more than alice holds)", false, burn(1_000_000_000_000_000_000, 4));

    // The program's own burn tree, read back from the sequencer's state: the
    // applied burns, in order, and only those.
    let v = d.rpc.call("getAccountView", json!([br(burns)])).expect("getAccountView");
    let acct: lee::Account = serde_json::from_value(v).expect("the burn tree account");
    let f: wrap_core::Frontier = borsh::from_slice(acct.data.actor_state(me)).expect("a frontier");
    assert_eq!((f.next_index, f.root), (leaves.len() as u64, wrap_core::root(&leaves)), "the program's tree is the applied burns'");
    println!("burn tree on chain: {} leaves, root {}", f.next_index, hx(&f.root));

    // Every block from the first of this run to the tip.
    let last = d.rpc.last_block();
    let blocks = dir.join("blocks");
    std::fs::create_dir_all(&blocks).unwrap();
    for id in first..=last {
        let (raw, _) = d.rpc.block(id).expect("the block");
        std::fs::write(blocks.join(format!("{id}.borsh")), &raw).unwrap();
    }
    let mut m = vec!["# label\texpected\tverdict\tblock\ttx\thash".to_string()];
    m.extend(d.manifest.iter().cloned());
    m.push(format!("# programs token {tp} sextant_lc {sa} wrap_bridge {me}"));
    m.push(format!("# config {ca} burns {burns} pairing {}", hx(&a.pairing)));
    m.push(format!("# blocks {first}..={last}"));
    m.push(format!("# burn leaves (applied burns, in order) {}", leaves.iter().map(|l| hx(l)).collect::<Vec<_>>().join(" ")));
    m.push(format!("# burn tree root {} (the program's own frontier, read back: {} leaves, root {})", hx(&wrap_core::root(&leaves)), f.next_index, hx(&f.root)));
    std::fs::write(dir.join("manifest.tsv"), m.join("\n") + "\n").unwrap();
    println!("blocks {first}..={last}, {} applied burns, root {}", leaves.len(), hx(&wrap_core::root(&leaves)));
}

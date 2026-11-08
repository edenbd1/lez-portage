//! A one-node LEZ v0.4.0-rc1 chain in memory with three actor-model programs
//! deployed: the canonical token program (lez-programs' actor-model port),
//! the Attest half of Sextant (`v04/programs/sextant_attest`), and
//! `wrap_bridge` (`v04/programs/wrap_bridge`).
//!
//! Transactions are signed `PublicTransaction`s and go through
//! `ValidatedStateDiff::from_public_transaction_metered`, the call the
//! sequencer's settlement makes for a charged transaction
//! (`chain_state::apply::settle_charged_transaction`), then
//! `V03State::apply_state_diff`: every actor runs in the RISC Zero executor,
//! and the cycle count is what the transaction is charged for. A refusal is a
//! *revert*: the transaction stays (its signers' nonces advance), its effects
//! do not.
//!
//! The Ethereum side is the anvil run of vectors/anvil, as for v0.3. Its block
//! is recorded in the Attest program by the record writer key (a stand-in for
//! Sextant's light client, not ported to v0.4); the storage proofs, the
//! deposits' leaves and paths, the nullifiers and the mints are checked
//! exactly as on v0.3.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use borsh::BorshSerialize;
use lee::{
    program::Program,
    public_transaction::{Message, WitnessSet},
    AccountId, Actor, PrivateKey, PublicAccountEvidence, PublicKey, PublicTransaction, V03State, ValidatedStateDiff,
};
use lee_core::program::TransactionEvent;
use serde_json::Value;
use sextant_core::mpt::AccountStorageProof;
use sextant_core::statement::HeaderRecord;
use wrap_bridge_v04::{config_account, token, Config, Policy};
use wrap_core::H256;

/// `MAX_GAS_EXEC`, the most execution gas one block (so one transaction) may use.
pub const GAS_CAP: u64 = 10_000_000;

pub const TOKEN: AccountId = AccountId::new(*b"token/v0.4/test/header/000000001");
pub const SA: AccountId = AccountId::new(*b"sextant_attest/v0.4/test/hdr/001");
/// The same program account id as the v0.3 tests: the anvil vault was
/// deployed with the pairing it names, so the vectors' deposits are this
/// program's.
pub const BR: AccountId = AccountId::new(*b"wrap_bridge/test/header/00000001");

pub const LEZ_CHAIN_ID: H256 = *b"lez/test/chain/00000000000000001";
pub const ZONE_ID: H256 = *b"lez/test/zone/000000000000000001";
pub const DEPLOYMENT_ID: H256 = *b"rfp022/test/deployment/000000001";
pub const ANVIL_CHAIN_ID: u64 = 31337;
pub const ETH: [u8; 20] = [0; 20];
pub const SALT: H256 = [0x5a; 32];
pub const SLOT: u64 = 8_192_000;

pub const ALICE_SEED: H256 = *b"alice's wallet seed, test only!!";
pub const BOB_SEED: H256 = *b"bob's wallet seed, test only!!!!";

pub fn root_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn bin(name: &str) -> Vec<u8> {
    let p = root_dir().join("v04/artifacts").join(name);
    std::fs::read(&p).unwrap_or_else(|_| panic!("missing {}: v04/scripts/build-guests.sh", p.display()))
}

pub fn program(name: &str) -> Program {
    Program::new(Cow::Owned(bin(name))).expect("a RISC0 program binary")
}

pub struct Key {
    pub sk: PrivateKey,
    pub pk: PublicKey,
    pub id: AccountId,
}

impl Key {
    /// The same derivation as the v0.3 tests: the vectors' deposits commit to these accounts.
    #[must_use]
    pub fn new(seed: u16) -> Self {
        let mut b = [0x5a; 32];
        b[..2].copy_from_slice(&seed.to_le_bytes());
        let sk = PrivateKey::try_new(b).expect("a valid secp256k1 scalar");
        let pk = PublicKey::new_from_private_key(&sk);
        let id = AccountId::from(&pk);
        Self { sk, pk, id }
    }
}

pub fn alice() -> Key {
    Key::new(100)
}
pub fn bob() -> Key {
    Key::new(200)
}
pub fn admin() -> Key {
    Key::new(300)
}
pub fn freezer() -> Key {
    Key::new(301)
}
pub fn writer() -> Key {
    Key::new(302)
}

/// One transaction's outcome: applied, or reverted with the guest's message.
#[derive(Debug)]
pub struct Outcome {
    pub cycles: u64,
    pub events: Vec<TransactionEvent>,
}

pub struct Chain {
    pub state: V03State,
    pub block: u64,
    pub now: u64,
    pub log: Vec<(String, u64, bool)>,
    next_key: u16,
}

impl Chain {
    #[must_use]
    pub fn new() -> Self {
        let state = V03State::new().with_named_programs([(TOKEN, program("token.bin")), (SA, program("sextant_attest.bin")), (BR, program("wrap_bridge.bin"))]);
        Self { state, block: 10, now: 1_760_000_000_000, log: vec![], next_key: 1000 }
    }

    /// Send `message` to `to`, declaring `actors`, signed by `signers`, with
    /// admission evidence for the fresh PDAs it creates (`pdas`: (program, seed)).
    pub fn send(&mut self, what: &str, to: Actor, actors: &[Actor], signers: &[&Key], pdas: &[(AccountId, lee_core::program::PdaSeed)], message: &impl BorshSerialize) -> Result<Outcome, String> {
        let nonces: BTreeMap<_, _> = signers.iter().map(|k| (k.id, self.state.get_account_by_id(k.id).nonce)).collect();
        // Signers are admitted by their signatures; fresh PDAs need evidence.
        let evidence: Vec<PublicAccountEvidence> = pdas.iter().map(|(program, seed)| PublicAccountEvidence::Pda { program: *program, seed: *seed }).collect();
        let data = Program::serialize_message(message).map_err(|e| e.to_string())?;
        let mut all = actors.to_vec();
        all.push(to);
        let msg = Message::new(to, data, all, nonces, None, evidence);
        let sks: Vec<&PrivateKey> = signers.iter().map(|k| &k.sk).collect();
        let tx = PublicTransaction::new(msg.clone(), WitnessSet::for_message(&msg, &sks));
        self.submit(what, &tx)
    }

    pub fn submit(&mut self, what: &str, tx: &PublicTransaction) -> Result<Outcome, String> {
        let (charge, r) = ValidatedStateDiff::from_public_transaction_metered(tx, &self.state, self.block, self.now, GAS_CAP);
        self.block += 1;
        self.now += 12_000;
        let diff = r.map_err(|e| format!("not includable: {e}"))?;
        let events = self.state.apply_state_diff(diff).map_err(|e| format!("apply: {e}"))?;
        self.log.push((what.to_string(), charge.cycles, charge.revert.is_none()));
        match charge.revert {
            None => Ok(Outcome { cycles: charge.cycles, events }),
            Some(e) => Err(e),
        }
    }

    /// A brand-new key: a relayer with no role.
    pub fn fresh(&mut self) -> Key {
        self.next_key += 1;
        Key::new(self.next_key)
    }

    #[must_use]
    pub fn actor_state(&self, id: AccountId, program: AccountId) -> Vec<u8> {
        self.state.get_account_by_id(id).data.actor_state(program).to_vec()
    }

    #[must_use]
    pub fn token_state(&self, id: AccountId) -> Option<token::TokenState> {
        let s = self.actor_state(id, TOKEN);
        (!s.is_empty()).then(|| borsh::from_slice(&s).expect("a token state"))
    }
}

impl Default for Chain {
    fn default() -> Self {
        Self::new()
    }
}

/// The first typed refusal in a revert message: `E72xx` (sextant_attest) or `E74xx` (wrap_bridge).
pub fn refusal(e: &str) -> Option<u32> {
    let i = e.match_indices('E').map(|(i, _)| i).find(|&i| {
        e.get(i + 1..i + 5).is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()) && (d.starts_with("72") || d.starts_with("74")))
    })?;
    e[i + 1..i + 5].parse().ok()
}

// ---- vectors (vectors/anvil, as for v0.3) ----

pub fn hexv(s: &str) -> Vec<u8> {
    let s = s.trim_start_matches("0x");
    if s.len() % 2 == 1 { hex::decode(format!("0{s}")).unwrap() } else { hex::decode(s).unwrap() }
}

pub fn h<const N: usize>(v: &Value) -> [u8; N] {
    let b = hexv(v.as_str().unwrap_or_else(|| panic!("not a hex string: {v}")));
    let mut out = [0u8; N];
    out[N - b.len()..].copy_from_slice(&b);
    out
}

pub fn q(v: &Value) -> u64 {
    u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

#[derive(Clone, Debug)]
pub struct Deposit {
    pub index: u64,
    pub leaf: H256,
    pub commitment: H256,
    pub token: [u8; 20],
    pub amount: u128,
}

pub struct Anvil {
    pub vault: [u8; 20],
    pub usdt: [u8; 20],
    pub pairing: H256,
    pub deposits: Vec<Deposit>,
    pub block: Value,
    pub proofs: Vec<(H256, AccountStorageProof)>,
    pub unknown: (H256, AccountStorageProof),
}

fn storage_proof(r: &Value) -> AccountStorageProof {
    let nodes = |v: &Value| v.as_array().unwrap().iter().map(|x| hexv(x.as_str().unwrap())).collect::<Vec<_>>();
    AccountStorageProof {
        address: h(&r["address"]),
        account_proof: nodes(&r["accountProof"]),
        slot: h(&r["storageProof"][0]["key"]),
        storage_proof: nodes(&r["storageProof"][0]["proof"]),
    }
}

#[must_use]
pub fn anvil() -> Anvil {
    anvil_at(&root_dir().join("vectors/anvil"))
}

/// The same vectors from any directory laid out like vectors/anvil
/// (scripts/anvil_deposits.py writes one for a live run).
#[must_use]
pub fn anvil_at(dir: &Path) -> Anvil {
    let json = |name: &str| -> Value { serde_json::from_slice(&std::fs::read(dir.join(name)).expect("vectors/anvil")).unwrap() };
    let d = json("deployment.json");
    let deposits = json("deposits.json")
        .as_array()
        .unwrap()
        .iter()
        .map(|x| Deposit { index: x["index"].as_u64().unwrap(), leaf: h(&x["leaf"]), commitment: h(&x["commitment"]), token: h(&x["token"]), amount: x["amount"].as_str().unwrap().parse().unwrap() })
        .collect();
    let proofs = json("proofs.json").as_array().unwrap().iter().map(|x| (h(&x["root"]), storage_proof(&x["proof"]["result"]))).collect();
    let u = json("proof-unknown.json");
    Anvil {
        vault: h(&d["vault"]),
        usdt: h(&d["usdt"]),
        pairing: h(&d["pairing"]),
        deposits,
        block: json("block.json")["result"].clone(),
        proofs,
        unknown: (h(&u["root"]), storage_proof(&u["proof"]["result"])),
    }
}

impl Anvil {
    pub fn leaves(&self, n: usize) -> Vec<H256> {
        self.deposits[..n].iter().map(|d| d.leaf).collect()
    }
    /// The anvil block as Sextant records a finalized header.
    pub fn record(&self, slot: u64) -> HeaderRecord {
        HeaderRecord {
            chain_id: ANVIL_CHAIN_ID,
            slot,
            beacon_root: wrap_core::sha256(&[b"anvil has no beacon chain", &slot.to_le_bytes()]),
            block_number: q(&self.block["number"]),
            state_root: h(&self.block["stateRoot"]),
            receipts_root: h(&self.block["receiptsRoot"]),
        }
    }
}

// ---- the Attest program ----

pub fn instance() -> H256 {
    sextant_attest_v04::writer_instance(&writer().id)
}

pub fn record_header(c: &mut Chain, record: HeaderRecord) -> Result<Outcome, String> {
    let w = writer();
    let inst = sextant_attest_v04::writer_instance(&w.id);
    let slot = record.slot;
    let hdr = sextant_attest_v04::header_account(&SA, &inst, slot);
    c.send("Record", Actor::new(w.id, SA), &[Actor::new(hdr, SA)], &[&w], &[], &sextant_attest_v04::Message::Record { slot, record })
}

// ---- the bridge ----

#[must_use]
pub fn config(vault: [u8; 20]) -> Config {
    Config {
        deployment_id: DEPLOYMENT_ID,
        lez_chain_id: LEZ_CHAIN_ID,
        zone_id: ZONE_ID,
        sextant: SA,
        instance_id: instance(),
        chain_id: ANVIL_CHAIN_ID,
        vault,
        token_program: TOKEN,
        admin: admin().id,
        freezer: freezer().id,
    }
}

pub fn eth_policy() -> Policy {
    Policy { denominations: vec![100_000_000_000_000_000, 1_000_000_000_000_000_000], mint_cap: 10_000_000_000_000_000_000 }
}
pub fn usdt_policy() -> Policy {
    Policy { denominations: vec![100_000_000, 1_000_000_000], mint_cap: 10_000_000_000 }
}

pub fn br(id: AccountId) -> Actor {
    Actor::new(id, BR)
}
pub fn tk(id: AccountId) -> Actor {
    Actor::new(id, TOKEN)
}

pub fn ca(cfg: &Config) -> AccountId {
    config_account(&BR, cfg)
}
pub fn entry(cfg: &Config, t: &[u8; 20]) -> AccountId {
    wrap_bridge_v04::entry_account(&BR, &ca(cfg), t)
}
pub fn definition(cfg: &Config, t: &[u8; 20]) -> AccountId {
    wrap_bridge_v04::definition_account(&BR, &ca(cfg), t, &SALT).0
}
pub fn burns(cfg: &Config) -> AccountId {
    wrap_bridge_v04::burns_account(&BR, &ca(cfg))
}

pub fn init_bridge(c: &mut Chain, cfg: &Config) -> Result<Outcome, String> {
    let (cfg_id, seed) = wrap_bridge_v04::config_pda(&BR, cfg);
    let relayer = c.fresh();
    c.send("Init", br(cfg_id), &[br(burns(cfg))], &[&relayer], &[(BR, seed)], &wrap_bridge_v04::Message::Init { config: cfg.clone() })
}

pub fn register(c: &mut Chain, signer: &Key, cfg: &Config, eth_token: [u8; 20], name: &str, policy: Policy) -> Result<Outcome, String> {
    let actors = [br(ca(cfg)), br(entry(cfg, &eth_token)), tk(definition(cfg, &eth_token)), tk(entry(cfg, &eth_token))];
    let m = wrap_bridge_v04::Message::Register { config: cfg.clone(), eth_token, name: name.into(), salt: SALT, policy };
    c.send("Register", br(signer.id), &actors, &[signer], &[], &m)
}

/// The holder opens its holding of the wrapped `eth_token` (holding -> definition -> holding).
pub fn open_holding(c: &mut Chain, holder: &Key, cfg: &Config, eth_token: &[u8; 20]) -> Result<Outcome, String> {
    let def = definition(cfg, eth_token);
    c.send("InitializeHolding", tk(holder.id), &[tk(def)], &[holder], &[], &token::Message::InitializeHolding { definition: def })
}

pub fn claim_message(a: &Anvil, cfg: &Config, d: &Deposit, n: usize, destination: AccountId, secret: H256, slot: u64) -> wrap_bridge_v04::Claim {
    let (root, proof) = a.proofs[n].clone();
    wrap_bridge_v04::Claim {
        config: cfg.clone(),
        eth_token: d.token,
        salt: SALT,
        amount: d.amount,
        slot,
        root,
        proof,
        index: d.index,
        path: wrap_core::path(&a.leaves(n), d.index as usize),
        destination,
        secret,
    }
}

/// Every actor a claim reaches, and the nullifier PDA it creates.
pub fn claim_actors(cfg: &Config, c: &wrap_bridge_v04::Claim) -> (Actor, Vec<Actor>, (AccountId, lee_core::program::PdaSeed)) {
    claim_actors_with(&BR, cfg, c)
}

/// The same for wrap_bridge deployed at `me` (the token program and Sextant are the config's).
pub fn claim_actors_with(me: &AccountId, cfg: &Config, c: &wrap_bridge_v04::Claim) -> (Actor, Vec<Actor>, (AccountId, lee_core::program::PdaSeed)) {
    let pair = wrap_bridge_v04::pairing(me, cfg);
    let nf = wrap_core::nullifier(&pair, &c.secret, c.index);
    let cfg_id = config_account(me, cfg);
    let (nul, seed) = wrap_bridge_v04::nullifier_pda(me, &cfg_id, &nf);
    let hdr = wrap_bridge_v04::header_account(&cfg.sextant, &cfg.instance_id, c.slot);
    let entry = wrap_bridge_v04::entry_account(me, &cfg_id, &c.eth_token);
    let def = wrap_bridge_v04::definition_account(me, &cfg_id, &c.eth_token, &c.salt).0;
    let tp = cfg.token_program;
    let actors = vec![
        Actor::new(hdr, cfg.sextant),
        Actor::new(cfg_id, *me),
        Actor::new(entry, *me),
        Actor::new(entry, tp),
        Actor::new(def, tp),
        Actor::new(c.destination, tp),
    ];
    (Actor::new(nul, *me), actors, (*me, seed))
}

/// A claim sent by a brand-new relayer account.
pub fn claim(ch: &mut Chain, cfg: &Config, c: &wrap_bridge_v04::Claim) -> Result<Outcome, String> {
    let (to, actors, pda) = claim_actors(cfg, c);
    let relayer = ch.fresh();
    ch.send("Claim", to, &actors, &[&relayer], &[pda], &wrap_bridge_v04::Message::Claim(Box::new(c.clone())))
}

pub fn burn_message(cfg: &Config, eth_token: [u8; 20], amount: u128, eth_commitment: H256, holder: AccountId) -> wrap_bridge_v04::Message {
    wrap_bridge_v04::Message::Burn { eth_token, amount, eth_commitment, salt: SALT, holder, config: cfg.clone() }
}

pub fn burn_actors(cfg: &Config, eth_token: &[u8; 20], holder: AccountId) -> Vec<Actor> {
    vec![br(ca(cfg)), br(entry(cfg, eth_token)), tk(holder), tk(entry(cfg, eth_token)), tk(definition(cfg, eth_token))]
}

/// `holder` burns `amount` of the wrapped `eth_token`, signing as the holder.
pub fn burn(ch: &mut Chain, cfg: &Config, holder: &Key, eth_token: [u8; 20], amount: u128, eth_commitment: H256) -> Result<Outcome, String> {
    let m = burn_message(cfg, eth_token, amount, eth_commitment, holder.id);
    ch.send("Burn", br(burns(cfg)), &burn_actors(cfg, &eth_token, holder.id), &[holder], &[], &m)
}

pub fn balance(ch: &Chain, holder: AccountId) -> u128 {
    match ch.token_state(holder) {
        Some(token::TokenState::Holding(h)) => h.balance,
        _ => 0,
    }
}

pub fn supply(ch: &Chain, cfg: &Config, t: &[u8; 20]) -> u128 {
    match ch.token_state(definition(cfg, t)) {
        Some(token::TokenState::Definition(d)) => d.total_supply,
        other => panic!("not a definition: {other:?}"),
    }
}

pub struct World {
    pub ch: Chain,
    pub a: Anvil,
    pub cfg: Config,
}

/// The anvil header recorded, the bridge configured, ETH and USDT registered.
#[must_use]
pub fn world() -> World {
    let a = anvil();
    let mut ch = Chain::new();
    record_header(&mut ch, a.record(SLOT)).expect("Record the anvil header");
    let cfg = config(a.vault);
    assert_eq!(wrap_bridge_v04::pairing(&BR, &cfg), a.pairing, "the vault was deployed with this deployment's pairing");
    init_bridge(&mut ch, &cfg).expect("Init");
    register(&mut ch, &admin(), &cfg, ETH, "Wrapped Ether (anvil)", eth_policy()).expect("Register ETH");
    register(&mut ch, &admin(), &cfg, a.usdt, "Wrapped USDT (anvil)", usdt_policy()).expect("Register USDT");
    World { ch, a, cfg }
}

pub fn secret(seed: &H256, n: u64) -> H256 {
    wrap_core::derive_secret(seed, b'd', n)
}

/// The ImageID of a committed program binary.
#[must_use]
pub fn image_id(name: &str) -> [u8; 32] {
    risc0_binfmt::compute_image_id(&bin(name)).expect("a valid program binary").as_bytes().try_into().expect("32 bytes")
}

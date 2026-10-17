//! A one-node LEZ v0.3 chain in memory with three programs deployed: Sextant's
//! light client (`sextant_lc`), LEZ v0.3.0's token program, and `wrap_bridge`.
//!
//! Transactions are signed `PublicTransaction`s and go through
//! `ValidatedStateDiff::from_public_transaction_with_cycle_budget`, the call
//! the sequencer's settlement makes, then `V03State::apply_state_diff`: every
//! plan and apply runs in the RISC Zero executor, and the cycle count is the
//! gas the transaction is charged.
//!
//! The Ethereum side is a local anvil chain (`scripts/anvil-vectors.sh`), which
//! has no beacon chain. Its block is wrapped in a beacon header built here, and
//! a Sextant instance is initialised from that header as its trusted
//! checkpoint: the checkpoint is the one input a light client takes on trust,
//! and on a real network it is a finalized beacon block root. Everything after
//! it (the storage proof against the block's state root, the deposit's leaf and
//! path, the nullifier, the mint) is verified exactly as it would be on Sepolia.

use lee::{
    public_transaction::{Message, WitnessSet},
    AccountId, PrivateKey, ProgramShardSelector, PublicKey, PublicTransaction, V03State, ValidatedStateDiff,
};
use serde_json::Value;
use sextant_core::lc::{self, CURRENT_COMMITTEE_DEPTH, CURRENT_COMMITTEE_INDEX, EXECUTION_DEPTH, EXECUTION_INDEX};
use sextant_core::mpt::AccountStorageProof;
use sextant_core::{ssz, BeaconHeader, ChainConfig, ExecutionHeader, LcHeader, SyncCommittee};
use std::path::{Path, PathBuf};
use wrap_bridge::{config_account, Config, Policy};
use wrap_core::H256;

/// The most execution gas one transaction may declare: `MAX_GAS_EXEC`.
pub const GAS_CAP: u64 = 10_000_000;

pub const LC: AccountId = AccountId::new(*b"sextant_lc/test/header/000000001");
pub const TOKEN: AccountId = AccountId::new(*b"token/v0.3.0/test/header/0000001");
pub const ATA: AccountId = AccountId::new(*b"ata/v0.3.0/test/header/000000001");
pub const BR: AccountId = AccountId::new(*b"wrap_bridge/test/header/00000001");

pub const LEZ_CHAIN_ID: H256 = *b"lez/test/chain/00000000000000001";
pub const ZONE_ID: H256 = *b"lez/test/zone/000000000000000001";
pub const DEPLOYMENT_ID: H256 = *b"rfp022/test/deployment/000000001";
/// The devnet Sextant instance the tests initialise from the anvil block.
pub const INSTANCE: H256 = *b"anvil/test/instance/000000000001";
pub const ANVIL_CHAIN_ID: u64 = 31337;
pub const ETH: [u8; 20] = [0; 20];
pub const SALT: H256 = [0x5a; 32];

/// The two depositors' wallet seeds (what a user backs up, and all they back up).
pub const ALICE_SEED: H256 = *b"alice's wallet seed, test only!!";
pub const BOB_SEED: H256 = *b"bob's wallet seed, test only!!!!";

pub fn root_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

pub fn bin(name: &str) -> Vec<u8> {
    let p = root_dir().join("artifacts/programs/v0.3").join(name);
    std::fs::read(&p).unwrap_or_else(|_| panic!("missing {}: scripts/build-guest.sh", p.display()))
}

/// A key with a fixed seed: the same account each run.
pub struct Key {
    pub sk: PrivateKey,
    pub id: AccountId,
}

impl Key {
    #[must_use]
    pub fn new(seed: u16) -> Self {
        let mut b = [0x5a; 32];
        b[..2].copy_from_slice(&seed.to_le_bytes());
        let sk = PrivateKey::try_new(b).expect("a valid secp256k1 scalar");
        let id = AccountId::from(&PublicKey::new_from_private_key(&sk));
        Self { sk, id }
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

pub struct Chain {
    pub state: V03State,
    pub block: u64,
    pub now: u64,
    pub last_cycles: u64,
    next_key: u16,
}

impl Chain {
    #[must_use]
    pub fn new() -> Self {
        let p = |n: &str| lee::program::Program::new(bin(n).into()).expect("a RISC0 program binary");
        let state = V03State::new().with_named_programs([(LC, p("sextant_lc.bin")), (TOKEN, p("token.bin")), (ATA, p("associated_token_account.bin")), (BR, p("wrap_bridge.bin"))]);
        Self { state, block: 10, now: 1_760_000_000_000, last_cycles: 0, next_key: 1000 }
    }

    /// Send `ix` to `program` over `rows`, signed by `signer`.
    pub fn send_as<I: borsh::BorshSerialize>(&mut self, signer: &Key, program: AccountId, ix: &I, rows: Vec<ProgramShardSelector>) -> Result<u64, String> {
        let nonces = vec![self.state.get_account_by_id(signer.id).nonce];
        let message = Message::try_new(program, rows, nonces, ix).map_err(|e| e.to_string())?;
        let tx = PublicTransaction::new(message.clone(), WitnessSet::for_message(&message, &[&signer.sk]));
        let r = ValidatedStateDiff::from_public_transaction_with_cycle_budget(&tx, &self.state, self.block, self.now, GAS_CAP);
        self.block += 1;
        self.now += 12_000;
        match r {
            Ok((diff, charge)) => {
                self.last_cycles = charge.cycles;
                let _ = self.state.apply_state_diff(diff);
                Ok(charge.cycles)
            }
            Err(e) => Err(format!("{e}")),
        }
    }

    /// The same, signed by a brand-new account: a relayer with no role and
    /// no prior relationship to anything.
    pub fn send<I: borsh::BorshSerialize>(&mut self, program: AccountId, ix: &I, rows: Vec<ProgramShardSelector>) -> Result<u64, String> {
        let k = Key::new(self.next_key);
        self.next_key += 1;
        self.send_as(&k, program, ix, rows)
    }

    /// The data `program` holds in account `id`.
    #[must_use]
    pub fn shard_of(&self, id: AccountId, program: AccountId) -> Vec<u8> {
        self.state.get_account_by_id(id).data.shard(program).to_vec()
    }
}

impl Default for Chain {
    fn default() -> Self {
        Self::new()
    }
}

/// The first typed refusal in an error: `E72xx` (sextant_lc) or `E74xx` (wrap_bridge).
pub fn refusal(e: &str) -> Option<u32> {
    let i = e.match_indices('E').map(|(i, _)| i).find(|&i| {
        e.get(i + 1..i + 5).is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()) && (d.starts_with("72") || d.starts_with("74")))
    })?;
    e[i + 1..i + 5].parse().ok()
}

// ---- vectors ----

pub fn json(rel: &str) -> Value {
    let p = root_dir().join(rel);
    serde_json::from_slice(&std::fs::read(&p).unwrap_or_else(|_| panic!("read {}: scripts/anvil-vectors.sh", p.display()))).unwrap()
}

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

/// One `Deposit` event of the vault, as the anvil run recorded it.
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
    /// `knownRoot[r]` at the last block, for each root `r` the tree had (by deposit count).
    pub proofs: Vec<(H256, AccountStorageProof)>,
    /// The same slot for a root the vault never had.
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

fn json_at(p: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(p).unwrap_or_else(|_| panic!("read {}", p.display()))).unwrap()
}

/// The same vectors from any directory laid out like `vectors/anvil` (the
/// client reads a live anvil run's this way).
#[must_use]
pub fn anvil_at(dir: &Path) -> Anvil {
    let json = |name: &str| json_at(&dir.join(name));
    let d = json("deployment.json");
    let deposits = json("deposits.json")
        .as_array()
        .unwrap()
        .iter()
        .map(|x| Deposit {
            index: x["index"].as_u64().unwrap(),
            leaf: h(&x["leaf"]),
            commitment: h(&x["commitment"]),
            token: h(&x["token"]),
            amount: x["amount"].as_str().unwrap().parse().unwrap(),
        })
        .collect();
    let proofs = json("proofs.json")
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (h(&x["root"]), storage_proof(&x["proof"]["result"])))
        .collect();
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
}

// ---- the devnet light-client instance ----

fn fold(leaf: &H256, branch: &[H256], index: u64) -> H256 {
    let mut v = *leaf;
    for (i, sib) in branch.iter().enumerate() {
        v = if (index >> i) & 1 == 1 { ssz::hash2(sib, &v) } else { ssz::hash2(&v, sib) };
    }
    v
}

/// The anvil block's execution header, as a beacon block would carry it.
pub fn execution_header(b: &Value) -> ExecutionHeader {
    let mut base_fee = [0u8; 32];
    base_fee[..16].copy_from_slice(&u128::from(q(&b["baseFeePerGas"])).to_le_bytes());
    ExecutionHeader {
        parent_hash: h(&b["parentHash"]),
        fee_recipient: h(&b["miner"]),
        state_root: h(&b["stateRoot"]),
        receipts_root: h(&b["receiptsRoot"]),
        logs_bloom: hexv(b["logsBloom"].as_str().unwrap()),
        prev_randao: h(&b["mixHash"]),
        block_number: q(&b["number"]),
        gas_limit: q(&b["gasLimit"]),
        gas_used: q(&b["gasUsed"]),
        timestamp: q(&b["timestamp"]),
        extra_data: hexv(b["extraData"].as_str().unwrap()),
        base_fee_per_gas: base_fee,
        block_hash: h(&b["hash"]),
        transactions_root: h(&b["transactionsRoot"]),
        withdrawals_root: b.get("withdrawalsRoot").map(h).unwrap_or_default(),
        blob_gas_used: b.get("blobGasUsed").map(q).unwrap_or_default(),
        excess_blob_gas: b.get("excessBlobGas").map(q).unwrap_or_default(),
    }
}

pub struct Devnet {
    pub checkpoint: H256,
    pub cfg: ChainConfig,
    pub header: LcHeader,
    pub committee: SyncCommittee,
    pub branch: Vec<H256>,
    pub slot: u64,
}

/// A beacon header at `slot` whose body commits to the anvil block's
/// execution header, and whose state commits to a committee of 512 keys.
#[must_use]
pub fn devnet(block: &Value, slot: u64) -> Devnet {
    let execution = execution_header(block);
    let filler = |tag: u8, n: usize| (0..n).map(|i| wrap_core::sha256(&[&[tag, i as u8]])).collect::<Vec<H256>>();
    let execution_branch = filler(1, EXECUTION_DEPTH);
    let body_root = fold(&ssz::execution_header_root(&execution), &execution_branch, EXECUTION_INDEX);
    let committee = SyncCommittee {
        pubkeys: (0..512u16).map(|i| {
            let mut k = [0u8; 48];
            k[..2].copy_from_slice(&i.to_le_bytes());
            k
        }).collect(),
        aggregate: [0xaa; 48],
    };
    let branch = filler(2, CURRENT_COMMITTEE_DEPTH);
    let state_root = fold(&ssz::sync_committee_root(&committee), &branch, CURRENT_COMMITTEE_INDEX);
    let beacon = BeaconHeader { slot, proposer_index: 0, parent_root: [0; 32], state_root, body_root };
    let header = LcHeader { beacon, execution, execution_branch };
    let checkpoint = ssz::beacon_header_root(&header.beacon);
    let cfg = ChainConfig { chain_id: ANVIL_CHAIN_ID, genesis_validators_root: [0; 32], forks: vec![(0, [0; 4])], min_participants: 342 };
    assert!(lc::bootstrap(&checkpoint, &header, &committee, &branch).is_ok());
    Devnet { checkpoint, cfg, header, committee, branch, slot }
}

/// `Init` the devnet instance from the anvil block.
pub fn init_light_client(c: &mut Chain, d: &Devnet) -> Result<u64, String> {
    let ix = sextant_program::Instruction::Init {
        instance_id: INSTANCE,
        checkpoint: d.checkpoint,
        cfg: d.cfg.clone(),
        header: d.header.clone(),
        committee: d.committee.clone(),
        branch: d.branch.clone(),
    };
    let rows = vec![
        ProgramShardSelector::new(sextant_program::instance_account(&LC, &INSTANCE), LC),
        ProgramShardSelector::new(sextant_program::header_account(&LC, &INSTANCE, d.slot), LC),
    ];
    c.send(LC, &ix, rows)
}

// ---- the bridge ----

#[must_use]
pub fn config(vault: [u8; 20], deployment_id: H256) -> Config {
    Config {
        deployment_id,
        lez_chain_id: LEZ_CHAIN_ID,
        zone_id: ZONE_ID,
        sextant: LC,
        instance_id: INSTANCE,
        chain_id: ANVIL_CHAIN_ID,
        vault,
        token_program: TOKEN,
        ata_program: ATA,
        admin: admin().id,
        freezer: freezer().id,
    }
}

/// The permitted amounts the anvil run registered in the vault.
pub fn eth_policy() -> Policy {
    Policy { denominations: vec![100_000_000_000_000_000, 1_000_000_000_000_000_000], mint_cap: 10_000_000_000_000_000_000 }
}
pub fn usdt_policy() -> Policy {
    Policy { denominations: vec![100_000_000, 1_000_000_000], mint_cap: 10_000_000_000 }
}

pub fn sel(id: AccountId, program: AccountId) -> ProgramShardSelector {
    ProgramShardSelector::new(id, program)
}

pub fn init_bridge(c: &mut Chain, cfg: &Config) -> Result<u64, String> {
    c.send(BR, &wrap_bridge::Instruction::Init { config: cfg.clone() }, vec![sel(config_account(&BR, cfg), BR)])
}

pub fn register(c: &mut Chain, signer: &Key, cfg: &Config, eth_token: [u8; 20], name: &str, salt: H256, policy: Policy) -> Result<u64, String> {
    let ca = config_account(&BR, cfg);
    let ix = wrap_bridge::Instruction::Register { config: cfg.clone(), eth_token, name: name.into(), salt, policy };
    let rows = vec![
        sel(ca, BR),
        sel(wrap_bridge::entry_account(&BR, &ca, &eth_token), BR),
        sel(signer.id, BR),
        sel(wrap_bridge::definition_account(&BR, &ca, &eth_token, &salt).0, TOKEN),
        sel(wrap_bridge::supply_account(&BR, &ca, &eth_token, &salt), TOKEN),
    ];
    c.send_as(signer, BR, &ix, rows)
}

/// The claim a client builds for deposit `d`, opened with `(destination, secret)`,
/// against the vault root after `n` deposits, under the devnet header at `slot`.
pub fn claim_ix(a: &Anvil, cfg: &Config, d: &Deposit, n: usize, destination: AccountId, secret: H256, slot: u64) -> wrap_bridge::Claim {
    let (root, proof) = a.proofs[n].clone();
    wrap_bridge::Claim {
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

pub fn claim_rows(cfg: &Config, c: &wrap_bridge::Claim) -> Vec<ProgramShardSelector> {
    let ca = config_account(&BR, cfg);
    let pair = wrap_bridge::pairing(&BR, cfg);
    let nf = wrap_core::nullifier(&pair, &c.secret, c.index);
    vec![
        sel(sextant_program::header_account(&LC, &cfg.instance_id, c.slot), LC),
        sel(ca, BR),
        sel(wrap_bridge::entry_account(&BR, &ca, &c.eth_token), BR),
        sel(wrap_bridge::nullifier_account(&BR, &ca, &nf), BR),
        sel(wrap_bridge::definition_account(&BR, &ca, &c.eth_token, &c.salt).0, TOKEN),
        sel(ata(cfg, &c.eth_token, c.destination), TOKEN),
    ]
}

pub fn claim(ch: &mut Chain, cfg: &Config, c: &wrap_bridge::Claim) -> Result<u64, String> {
    let rows = claim_rows(cfg, c);
    ch.send(BR, &wrap_bridge::Instruction::Claim(Box::new(c.clone())), rows)
}

/// `owner`'s associated token account for the wrapped `eth_token` of `cfg`.
pub fn ata(cfg: &Config, eth_token: &[u8; 20], owner: AccountId) -> AccountId {
    let ca = config_account(&BR, cfg);
    let def = wrap_bridge::definition_account(&BR, &ca, eth_token, &SALT).0;
    wrap_bridge::token::ata_account(&ATA, &owner, &def, &TOKEN)
}

/// `owner`'s balance of the wrapped `eth_token`.
pub fn balance(ch: &Chain, cfg: &Config, eth_token: &[u8; 20], owner: AccountId) -> u128 {
    let d = ch.shard_of(ata(cfg, eth_token, owner), TOKEN);
    if d.is_empty() {
        return 0;
    }
    match borsh::from_slice::<wrap_bridge::token::TokenHolding>(&d).expect("a token holding") {
        wrap_bridge::token::TokenHolding::Fungible { balance, .. } => balance,
        _ => panic!("not fungible"),
    }
}

pub fn supply(ch: &Chain, cfg: &Config, eth_token: &[u8; 20]) -> u128 {
    let ca = config_account(&BR, cfg);
    let d = ch.shard_of(wrap_bridge::definition_account(&BR, &ca, eth_token, &SALT).0, TOKEN);
    match borsh::from_slice::<wrap_bridge::token::TokenDefinition>(&d).expect("a token definition") {
        wrap_bridge::token::TokenDefinition::Fungible { total_supply, .. } => total_supply,
        _ => panic!("not fungible"),
    }
}

/// A world with the light client initialised at the anvil block, the bridge
/// configured, and the two tokens the vault registered (ETH and USDT) registered.
pub struct World {
    pub ch: Chain,
    pub a: Anvil,
    pub cfg: Config,
    pub slot: u64,
}

pub const SLOT: u64 = 8_192_000;

#[must_use]
pub fn world() -> World {
    let a = anvil();
    let mut ch = Chain::new();
    let d = devnet(&a.block, SLOT);
    init_light_client(&mut ch, &d).expect("Init of the devnet light-client instance");
    let cfg = config(a.vault, DEPLOYMENT_ID);
    assert_eq!(wrap_bridge::pairing(&BR, &cfg), a.pairing, "the vault was deployed with this deployment's pairing");
    init_bridge(&mut ch, &cfg).expect("Init of the bridge configuration");
    register(&mut ch, &admin(), &cfg, ETH, "Wrapped Ether (anvil)", SALT, eth_policy()).expect("Register ETH");
    register(&mut ch, &admin(), &cfg, a.usdt, "Wrapped USDT (anvil)", SALT, usdt_policy()).expect("Register USDT");
    World { ch, a, cfg, slot: SLOT }
}

// ---- the privacy-preserving claim ----

/// Carol's wallet seed: her deposit commits to a private LEZ account.
pub const CAROL_SEED: H256 = *b"carol's wallet seed, test only!!";

/// The keys of a private (shielded) LEZ account: the authorization secret
/// and the two halves of the ML-KEM-768 viewing key seed, all derived from a
/// wallet seed, as a wallet would.
pub struct Shielded {
    pub ask: lee_core::AuthorizationSecretKey,
    pub d: H256,
    pub z: H256,
}

impl Shielded {
    #[must_use]
    pub fn from_seed(seed: &H256, n: u64) -> Self {
        let k = |tag: &[u8]| wrap_core::sha256(&[b"shielded", tag, seed, &n.to_le_bytes()]);
        Self { ask: lee_core::AuthorizationSecretKey(k(b"ask")), d: k(b"d"), z: k(b"z") }
    }
    #[must_use]
    pub fn npk(&self) -> lee_core::NullifierPublicKey {
        lee_core::NullifierPublicKey::from(&lee_core::NullifierSecretKey::from(&self.ask))
    }
    #[must_use]
    pub fn vpk(&self) -> lee_core::encryption::ViewingPublicKey {
        lee_core::encryption::ViewingPublicKey::from_seed(&self.d, &self.z)
    }
    /// The account id: nothing public names it until a transaction creates it.
    #[must_use]
    pub fn id(&self) -> AccountId {
        AccountId::for_regular_private_account(&self.npk(), &self.vpk(), lee_core::Identifier::ZERO)
    }
    /// The witness that creates this account inside the privacy circuit.
    #[must_use]
    pub fn init_witness(&self, random_seed: H256) -> lee_core::PrivateWitness {
        lee_core::PrivateWitness {
            vpk: self.vpk(),
            random_seed,
            identifier: lee_core::Identifier::ZERO,
            kind: lee_core::WitnessKind::Regular { ask: Some(self.ask) },
            nullifier: lee_core::NullifierWitness::Init { npk: self.npk(), commitment_root: lee_core::DUMMY_COMMITMENT_HASH },
        }
    }
}

/// The private account carol's deposit committed to.
#[must_use]
pub fn carol_private() -> Shielded {
    Shielded::from_seed(&CAROL_SEED, 0)
}

/// The rows of a claim minting to the committed private account itself.
pub fn private_claim_rows(cfg: &Config, c: &wrap_bridge::Claim) -> Vec<ProgramShardSelector> {
    let mut rows = claim_rows(cfg, c);
    rows[5] = sel(c.destination, TOKEN);
    rows
}

/// Prove `c` as a privacy-preserving transaction minting to the private
/// account `to`: wrap_bridge's plan, Sextant's and the token program's plans
/// and the token program's apply to the private account are each proven, and
/// composed by LEZ's privacy circuit. The effects on public accounts are
/// deferred: they travel in the transaction and the sequencer applies them.
/// No account signs: the claimant needs no public account at all.
pub fn prove_private_claim(cfg: &Config, c: &wrap_bridge::Claim, to: &Shielded, random_seed: H256) -> Result<lee::PrivacyPreservingTransaction, String> {
    use lee::privacy_preserving_transaction::{circuit::ProgramWithDependencies, Message, WitnessSet};
    let p = |n: &str| lee::program::Program::new(bin(n).into()).expect("a RISC0 program binary");
    let deps = std::collections::HashMap::from([(LC, p("sextant_lc.bin")), (TOKEN, p("token.bin"))]);
    let pwd = ProgramWithDependencies::new(p("wrap_bridge.bin"), BR, deps);
    let input = lee::ProvingInput {
        shard_selectors: private_claim_rows(cfg, c),
        private_witnesses: vec![to.init_witness(random_seed)],
        instruction_data: lee::program::Program::serialize_instruction(wrap_bridge::Instruction::Claim(Box::new(c.clone()))).map_err(|e| e.to_string())?,
        ..Default::default()
    };
    let (output, proof) = lee::execute_and_prove(input, &pwd).map_err(|e| format!("{e}"))?;
    let message = Message::from_circuit_output(vec![], output);
    let witness_set = WitnessSet::for_message(&message, proof, &[]);
    Ok(lee::PrivacyPreservingTransaction::new(message, witness_set))
}

/// Settle a privacy-preserving transaction the way the sequencer does: every
/// check of `ValidatedStateDiff::from_privacy_preserving_transaction` (the
/// proof against the circuit's ImageID and the program images, commitments
/// new, nullifiers unspent), then the deferred public effects, applied by each
/// program in the executor.
pub fn settle_private(ch: &mut Chain, tx: &lee::PrivacyPreservingTransaction) -> Result<(), String> {
    let r = ch.state.transition_from_privacy_preserving_transaction(tx, ch.block, ch.now).map_err(|e| format!("{e}"));
    ch.block += 1;
    ch.now += 12_000;
    r
}

/// What `keys` reads from a transaction's notes: the private account it now
/// holds, if one of the notes is addressed to it and opens to the commitment
/// the chain recorded.
#[must_use]
pub fn read_note(ch: &Chain, tx: &lee::PrivacyPreservingTransaction, keys: &Shielded) -> Option<lee_core::account::Account> {
    tx.message().private_actions.iter().find_map(|a| {
        let ss = lee_core::SharedSecretKey::decapsulate(&a.encrypted_post_state.epk, &keys.d, &keys.z)?;
        let (_, account) = lee_core::encryption::EncryptionScheme::decrypt(&a.encrypted_post_state.ciphertext, &ss, &a.nullifier)?;
        let cm = lee_core::Commitment::new(&keys.id(), &account);
        (cm == a.commitment && ch.state.get_proof_for_commitment(&cm).is_some()).then_some(account)
    })
}

/// The wrapped-token balance in a private account's token shard.
#[must_use]
pub fn private_balance(account: &lee_core::account::Account) -> u128 {
    let d = account.data.shard(TOKEN).to_vec();
    match borsh::from_slice::<wrap_bridge::token::TokenHolding>(&d).expect("a token holding") {
        wrap_bridge::token::TokenHolding::Fungible { balance, .. } => balance,
        _ => panic!("not fungible"),
    }
}

/// The ImageID of a committed program binary.
#[must_use]
pub fn image_id(name: &str) -> [u8; 32] {
    risc0_binfmt::compute_image_id(&bin(name)).expect("a valid program binary").as_bytes().try_into().expect("32 bytes")
}

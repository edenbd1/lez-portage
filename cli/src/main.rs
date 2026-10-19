//! `wrap`: the RFP-022 prototype on a LEZ v0.3 sequencer.
//!
//! It loads the four committed binaries through program_loader, initialises a
//! Sextant instance from a beacon header built around an anvil block (anvil has
//! no beacon chain: that header is the trusted checkpoint, as in the executor
//! tests), initialises and registers the bridge, and sends claims and burns
//! against deposits made into WrapVault on that anvil chain.
//!
//! A run directory (`--run`, `WRAP_RUN`) holds what the steps share:
//! `programs.json` (the four header accounts), `accounts.json` (the wallet
//! accounts of the run: admin, freezer, alice, bob, and carol's private
//! account) and `anvil/` (the vault's deposits, block and storage proofs, in
//! the layout of `vectors/anvil`).
//!
//! Every public transaction is first executed locally, through `lee::V03State`
//! with the committed binaries at their header addresses and the accounts the
//! instruction touches as the chain has them now (the **pre-check**). One the
//! programs accept is sent with a gas limit of its measured gas plus 1% and
//! 10,000; one they refuse is reported with its typed code (`E72xx` Sextant,
//! `E74xx` the bridge) and, with `--force`, sent anyway with the gas limit of
//! the honest instruction of its kind, so that the chain's own verdict is on
//! record. On v0.3 a refused public transaction is still included: it changes
//! nothing and its payer is charged the whole gas limit. The verdict is read
//! back from the chain both ways: the accounts the instruction touches, and the
//! gas the block's fee transaction records.
//!
//! The private claim is a privacy-preserving transaction built and proven here
//! by the wallet crate (`send_privacy_preserving_tx`): no account signs it, no
//! fee is paid, and its verdict is the nullifier account it writes.

use std::{collections::HashMap, path::PathBuf, time::Duration};

use anyhow::{anyhow, ensure, Context as _, Result};
use clap::{Parser, Subcommand, ValueEnum};
use common::{transaction::LeeTransaction, HashType};
use lee::{
    program::Program,
    public_transaction::{Message, WitnessSet},
    AccountId, FeeDeclaration, PrivateKey, ProgramShardSelector, PublicKey, PublicTransaction, V03State,
    ValidatedStateDiff,
};
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use wallet::{program_facades::program_loader::ProgramLoader, AccountIdentity, AccountMention, WalletCore};
use wrap_bridge::{burns_account, config_account, definition_account, entry_account, nullifier_account, supply_account, Config};
use wrap_core::H256;
use wrap_executor_tests as et;

mod bedrock;
mod chain;
mod zk;

const LOCAL_RPC: &str = "http://127.0.0.1:3051";
/// `MAX_GAS_EXEC`: the most execution gas one transaction may declare.
const GAS_CAP: u64 = 10_000_000;
/// The fee program (`fee_core::fee_account_id()`): every block ends with its `Distribute`.
const FEE_PROGRAM: &str = "2TsGdKDWTY92X4vSBZzjzemZpThhyqNEdZJj3xBrQ9AZ";
/// The gas limit a refused instruction is sent with: the honest instruction of
/// its kind as measured (README, docs/raw/), plus about 10%.
const HONEST_CLAIM: u64 = 330_000;
const HONEST_BURN: u64 = 255_000;
const HONEST_REGISTER: u64 = 100_000;
const HONEST_INIT_SEXTANT: u64 = 2_100_000;
/// This deployment's id: a local run, distinct from the executor tests'.
const DEPLOYMENT_ID: H256 = *b"rfp022/local/deployment/00000001";
/// The Sextant instance initialised from the anvil block.
const INSTANCE: H256 = *b"anvil/local/instance/00000000001";
/// The devnet beacon slot the anvil block is wrapped in.
const SLOT: u64 = 8_192_000;
/// The wrapped tokens' definition salt.
const SALT: H256 = [0x5a; 32];

/// The deposits a run makes: (depositor, seed counter, token, amount, destination).
/// Carol's commits to her private account; the others to public accounts.
const PLAN: [(&str, u64, &str, u128); 4] = [
    ("alice", 0, "eth", 1_000_000_000_000_000_000),
    ("alice", 1, "usdt", 100_000_000),
    ("bob", 0, "eth", 100_000_000_000_000_000),
    ("carol", 0, "eth", 1_000_000_000_000_000_000),
];
/// The seed counter of a deposit alice never made.
const PHANTOM_N: u64 = 9;

#[derive(Parser)]
#[command(name = "wrap", version, about = "the RFP-022 bridge prototype on a LEZ v0.3 sequencer")]
struct Cli {
    /// The sequencer.
    #[arg(long, env = "WRAP_RPC", global = true, default_value = LOCAL_RPC)]
    rpc: String,
    /// The run directory (programs.json, accounts.json, anvil/).
    #[arg(long, env = "WRAP_RUN", global = true, default_value = "run")]
    run: PathBuf,
    /// The committed program binaries.
    #[arg(long, env = "WRAP_ARTIFACTS", global = true, default_value = "artifacts/programs/v0.3")]
    artifacts: PathBuf,
    /// The account that pays every public fee (its key must be in the wallet).
    #[arg(long, env = "WRAP_PAYER", global = true)]
    payer: Option<AccountId>,
    /// Send even if the pre-check says the programs refuse.
    #[arg(long, global = true)]
    force: bool,
    /// Seconds to wait for each attempt to be included.
    #[arg(long, env = "WRAP_WAIT", global = true, default_value_t = 120)]
    wait: u64,
    /// A label for the JSON line.
    #[arg(long, global = true, default_value = "")]
    label: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq, Debug)]
enum Token {
    Eth,
    Usdt,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq, Debug)]
enum Who {
    Alice,
    Bob,
    Carol,
}

#[derive(Subcommand)]
enum Cmd {
    /// The ImageID, size and sha256 of each committed binary.
    ImageId,
    /// Load `<name>.bin` from the artifacts through program_loader.
    Deploy {
        name: String,
        /// Make the header immutable.
        #[arg(long)]
        immutable: bool,
    },
    /// Create the run's wallet accounts (admin, freezer, alice, bob public;
    /// carol private) and write accounts.json.
    Setup,
    /// The pairing the vault must be deployed with, and each deposit's commitment.
    Plan,
    /// The run's account ids, for the final-state lines.
    Ids,
    /// `Init` the Sextant instance from a beacon header built around the anvil block.
    InitSextant,
    /// `Init` the bridge configuration.
    InitBridge,
    /// `Register` a wrapped token (signed by the admin).
    Register {
        #[arg(value_enum)]
        token: Token,
    },
    /// Claim deposit `index` (public by default).
    Claim {
        index: usize,
        /// The root of the vault's tree after this many deposits (default: all).
        #[arg(long)]
        root_after: Option<usize>,
        /// Open the deposit with another destination than the one it committed to.
        #[arg(long, value_enum)]
        redirect_to: Option<Who>,
        /// Claim the deposit alice never made (its root is not the vault's).
        #[arg(long)]
        phantom: bool,
        /// A privacy-preserving claim into the private account the deposit committed to.
        #[arg(long)]
        private: bool,
    },
    /// Burn `amount` of the wrapped token from `owner`'s associated token
    /// account, or with `--private` (carol) from her private account, as a
    /// privacy-preserving `BurnFrom`. The Ethereum recipient is the owner's
    /// anvil address, hidden with `--fee` in the burn's commitment.
    Burn {
        #[arg(value_enum)]
        owner: Who,
        #[arg(value_enum)]
        token: Token,
        amount: u128,
        /// The fee a relayer may take from the release, fixed now.
        #[arg(long, default_value_t = 0)]
        fee: u128,
        /// A privacy-preserving BurnFrom from carol's private account.
        #[arg(long)]
        private: bool,
    },
    /// The opening a release needs for `owner`'s burn (run/burns.json): its
    /// leaf, index and path in the burn tree, which must lead to the root the
    /// chain's burn tree holds now, and its nullifier.
    ///
    /// With `--zk`, also the zero-knowledge release proof: the guest
    /// `zk/burn_membership` proves the opening in RISC Zero (the vault's
    /// address and chain, the pairing and `--relayer` are its public context),
    /// and its receipt is compressed to Groth16 for `ZkBurnVerifier`. The
    /// opening then stays here: the release carries the journal and the seal.
    ReleaseProof {
        #[arg(value_enum)]
        owner: Who,
        #[arg(long)]
        zk: bool,
        /// The address that will submit the release (and earn its fee): the journal binds it.
        #[arg(long, required_if_eq("zk", "true"))]
        relayer: Option<String>,
        /// The committed guest.
        #[arg(long, env = "WRAP_ZK_GUEST", default_value = "artifacts/zk/burn_membership.bin")]
        guest: PathBuf,
        /// With `--zk`: prove against the tree a chain-prove record rebuilt from
        /// LEZ blocks (its receipt resolved inside), with `zk/burn_release`.
        #[arg(long, requires = "zk")]
        chain: Option<PathBuf>,
        #[arg(long, env = "WRAP_RELEASE_GUEST", default_value = "artifacts/zk/burn_release.bin")]
        release_guest: PathBuf,
        /// With `--chain`: prove against an anchor-prove record of that chain
        /// (its receipt resolved inside), with `zk/bedrock_release`.
        #[arg(long, requires = "chain")]
        bedrock: Option<PathBuf>,
        #[arg(long, env = "WRAP_BEDROCK_RELEASE_GUEST", default_value = "artifacts/zk/bedrock_release.bin")]
        bedrock_release_guest: PathBuf,
        /// The vault the release is for, if not the run's (another vault of the same pairing).
        #[arg(long, requires = "zk")]
        vault: Option<String>,
    },
    /// Prove a `ZkReleaseInput` (JSON: chain_id, vault, pairing, relayer, token,
    /// amount, recipient, fee, secret, index, path) with the zero-knowledge
    /// release guest, to Groth16; print the journal, the seal and the timings.
    /// Needs no sequencer: the Foundry fixture is made with it.
    ZkProve {
        input: PathBuf,
        #[arg(long, env = "WRAP_ZK_GUEST", default_value = "artifacts/zk/burn_membership.bin")]
        guest: PathBuf,
        /// Prove against a chain-prove record's tree instead, with `zk/burn_release`.
        #[arg(long, conflicts_with = "bedrock")]
        chain: Option<PathBuf>,
        #[arg(long, env = "WRAP_RELEASE_GUEST", default_value = "artifacts/zk/burn_release.bin")]
        release_guest: PathBuf,
        /// Prove against an anchor-prove record's tree instead, with `zk/bedrock_release`.
        #[arg(long)]
        bedrock: Option<PathBuf>,
        #[arg(long, env = "WRAP_BEDROCK_RELEASE_GUEST", default_value = "artifacts/zk/bedrock_release.bin")]
        bedrock_release_guest: PathBuf,
    },
    /// Prove the Bedrock anchor guest (`zk/bedrock_anchor`): the tip of a
    /// chain-prove record (its receipt verified inside) is the LEZ block a
    /// Bedrock block inscribes, with the headers above it (`--block`, a
    /// `bedrock-tools anchor` record). Writes the record to `--out` and the
    /// succinct receipt next to it. Needs no sequencer.
    AnchorProve {
        #[arg(long)]
        chain: PathBuf,
        #[arg(long)]
        block: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, env = "WRAP_ANCHOR_GUEST", default_value = "artifacts/zk/bedrock_anchor.bin")]
        guest: PathBuf,
    },
    /// Prove the LEZ chain guest (`zk/lez_chain`) over the sequencer's blocks:
    /// from the block after `--anchor` (the vault's pinned starting block), or
    /// after the tip of `--prior` (an earlier chain-prove record, whose
    /// receipt is verified inside), to `--to` (default: the latest block).
    /// Writes the record to `--out` and the succinct receipt next to it.
    ChainProve {
        #[arg(long, conflicts_with = "prior")]
        anchor: Option<u64>,
        #[arg(long)]
        prior: Option<PathBuf>,
        #[arg(long)]
        to: Option<u64>,
        #[arg(long)]
        out: PathBuf,
        /// Also write each block's bytes to this directory (`<id>.borsh`).
        #[arg(long)]
        save_blocks: Option<PathBuf>,
        /// Read the blocks from this directory (`<id>.borsh`) instead of the sequencer.
        #[arg(long, conflicts_with = "save_blocks", requires = "to")]
        blocks_dir: Option<PathBuf>,
        /// The anchor's hash, when the blocks are not read from the sequencer.
        #[arg(long, requires = "anchor")]
        anchor_hash: Option<String>,
        #[arg(long, env = "WRAP_CHAIN_GUEST", default_value = "artifacts/zk/lez_chain.bin")]
        guest: PathBuf,
    },
    /// The deployment's chain parameters (the bridge program, its burn-tree
    /// account) and a block's id and hash (default: the latest), the anchor a
    /// ZkChainBurnVerifier is deployed with.
    ChainParams {
        #[arg(long)]
        at: Option<u64>,
    },
    /// Make up a LEZ block after an anchor: one privacy-preserving transaction
    /// whose only effect is the bridge's `Append(leaf)` on its burn tree. A
    /// well-formed block that no sequencer produced: the chain guest proves a
    /// tree over it as over any other, which is what the finality attestation
    /// is there to refuse. Writes `<anchor+1>.borsh` to `--out-dir`.
    ChainForge {
        #[arg(long)]
        anchor: u64,
        #[arg(long)]
        anchor_hash: String,
        #[arg(long)]
        leaf: String,
        #[arg(long)]
        out_dir: PathBuf,
    },
    /// Check that LEZ's `Block` and the guest's mirror decode, re-encode and
    /// hash saved blocks (`<id>.borsh`) identically.
    ChainLayout { dir: PathBuf },
    /// Sync the wallet and decrypt carol's private account.
    ShowPrivate,
    /// A public owner's wrapped balance and a token's supply, as the chain holds them.
    Balance {
        #[arg(value_enum)]
        owner: Who,
        #[arg(value_enum)]
        token: Token,
    },
}

fn parse_id(s: &str) -> Result<AccountId> {
    s.parse().map_err(|e| anyhow!("not an account id {s}: {e:?}"))
}

fn image_hex(bin: &[u8]) -> Result<String> {
    Ok(hex::encode(risc0_binfmt::compute_image_id(bin).map_err(|e| anyhow!("{e}"))?.as_bytes()))
}

fn read_json(p: &PathBuf) -> Result<Value> {
    serde_json::from_slice(&std::fs::read(p).with_context(|| format!("reading {}", p.display()))?).with_context(|| format!("parsing {}", p.display()))
}

fn seed_of(w: Who) -> H256 {
    match w {
        Who::Alice => et::ALICE_SEED,
        Who::Bob => et::BOB_SEED,
        Who::Carol => et::CAROL_SEED,
    }
}

fn who_of(s: &str) -> Who {
    match s {
        "alice" => Who::Alice,
        "bob" => Who::Bob,
        _ => Who::Carol,
    }
}

struct Programs {
    lc: AccountId,
    token: AccountId,
    ata: AccountId,
    br: AccountId,
}

struct Accounts {
    admin: AccountId,
    freezer: AccountId,
    alice: AccountId,
    bob: AccountId,
    carol: AccountId,
}

impl Accounts {
    fn of(&self, w: Who) -> AccountId {
        match w {
            Who::Alice => self.alice,
            Who::Bob => self.bob,
            Who::Carol => self.carol,
        }
    }
}

struct Ctx {
    rpc: SequencerClient,
    run: PathBuf,
    artifacts: PathBuf,
    payer: Option<AccountId>,
    force: bool,
    wait: u64,
    label: String,
}

/// What the pre-check found: the gas the programs use, or their refusal.
enum Pre {
    Gas(u64),
    Refused(String),
}

impl Ctx {
    fn bin(&self, name: &str) -> Result<Vec<u8>> {
        let p = self.artifacts.join(format!("{name}.bin"));
        std::fs::read(&p).with_context(|| format!("reading {}", p.display()))
    }
    fn programs(&self) -> Result<Programs> {
        let j = read_json(&self.run.join("programs.json"))?;
        let g = |k: &str| j[k].as_str().ok_or_else(|| anyhow!("programs.json has no {k}")).and_then(parse_id);
        Ok(Programs { lc: g("sextant_lc")?, token: g("token")?, ata: g("associated_token_account")?, br: g("wrap_bridge")? })
    }
    fn accounts(&self) -> Result<Accounts> {
        let j = read_json(&self.run.join("accounts.json"))?;
        let g = |k: &str| j[k].as_str().ok_or_else(|| anyhow!("accounts.json has no {k}")).and_then(parse_id);
        Ok(Accounts { admin: g("admin")?, freezer: g("freezer")?, alice: g("alice")?, bob: g("bob")?, carol: g("carol_private")? })
    }
    fn anvil(&self) -> et::Anvil {
        et::anvil_at(&self.run.join("anvil"))
    }
    fn payer(&self) -> Result<AccountId> {
        self.payer.ok_or_else(|| anyhow!("name the fee payer with --payer or WRAP_PAYER"))
    }
    /// The bridge configuration of this run: its address commits to all of it.
    fn config(&self, p: &Programs, a: &Accounts, vault: [u8; 20]) -> Config {
        Config {
            deployment_id: DEPLOYMENT_ID,
            lez_chain_id: et::LEZ_CHAIN_ID,
            zone_id: et::ZONE_ID,
            sextant: p.lc,
            instance_id: INSTANCE,
            chain_id: et::ANVIL_CHAIN_ID,
            vault,
            token_program: p.token,
            ata_program: p.ata,
            admin: a.admin,
            freezer: a.freezer,
        }
    }
    fn eth_token(&self, an: &et::Anvil, t: Token) -> [u8; 20] {
        match t {
            Token::Eth => et::ETH,
            Token::Usdt => an.usdt,
        }
    }
    fn ata(&self, p: &Programs, cfg: &Config, eth_token: &[u8; 20], owner: AccountId) -> AccountId {
        let ca = config_account(&p.br, cfg);
        let def = definition_account(&p.br, &ca, eth_token, &SALT).0;
        wrap_bridge::token::ata_account(&p.ata, &owner, &def, &p.token)
    }

    async fn account(&self, id: AccountId) -> Result<lee::Account> {
        retry(|| self.rpc.get_account(id)).await.map_err(|e| anyhow!("reading {id} from the sequencer: {e}"))
    }
    async fn shard(&self, id: AccountId, program: AccountId) -> Result<Vec<u8>> {
        Ok(self.account(id).await?.data.shard(program).to_vec())
    }
    async fn balance(&self, who: AccountId) -> Result<u128> {
        retry(|| self.rpc.get_account_balance(who)).await.map_err(|e| anyhow!("balance of {who}: {e}"))
    }
    async fn included(&self, h: HashType) -> Option<(LeeTransaction, u64)> {
        match self.rpc.get_transaction(h).await {
            Ok(Some((t, b))) => Some((t, b)),
            _ => None,
        }
    }
    async fn loaded(&self, a: AccountId) -> Result<bool> {
        Ok(!self.account(a).await?.data.shard(lee_core::program::PROGRAM_LOADER_ACCOUNT_ID).is_empty())
    }

    /// `(gas_used_exec, gas_used_stor)` of block `b`, read from its fee transaction.
    async fn block_fee_summary(&self, b: u64) -> Result<(u64, u64)> {
        let block = retry(|| self.rpc.get_block(b)).await.map_err(|e| anyhow!("getBlock {b}: {e}"))?.ok_or_else(|| anyhow!("no block {b}"))?;
        let fee = parse_id(FEE_PROGRAM)?;
        for tx in &block.body.transactions {
            if let LeeTransaction::Public(t) = tx {
                let m = t.message();
                if m.program_account_id == fee && m.instruction_data.first() == Some(&0) && m.instruction_data.len() >= 17 {
                    let d = &m.instruction_data;
                    return Ok((u64::from_le_bytes(d[1..9].try_into()?), u64::from_le_bytes(d[9..17].try_into()?)));
                }
            }
        }
        Err(anyhow!("block {b} has no fee transaction"))
    }

    /// Execute `data` to `program` over `rows` locally, signed by `signers`.
    async fn precheck(&self, w: &WalletCore, p: &Programs, program: AccountId, data: &[u8], rows: &[ProgramShardSelector], signers: &[AccountId]) -> Result<Pre> {
        let prog = |n: &str| -> Result<Program> { Ok(Program::new(self.bin(n)?.into())?) };
        let mut accounts = vec![];
        let mut seen = vec![];
        for r in rows.iter().map(|r| r.account_id).chain(signers.iter().copied()) {
            if !seen.contains(&r) {
                seen.push(r);
                accounts.push((r, self.account(r).await?));
            }
        }
        let state = V03State::new()
            .with_named_programs([
                (p.lc, prog("sextant_lc")?),
                (p.token, prog("token")?),
                (p.ata, prog("associated_token_account")?),
                (p.br, prog("wrap_bridge")?),
            ])
            .with_public_accounts(accounts);
        let fake = PrivateKey::try_new([0x5a; 32]).expect("a valid scalar");
        let mut keys: Vec<&PrivateKey> = vec![];
        let mut ids = vec![];
        for s in signers {
            keys.push(w.get_account_public_signing_key(*s).ok_or_else(|| anyhow!("the wallet holds no key for {s}"))?);
            ids.push(*s);
        }
        if keys.is_empty() {
            keys.push(&fake);
            ids.push(AccountId::from(&PublicKey::new_from_private_key(&fake)));
        }
        let nonces = ids.iter().map(|i| state.get_account_by_id(*i).nonce).collect();
        let message = Message::new_preserialized(program, rows.to_vec(), nonces, data.to_vec(), None);
        let tx = PublicTransaction::new(message.clone(), WitnessSet::for_message(&message, &keys));
        Ok(match ValidatedStateDiff::from_public_transaction_with_cycle_budget(&tx, &state, 1, 0, GAS_CAP) {
            Ok((_, charge)) => Pre::Gas(charge.cycles),
            Err(e) => Pre::Refused(typed(&e.to_string())),
        })
    }

    /// Pre-check, then send over `rows`, signed by `signers` and the payer, paid by the payer.
    #[allow(clippy::too_many_arguments)]
    async fn run_public(&self, w: &WalletCore, p: &Programs, op: &str, program: AccountId, data: Vec<u8>, rows: Vec<ProgramShardSelector>, signers: &[AccountId], fallback_gas: u64) -> Result<Value> {
        let payer = self.payer()?;
        ensure!(!signers.contains(&payer), "the payer signs separately");
        let pre = self.precheck(w, p, program, &data, &rows, signers).await?;
        let (gas_limit, pre_json) = match &pre {
            // Execution is deterministic: the chain charges exactly the pre-checked cycles.
            Pre::Gas(g) => ((g + g / 100 + 10_000).min(GAS_CAP), json!({"gas": g})),
            // A refused public transaction is included and charged its whole gas limit;
            // the limit is the honest instruction's, so running out of gas cannot refuse it.
            Pre::Refused(e) => (fallback_gas, json!({"refused": e})),
        };
        let mut line = json!({"op": op, "label": self.label, "precheck": pre_json, "gas_limit": gas_limit});
        if matches!(pre, Pre::Refused(_)) && !self.force {
            line["verdict"] = "refused-precheck".into();
            println!("{line}");
            return Ok(line);
        }
        let mut keys = vec![];
        for s in signers.iter().chain(std::iter::once(&payer)) {
            keys.push(w.get_account_public_signing_key(*s).ok_or_else(|| anyhow!("the wallet holds no key for {s}"))?.clone());
        }
        let all: Vec<AccountId> = signers.iter().copied().chain(std::iter::once(payer)).collect();
        let fees = retry(|| self.rpc.get_fee_state()).await.map_err(|e| anyhow!("getFeeState: {e}"))?;
        let build = |nonces: Vec<lee_core::account::Nonce>, max_fee| {
            let message = Message::new_preserialized(program, rows.clone(), nonces, data.clone(), Some(FeeDeclaration::new(payer, gas_limit, 0, max_fee)));
            let refs: Vec<&PrivateKey> = keys.iter().collect();
            LeeTransaction::Public(PublicTransaction::new(message.clone(), WitnessSet::for_message(&message, &refs)))
        };
        let zero = vec![lee_core::account::Nonce(0); all.len()];
        let data_bytes = borsh::to_vec(&build(zero, 0))?.len() as u128;
        let reserve = u128::from(gas_limit) * u128::from(fees.base_fee_exec) + data_bytes * u128::from(fees.base_fee_stor);
        let max_fee = reserve * 2;
        let balance = self.balance(payer).await?;
        line["tx_bytes"] = (data_bytes as u64).into();
        line["payer_balance_before"] = balance.to_string().into();
        ensure!(balance > reserve, "the payer holds {balance}, below the fee reserve {reserve}");
        let mut before = vec![];
        for r in &rows {
            before.push(self.shard(r.account_id, r.program_account_id).await?);
        }
        let mut hashes: Vec<HashType> = vec![];
        let mut found = None;
        for attempt in 1..=3 {
            let nonces = retry(|| self.rpc.get_accounts_nonces(all.clone())).await.map_err(|e| anyhow!("nonce: {e}"))?;
            match self.rpc.send_transaction(build(nonces, max_fee)).await {
                Ok(h) => {
                    eprintln!("{op}: attempt {attempt} sent {h}");
                    hashes.push(h);
                }
                Err(e) => eprintln!("{op}: attempt {attempt} not accepted by the sequencer: {e}"),
            }
            let start = std::time::Instant::now();
            while start.elapsed() < Duration::from_secs(self.wait) && found.is_none() {
                for h in &hashes {
                    if let Some((_, b)) = self.included(*h).await {
                        found = Some((*h, b));
                        break;
                    }
                }
                if found.is_none() {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
            if found.is_some() {
                break;
            }
        }
        line["attempts"] = hashes.iter().map(|h| h.to_string()).collect();
        match found {
            Some((h, b)) => {
                line["tx"] = h.to_string().into();
                line["block"] = b.into();
                let mut after = vec![];
                for r in &rows {
                    after.push(self.shard(r.account_id, r.program_account_id).await?);
                }
                let changed = after != before;
                line["state_changed"] = changed.into();
                let (exec, stor) = self.block_fee_summary(b).await?;
                line["block_gas_exec"] = exec.into();
                line["block_gas_stor"] = stor.into();
                let alone = stor == data_bytes as u64;
                line["verdict"] = match (alone, &pre) {
                    (false, _) => "included-shared-block",
                    (true, Pre::Gas(g)) if exec == *g && exec < gas_limit && changed => "applied",
                    (true, _) if exec == gas_limit && !changed => "reverted",
                    _ => "inconsistent",
                }
                .into();
            }
            None => {
                line["tx"] = hashes.last().map(|h| h.to_string()).unwrap_or_else(|| "-".into()).into();
                line["verdict"] = if hashes.is_empty() { "rejected" } else { "not-included" }.into();
            }
        }
        line["payer_balance_after"] = self.balance(payer).await?.to_string().into();
        println!("{line}");
        Ok(line)
    }

    /// The claim a client builds for deposit `d` of `an`, against the root after `n` deposits.
    fn claim_ix(&self, an: &et::Anvil, cfg: &Config, d: &et::Deposit, n: usize, destination: AccountId, secret: H256) -> wrap_bridge::Claim {
        let (root, proof) = an.proofs[n].clone();
        wrap_bridge::Claim {
            config: cfg.clone(),
            eth_token: d.token,
            salt: SALT,
            amount: d.amount,
            slot: SLOT,
            root,
            proof,
            index: d.index,
            path: wrap_core::path(&an.leaves(n), d.index as usize),
            destination,
            secret,
        }
    }

    fn claim_rows(&self, p: &Programs, cfg: &Config, c: &wrap_bridge::Claim, to_account_itself: bool) -> Vec<ProgramShardSelector> {
        let ca = config_account(&p.br, cfg);
        let nf = wrap_core::nullifier(&wrap_bridge::pairing(&p.br, cfg), &c.secret, c.index);
        let dest = if to_account_itself { c.destination } else { self.ata(p, cfg, &c.eth_token, c.destination) };
        vec![
            ProgramShardSelector::new(sextant_program::header_account(&p.lc, &cfg.instance_id, c.slot), p.lc),
            ProgramShardSelector::new(ca, p.br),
            ProgramShardSelector::new(entry_account(&p.br, &ca, &c.eth_token), p.br),
            ProgramShardSelector::new(nullifier_account(&p.br, &ca, &nf), p.br),
            ProgramShardSelector::new(definition_account(&p.br, &ca, &c.eth_token, &c.salt).0, p.token),
            ProgramShardSelector::new(dest, p.token),
        ]
    }
}

/// The run's record of the burns that landed, in the order the burn tree appended them.
fn burns_record(ctx: &Ctx) -> Result<Vec<Value>> {
    let p = ctx.run.join("burns.json");
    if !p.exists() {
        return Ok(vec![]);
    }
    Ok(read_json(&p)?.as_array().cloned().unwrap_or_default())
}

fn push_burn(ctx: &Ctx, entry: Value) -> Result<()> {
    let mut all = burns_record(ctx)?;
    all.push(entry);
    std::fs::write(ctx.run.join("burns.json"), serde_json::to_vec_pretty(&all)?)?;
    Ok(())
}

fn h32(v: &Value) -> Result<H256> {
    let s = v.as_str().ok_or_else(|| anyhow!("not a hex string: {v}"))?;
    hex::decode(s.trim_start_matches("0x"))?.try_into().map_err(|_| anyhow!("not 32 bytes: {s}"))
}

/// The burn tree's frontier as the chain holds it.
async fn frontier(ctx: &Ctx, p: &Programs, cfg: &Config) -> Result<wrap_core::Frontier> {
    let d = ctx.shard(burns_account(&p.br, &config_account(&p.br, cfg)), p.br).await?;
    Ok(if d.is_empty() { wrap_core::Frontier::default() } else { borsh::from_slice(&d)? })
}

/// The first typed refusal in an error (`E72xx` sextant_lc, `E74xx` wrap_bridge), or the error.
fn typed(e: &str) -> String {
    match et::refusal(e) {
        Some(code) => {
            let i = e.find(&format!("E{code}")).unwrap_or(0);
            e[i..].chars().take_while(|c| *c != '\n' && *c != '"').collect()
        }
        None => e.chars().take(300).collect(),
    }
}

fn holding_balance(d: &[u8]) -> Option<u128> {
    match borsh::from_slice::<wrap_bridge::token::TokenHolding>(d).ok()? {
        wrap_bridge::token::TokenHolding::Fungible { balance, .. } => Some(balance),
        _ => None,
    }
}

fn definition_supply(d: &[u8]) -> Option<u128> {
    match borsh::from_slice::<wrap_bridge::token::TokenDefinition>(d).ok()? {
        wrap_bridge::token::TokenDefinition::Fungible { total_supply, .. } => Some(total_supply),
        _ => None,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let rpc = SequencerClientBuilder::default().build(&cli.rpc).with_context(|| format!("not a sequencer URL: {}", cli.rpc))?;
    let ctx = Ctx { rpc, run: cli.run, artifacts: cli.artifacts, payer: cli.payer, force: cli.force, wait: cli.wait, label: cli.label };
    let opens_wallet = !matches!(cli.cmd, Cmd::ImageId | Cmd::Plan | Cmd::Ids | Cmd::Balance { .. } | Cmd::ReleaseProof { .. } | Cmd::ZkProve { .. } | Cmd::ChainProve { .. } | Cmd::ChainLayout { .. } | Cmd::ChainParams { .. } | Cmd::ChainForge { .. } | Cmd::AnchorProve { .. });
    let mut wallet = if opens_wallet {
        Some(WalletCore::from_env().await.context("this command needs a v0.3 wallet home (LEE_WALLET_HOME_DIR)")?)
    } else {
        None
    };
    match cli.cmd {
        Cmd::ImageId => {
            let mut out = serde_json::Map::new();
            for n in ["sextant_lc", "token", "associated_token_account", "wrap_bridge"] {
                let b = ctx.bin(n)?;
                out.insert(n.into(), json!({"image_id": image_hex(&b)?, "bytes": b.len(), "sha256": hex::encode(Sha256::digest(&b))}));
            }
            let g = PathBuf::from(std::env::var("WRAP_ZK_GUEST").unwrap_or_else(|_| "artifacts/zk/burn_membership.bin".into()));
            if let Ok(b) = std::fs::read(&g) {
                out.insert("burn_membership".into(), json!({"image_id": hex::encode(zk::image_id(&b)?), "bytes": b.len(), "sha256": hex::encode(Sha256::digest(&b))}));
            }
            for (k, var) in [("lez_chain", "WRAP_CHAIN_GUEST"), ("burn_release", "WRAP_RELEASE_GUEST"), ("bedrock_anchor", "WRAP_ANCHOR_GUEST"), ("bedrock_release", "WRAP_BEDROCK_RELEASE_GUEST")] {
                let g = PathBuf::from(std::env::var(var).unwrap_or_else(|_| format!("artifacts/zk/{k}.bin")));
                if let Ok(b) = std::fs::read(&g) {
                    out.insert(k.into(), json!({"image_id": hex::encode(zk::image_id(&b)?), "bytes": b.len(), "sha256": hex::encode(Sha256::digest(&b))}));
                }
            }
            println!("{}", Value::Object(out));
        }
        Cmd::Deploy { name, immutable } => {
            let payer = ctx.payer()?;
            let bytecode = ctx.bin(&name)?;
            let user_elf = risc0_binfmt::ProgramBinary::decode(&bytecode).map_err(|e| anyhow!("{e}"))?.user_elf.to_vec();
            let segments = user_elf.len().div_ceil(program_loader_core::MAX_SEGMENT_DATA_LEN);
            let w = wallet.as_mut().expect("deploy opens the wallet");
            let header = w.create_new_account_public(None).0;
            let segs: Vec<AccountId> = std::iter::repeat_with(|| w.create_new_account_public(None).0).take(segments).collect();
            w.store_persistent_data()?;
            let loader = ProgramLoader(w);
            let chunks: Vec<&[u8]> = user_elf.chunks(program_loader_core::MAX_SEGMENT_DATA_LEN).collect();
            let mut sent: Vec<String> = vec![];
            // Segments last to first (each names the next), then the header.
            for i in (0..segs.len()).rev() {
                let next = segs.get(i + 1).copied();
                let mut tries = 0;
                while !ctx.loaded(segs[i]).await? {
                    tries += 1;
                    ensure!(tries <= 4, "segment {i} ({}) is not on chain after 4 sends", segs[i]);
                    match loader.write_segment(segs[i], chunks[i].to_vec(), next, Some(payer)).await {
                        Ok(h) => sent.push(format!("{name}-segment{i}:{h}")),
                        Err(e) => eprintln!("segment {i}: send failed (try {tries}): {e:?}"),
                    }
                    wait_for(|| ctx.loaded(segs[i]), Duration::from_secs(ctx.wait)).await;
                }
            }
            let mut tries = 0;
            while !ctx.loaded(header).await? {
                tries += 1;
                ensure!(tries <= 4, "the header {header} is not on chain after 4 sends");
                match loader.create_header(header, segs[0], &segs, immutable, Some(payer)).await {
                    Ok(h) => sent.push(format!("{name}-header:{h}")),
                    Err(e) => eprintln!("header: send failed (try {tries}): {e:?}"),
                }
                wait_for(|| ctx.loaded(header), Duration::from_secs(ctx.wait)).await;
            }
            println!(
                "{}",
                json!({"op": "deploy", "name": name, "header": header.to_string(), "image_id": image_hex(&bytecode)?, "immutable": immutable,
                       "segments": segs.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "sent": sent})
            );
        }
        Cmd::Setup => {
            let w = wallet.as_mut().expect("setup opens the wallet");
            let mut j = serde_json::Map::new();
            for n in ["admin", "freezer", "alice", "bob"] {
                j.insert(n.into(), w.create_new_account_public(None).0.to_string().into());
            }
            j.insert("carol_private".into(), w.create_new_account_private(None).0.to_string().into());
            w.store_persistent_data()?;
            std::fs::create_dir_all(&ctx.run)?;
            std::fs::write(ctx.run.join("accounts.json"), serde_json::to_vec_pretty(&j)?)?;
            println!("{}", Value::Object(j));
        }
        Cmd::Plan => {
            let p = ctx.programs()?;
            let a = ctx.accounts()?;
            // The vault is not part of the pairing (the LEZ config pins the vault, the
            // vault pins the LEZ deployment), so it is computed before the vault exists.
            let cfg = ctx.config(&p, &a, [0; 20]);
            let pairing = wrap_bridge::pairing(&p.br, &cfg);
            let deposits: Vec<Value> = PLAN
                .iter()
                .map(|(who, n, token, amount)| {
                    let w = who_of(who);
                    let secret = wrap_core::derive_secret(&seed_of(w), b'd', *n);
                    let c = wrap_core::commitment(&pairing, a.of(w).value(), &secret);
                    json!({"from": who, "token": token, "amount": amount.to_string(), "commitment": format!("0x{}", hex::encode(c)),
                           "destination": a.of(w).to_string(), "private": w == Who::Carol})
                })
                .collect();
            let phantom = wrap_core::commitment(&pairing, a.alice.value(), &wrap_core::derive_secret(&et::ALICE_SEED, b'd', PHANTOM_N));
            println!("{}", json!({"pairing": format!("0x{}", hex::encode(pairing)), "phantom": format!("0x{}", hex::encode(phantom)), "deposits": deposits}));
        }
        Cmd::Ids => {
            let p = ctx.programs()?;
            let a = ctx.accounts()?;
            let an = ctx.anvil();
            let cfg = ctx.config(&p, &a, an.vault);
            let ca = config_account(&p.br, &cfg);
            let pair = wrap_bridge::pairing(&p.br, &cfg);
            let mut j = json!({
                "config": ca.to_string(),
                "burns": burns_account(&p.br, &ca).to_string(),
                "sextant_instance": sextant_program::instance_account(&p.lc, &INSTANCE).to_string(),
                "sextant_header": sextant_program::header_account(&p.lc, &INSTANCE, SLOT).to_string(),
                "slot": SLOT,
            });
            for (t, n) in [(Token::Eth, "eth"), (Token::Usdt, "usdt")] {
                let et_ = ctx.eth_token(&an, t);
                j[format!("{n}_entry")] = entry_account(&p.br, &ca, &et_).to_string().into();
                j[format!("{n}_definition")] = definition_account(&p.br, &ca, &et_, &SALT).0.to_string().into();
                j[format!("{n}_supply_holding")] = supply_account(&p.br, &ca, &et_, &SALT).to_string().into();
                j[format!("alice_{n}_ata")] = ctx.ata(&p, &cfg, &et_, a.alice).to_string().into();
            }
            for (i, (who, n, _, _)) in PLAN.iter().enumerate() {
                let s = wrap_core::derive_secret(&seed_of(who_of(who)), b'd', *n);
                j[format!("nullifier_{i}")] = nullifier_account(&p.br, &ca, &wrap_core::nullifier(&pair, &s, i as u64)).to_string().into();
            }
            println!("{j}");
        }
        Cmd::InitSextant => {
            let p = ctx.programs()?;
            let an = ctx.anvil();
            let d = et::devnet(&an.block, SLOT);
            eprintln!(
                "{}",
                json!({"caveat": format!("anvil has no beacon chain: the Sextant instance is initialised from a beacon header built around anvil block {} (state root 0x{}), the trusted checkpoint a light client takes on trust; the storage proofs, paths, nullifiers and mints after it are verified as on a real network", d.header.execution.block_number, hex::encode(d.header.execution.state_root))})
            );
            let ix = sextant_program::Instruction::Init {
                instance_id: INSTANCE,
                checkpoint: d.checkpoint,
                cfg: d.cfg.clone(),
                header: d.header.clone(),
                committee: d.committee.clone(),
                branch: d.branch.clone(),
            };
            let rows = vec![
                ProgramShardSelector::new(sextant_program::instance_account(&p.lc, &INSTANCE), p.lc),
                ProgramShardSelector::new(sextant_program::header_account(&p.lc, &INSTANCE, SLOT), p.lc),
            ];
            ctx.run_public(wallet.as_ref().unwrap(), &p, "init-sextant", p.lc, Program::serialize_instruction(ix)?, rows, &[], HONEST_INIT_SEXTANT).await?;
        }
        Cmd::InitBridge => {
            let p = ctx.programs()?;
            let a = ctx.accounts()?;
            let an = ctx.anvil();
            let cfg = ctx.config(&p, &a, an.vault);
            ensure!(wrap_bridge::pairing(&p.br, &cfg) == an.pairing, "the vault was deployed with another pairing than this deployment's");
            let rows = vec![ProgramShardSelector::new(config_account(&p.br, &cfg), p.br)];
            let ix = wrap_bridge::Instruction::Init { config: cfg };
            ctx.run_public(wallet.as_ref().unwrap(), &p, "init-bridge", p.br, Program::serialize_instruction(ix)?, rows, &[], HONEST_REGISTER).await?;
        }
        Cmd::Register { token } => {
            let p = ctx.programs()?;
            let a = ctx.accounts()?;
            let an = ctx.anvil();
            let cfg = ctx.config(&p, &a, an.vault);
            let ca = config_account(&p.br, &cfg);
            let eth_token = ctx.eth_token(&an, token);
            let (name, policy) = match token {
                Token::Eth => ("Wrapped Ether (anvil)", et::eth_policy()),
                Token::Usdt => ("Wrapped USDT (anvil)", et::usdt_policy()),
            };
            let rows = vec![
                ProgramShardSelector::new(ca, p.br),
                ProgramShardSelector::new(entry_account(&p.br, &ca, &eth_token), p.br),
                ProgramShardSelector::new(a.admin, p.br),
                ProgramShardSelector::new(definition_account(&p.br, &ca, &eth_token, &SALT).0, p.token),
                ProgramShardSelector::new(supply_account(&p.br, &ca, &eth_token, &SALT), p.token),
            ];
            let ix = wrap_bridge::Instruction::Register { config: cfg, eth_token, name: name.into(), salt: SALT, policy };
            ctx.run_public(wallet.as_ref().unwrap(), &p, "register", p.br, Program::serialize_instruction(ix)?, rows, &[a.admin], HONEST_REGISTER).await?;
        }
        Cmd::Claim { index, root_after, redirect_to, phantom, private } => {
            let p = ctx.programs()?;
            let a = ctx.accounts()?;
            let an = ctx.anvil();
            let cfg = ctx.config(&p, &a, an.vault);
            let n = root_after.unwrap_or(an.deposits.len());
            let (who, k, _, _) = PLAN.get(index).ok_or_else(|| anyhow!("the plan has deposits 0 to {}", PLAN.len() - 1))?;
            let who = who_of(who);
            let secret = wrap_core::derive_secret(&seed_of(who), b'd', *k);
            let d = an.deposits[index].clone();
            let mut c = ctx.claim_ix(&an, &cfg, &d, n, a.of(who), secret);
            if let Some(r) = redirect_to {
                c.destination = a.of(r);
            }
            if phantom {
                // A genuine storage proof of knownRoot[root] = 0 for a root the vault never had.
                c.secret = wrap_core::derive_secret(&et::ALICE_SEED, b'd', PHANTOM_N);
                c.destination = a.alice;
                c.index = 0;
                c.amount = 1_000_000_000_000_000_000;
                c.eth_token = et::ETH;
                c.root = an.unknown.0;
                c.proof = an.unknown.1.clone();
                let pair = wrap_bridge::pairing(&p.br, &cfg);
                let leaf = wrap_core::leaf(cfg.chain_id, &cfg.vault, &pair, &et::ETH, c.amount, &wrap_core::commitment(&pair, a.alice.value(), &c.secret));
                c.path = wrap_core::path(&[leaf], 0);
            }
            if !private {
                let rows = ctx.claim_rows(&p, &cfg, &c, false);
                let ix = wrap_bridge::Instruction::Claim(Box::new(c));
                ctx.run_public(wallet.as_ref().unwrap(), &p, "claim", p.br, Program::serialize_instruction(ix)?, rows, &[], HONEST_CLAIM).await?;
            } else {
                ensure!(c.destination == a.carol, "the private path mints into carol's private account");
                let w = wallet.as_mut().expect("claim opens the wallet");
                let rows = ctx.claim_rows(&p, &cfg, &c, true);
                let nul = rows[3];
                let mentions: Vec<AccountMention> = rows
                    .iter()
                    .map(|r| {
                        if r.account_id == a.carol {
                            AccountIdentity::PrivateOwned(r.account_id).select_program_shard(r.program_account_id)
                        } else {
                            AccountIdentity::PublicNoSign(r.account_id).select_program_shard(r.program_account_id)
                        }
                    })
                    .collect();
                let prog = |n: &str| -> Result<Program> { Ok(Program::new(ctx.bin(n)?.into())?) };
                let deps = HashMap::from([(p.lc, prog("sextant_lc")?), (p.token, prog("token")?)]);
                let pwd = lee::privacy_preserving_transaction::circuit::ProgramWithDependencies::new(prog("wrap_bridge")?, p.br, deps);
                let before = ctx.shard(nul.account_id, nul.program_account_id).await?;
                let opening = (c.destination, c.secret, d.commitment, d.leaf, c.path.clone());
                let data = Program::serialize_instruction(wrap_bridge::Instruction::Claim(Box::new(c)))?;
                eprintln!("proving the private claim locally (real proving unless RISC0_DEV_MODE is set); this takes minutes");
                let t0 = std::time::Instant::now();
                let sent = w.send_privacy_preserving_tx(mentions, data, &pwd).await;
                let prove_ms = t0.elapsed().as_millis() as u64;
                let mut line = json!({"op": "claim-private", "label": ctx.label, "prove_and_submit_ms": prove_ms,
                                      "dev_mode": std::env::var("RISC0_DEV_MODE").is_ok_and(|v| !v.is_empty() && v != "0" && v != "false")});
                match sent {
                    Err(e) => {
                        line["verdict"] = "rejected".into();
                        line["error"] = typed(&format!("{e:?}")).into();
                    }
                    Ok((h, _)) => {
                        line["tx"] = h.to_string().into();
                        line["attempts"] = json!([h.to_string()]);
                        let start = std::time::Instant::now();
                        let mut found = None;
                        while start.elapsed() < Duration::from_secs(ctx.wait.max(300)) && found.is_none() {
                            found = ctx.included(h).await;
                            if found.is_none() {
                                tokio::time::sleep(Duration::from_secs(2)).await;
                            }
                        }
                        match found {
                            None => line["verdict"] = "not-included".into(),
                            Some((tx, b)) => {
                                line["block"] = b.into();
                                let bytes = borsh::to_vec(&tx)?;
                                line["tx_bytes"] = bytes.len().into();
                                line["kind"] = match &tx {
                                    LeeTransaction::PrivacyPreserving(_) => "privacy-preserving",
                                    LeeTransaction::Public(_) => "public",
                                }
                                .into();
                                let has = |x: &[u8]| bytes.windows(x.len()).any(|y| y == x);
                                let zeros = wrap_core::zeros();
                                let nodes: Vec<&H256> = opening.4.iter().filter(|n| !zeros.contains(n)).collect();
                                line["absent_from_tx"] = json!({
                                    "destination": !has(opening.0.value()), "secret": !has(&opening.1),
                                    "commitment": !has(&opening.2), "leaf": !has(&opening.3),
                                    "path_nodes": format!("{} of {}", nodes.iter().filter(|n| !has(n.as_slice())).count(), nodes.len()),
                                });
                                let after = ctx.shard(nul.account_id, nul.program_account_id).await?;
                                line["nullifier_account"] = nul.account_id.to_string().into();
                                line["state_changed"] = (after != before).into();
                                line["verdict"] = if before.is_empty() && after == vec![1] { "applied" } else if after == before { "reverted" } else { "inconsistent" }.into();
                            }
                        }
                    }
                }
                println!("{line}");
            }
        }
        Cmd::Burn { owner, token, amount, fee, private } => {
            let p = ctx.programs()?;
            let a = ctx.accounts()?;
            let an = ctx.anvil();
            let cfg = ctx.config(&p, &a, an.vault);
            let ca = config_account(&p.br, &cfg);
            let eth_token = ctx.eth_token(&an, token);
            let o = a.of(owner);
            let recipient: [u8; 20] = et::h(&read_json(&ctx.run.join("anvil/deployment.json"))?[format!("{owner:?}").to_lowercase()]);
            let secret = wrap_core::derive_secret(&seed_of(owner), b'b', 0);
            let ec = wrap_core::eth_commitment(&recipient, fee, &secret);
            let leaf = wrap_core::burn_leaf(&wrap_bridge::pairing(&p.br, &cfg), &eth_token, amount, &ec);
            let before = frontier(&ctx, &p, &cfg).await?;
            let record = |tx: &str, block: u64| {
                json!({"owner": format!("{owner:?}").to_lowercase(), "token": format!("0x{}", hex::encode(eth_token)), "amount": amount.to_string(),
                       "recipient": format!("0x{}", hex::encode(recipient)), "fee": fee.to_string(), "secret": format!("0x{}", hex::encode(secret)),
                       "leaf": format!("0x{}", hex::encode(leaf)), "index": before.next_index, "tx": tx, "block": block, "private": private})
            };
            if !private {
                let rows = vec![
                    ProgramShardSelector::new(ca, p.br),
                    ProgramShardSelector::new(entry_account(&p.br, &ca, &eth_token), p.br),
                    ProgramShardSelector::new(burns_account(&p.br, &ca), p.br),
                    ProgramShardSelector::new(definition_account(&p.br, &ca, &eth_token, &SALT).0, p.token),
                    ProgramShardSelector::new(o, p.br),
                    ProgramShardSelector::new(ctx.ata(&p, &cfg, &eth_token, o), p.token),
                ];
                let ix = wrap_bridge::Instruction::Burn { config: cfg.clone(), eth_token, salt: SALT, amount, eth_commitment: ec };
                let line = ctx.run_public(wallet.as_ref().unwrap(), &p, "burn", p.br, Program::serialize_instruction(ix)?, rows, &[o], HONEST_BURN).await?;
                if line["verdict"] == "applied" {
                    let after = frontier(&ctx, &p, &cfg).await?;
                    ensure!(after.next_index == before.next_index + 1, "the burn tree did not grow by one leaf");
                    push_burn(&ctx, record(line["tx"].as_str().unwrap_or("-"), line["block"].as_u64().unwrap_or(0)))?;
                }
            } else {
                ensure!(owner == Who::Carol, "the private path burns from carol's private account");
                let w = wallet.as_mut().expect("burn opens the wallet");
                w.sync_to_latest_block().await?;
                w.store_persistent_data()?;
                let held = w.get_account_private(a.carol).ok_or_else(|| anyhow!("the wallet does not hold carol's private account"))?;
                let held_balance = holding_balance(&held.data.shard(p.token).to_vec()).unwrap_or(0);
                let mut line = json!({"op": "burn-private", "label": ctx.label, "held_before": held_balance.to_string(),
                                      "dev_mode": std::env::var("RISC0_DEV_MODE").is_ok_and(|v| !v.is_empty() && v != "0" && v != "false")});
                // The policy and freeze flags live in public accounts the plan cannot
                // read: settlement checks them (and a sequencer drops a private
                // transaction that fails there). Check them here, before proving.
                let entry: wrap_bridge::TokenEntry = borsh::from_slice(&ctx.shard(entry_account(&p.br, &ca, &eth_token), p.br).await?)?;
                let live: wrap_bridge::Live = borsh::from_slice(&ctx.shard(ca, p.br).await?)?;
                let refusal = if !entry.policy.denominations.contains(&amount) {
                    Some(format!("E{}: burn one of the token's permitted amounts", wrap_bridge::E_AMOUNT))
                } else if entry.frozen_burn || live.frozen_burn {
                    Some(format!("E{}: burning of this token is frozen", wrap_bridge::E_FROZEN))
                } else if held_balance < amount {
                    Some("the private account holds less than the amount".into())
                } else {
                    None
                };
                if let Some(r) = refusal {
                    line["precheck"] = json!({"refused": r});
                    line["verdict"] = "refused-precheck".into();
                    println!("{line}");
                    return Ok(());
                }
                let rows = vec![
                    ProgramShardSelector::new(ca, p.br),
                    ProgramShardSelector::new(entry_account(&p.br, &ca, &eth_token), p.br),
                    ProgramShardSelector::new(burns_account(&p.br, &ca), p.br),
                    ProgramShardSelector::new(definition_account(&p.br, &ca, &eth_token, &SALT).0, p.token),
                    ProgramShardSelector::new(a.carol, p.token),
                ];
                let mentions: Vec<AccountMention> = rows
                    .iter()
                    .map(|r| {
                        if r.account_id == a.carol {
                            AccountIdentity::PrivateOwned(r.account_id).select_program_shard(r.program_account_id)
                        } else {
                            AccountIdentity::PublicNoSign(r.account_id).select_program_shard(r.program_account_id)
                        }
                    })
                    .collect();
                let prog = |n: &str| -> Result<Program> { Ok(Program::new(ctx.bin(n)?.into())?) };
                let deps = HashMap::from([(p.token, prog("token")?)]);
                let pwd = lee::privacy_preserving_transaction::circuit::ProgramWithDependencies::new(prog("wrap_bridge")?, p.br, deps);
                let data = Program::serialize_instruction(wrap_bridge::Instruction::BurnFrom { config: cfg.clone(), eth_token, salt: SALT, amount, eth_commitment: ec })?;
                eprintln!("proving the private burn locally (real proving unless RISC0_DEV_MODE is set); this takes minutes");
                let t0 = std::time::Instant::now();
                let sent = w.send_privacy_preserving_tx(mentions, data, &pwd).await;
                line["prove_and_submit_ms"] = (t0.elapsed().as_millis() as u64).into();
                match sent {
                    Err(e) => {
                        line["verdict"] = "rejected".into();
                        line["error"] = typed(&format!("{e:?}")).into();
                    }
                    Ok((h, _)) => {
                        line["tx"] = h.to_string().into();
                        line["attempts"] = json!([h.to_string()]);
                        let start = std::time::Instant::now();
                        let mut found = None;
                        while start.elapsed() < Duration::from_secs(ctx.wait.max(300)) && found.is_none() {
                            found = ctx.included(h).await;
                            if found.is_none() {
                                tokio::time::sleep(Duration::from_secs(2)).await;
                            }
                        }
                        match found {
                            None => line["verdict"] = "not-included".into(),
                            Some((tx, b)) => {
                                line["block"] = b.into();
                                let bytes = borsh::to_vec(&tx)?;
                                line["tx_bytes"] = bytes.len().into();
                                line["kind"] = match &tx {
                                    LeeTransaction::PrivacyPreserving(_) => "privacy-preserving",
                                    LeeTransaction::Public(_) => "public",
                                }
                                .into();
                                let has = |x: &[u8]| bytes.windows(x.len()).any(|y| y == x);
                                line["absent_from_tx"] = json!({"holder": !has(a.carol.value()), "recipient": !has(&recipient), "secret": !has(&secret)});
                                line["leaf_in_tx"] = has(&leaf).into();
                                let after = frontier(&ctx, &p, &cfg).await?;
                                line["burns_account"] = burns_account(&p.br, &ca).to_string().into();
                                let mut expect = before.clone();
                                expect.insert(&leaf);
                                line["state_changed"] = (after != before).into();
                                line["verdict"] = if after == expect { "applied" } else if after == before { "reverted" } else { "inconsistent" }.into();
                                if after == expect {
                                    push_burn(&ctx, record(&h.to_string(), b))?;
                                }
                            }
                        }
                    }
                }
                println!("{line}");
            }
        }
        Cmd::ZkProve { input, guest, chain: chain_rec, release_guest, bedrock: bedrock_rec, bedrock_release_guest } => {
            let i = zk::input_from_json(&read_json(&input)?)?;
            let mut out = match (&chain_rec, &bedrock_rec) {
                (_, Some(bp)) => {
                    let rec = read_json(bp)?;
                    let journal = hex::decode(rec["journal"].as_str().unwrap_or("").trim_start_matches("0x"))?;
                    let receipt = chain::load_receipt(&chain::receipt_path(bp))?;
                    let elf = std::fs::read(&bedrock_release_guest).with_context(|| format!("reading {}", bedrock_release_guest.display()))?;
                    bedrock::prove_release(&elf, &i, &journal, receipt)?
                }
                (None, None) => zk::prove(&std::fs::read(&guest).with_context(|| format!("reading {}", guest.display()))?, &i)?,
                (Some(cp), None) => {
                    let rec = read_json(cp)?;
                    let journal = hex::decode(rec["journal"].as_str().unwrap_or("").trim_start_matches("0x"))?;
                    let receipt = chain::load_receipt(&chain::receipt_path(cp))?;
                    let elf = std::fs::read(&release_guest).with_context(|| format!("reading {}", release_guest.display()))?;
                    chain::prove_release(&elf, &i, &journal, receipt)?
                }
            };
            out["op"] = "zk-prove".into();
            println!("{out}");
        }
        Cmd::ReleaseProof { owner, zk, relayer, guest, chain: chain_rec, release_guest, bedrock: bedrock_rec, bedrock_release_guest, vault: vault_override } => {
            let p = ctx.programs()?;
            let a = ctx.accounts()?;
            let an = ctx.anvil();
            let cfg = ctx.config(&p, &a, an.vault);
            let burns = burns_record(&ctx)?;
            let leaves: Vec<H256> = burns.iter().map(|b| h32(&b["leaf"])).collect::<Result<_>>()?;
            let chain = frontier(&ctx, &p, &cfg).await?;
            // The client's leaves must be the chain's tree: the same count and the same root.
            ensure!(chain.next_index == leaves.len() as u64 && chain.root == wrap_core::root(&leaves),
                "the recorded burns ({}) do not rebuild the chain's burn tree ({} leaves, root {})", leaves.len(), chain.next_index, hex::encode(chain.root));
            let me = format!("{owner:?}").to_lowercase();
            let (i, b) = burns.iter().enumerate().find(|(_, b)| b["owner"] == me.as_str()).ok_or_else(|| anyhow!("no burn by {me} in the record"))?;
            let secret = h32(&b["secret"])?;
            let pair = wrap_bridge::pairing(&p.br, &cfg);
            let path = wrap_core::path(&leaves, i);
            ensure!(wrap_core::root_from_path(&leaves[i], i as u64, &path) == Some(chain.root));
            let mut out = json!({"op": "release-proof", "owner": me, "token": b["token"], "amount": b["amount"], "recipient": b["recipient"], "fee": b["fee"],
                       "secret": b["secret"], "index": i, "leaf": b["leaf"], "path": path.iter().map(|x| format!("0x{}", hex::encode(x))).collect::<Vec<_>>(),
                       "root": format!("0x{}", hex::encode(chain.root)), "nullifier": format!("0x{}", hex::encode(wrap_core::burn_nullifier(&pair, &secret, i as u64))),
                       "burn_tx": b["tx"], "burn_block": b["block"], "private": b["private"],
                       "burns_account": burns_account(&p.br, &config_account(&p.br, &cfg)).to_string(), "bridge_program": p.br.to_string()});
            if zk {
                let relayer = relayer.as_deref().ok_or_else(|| anyhow!("--zk needs --relayer"))?;
                let a20 = |v: &Value| -> Result<[u8; 20]> {
                    let s = v.as_str().ok_or_else(|| anyhow!("not an address: {v}"))?;
                    hex::decode(s.trim_start_matches("0x"))?.try_into().map_err(|_| anyhow!("not 20 bytes: {s}"))
                };
                let num = |v: &Value| -> Result<u128> { v.as_str().ok_or_else(|| anyhow!("not a decimal: {v}"))?.parse().map_err(|e| anyhow!("{e}")) };
                let release_vault = match &vault_override {
                    Some(v) => a20(&json!(v))?,
                    None => an.vault,
                };
                let input = wrap_core::zk::ZkReleaseInput {
                    chain_id: et::ANVIL_CHAIN_ID, vault: release_vault, pairing: pair, relayer: a20(&json!(relayer))?,
                    token: a20(&b["token"])?, amount: num(&b["amount"])?, recipient: a20(&b["recipient"])?, fee: num(&b["fee"])?,
                    secret, index: i as u64, path: path.clone(),
                };
                if let Some(cp) = &chain_rec {
                    // The chain-derived tree: the private burns the chain guest rebuilt from LEZ blocks.
                    let rec = read_json(cp)?;
                    let derived: Vec<H256> = rec["leaves"].as_array().cloned().unwrap_or_default().iter().map(|l| h32(&l["leaf"])).collect::<Result<_>>()?;
                    let leaf = h32(&b["leaf"])?;
                    let j = derived.iter().position(|l| *l == leaf).ok_or_else(|| {
                        anyhow!("{me}'s burn is not in the chain-derived tree (only privacy-preserving burns are; a public burn may have reverted and still be in its block)")
                    })?;
                    let dpath = wrap_core::path(&derived, j);
                    let droot = chain::h(&rec["root"])?;
                    ensure!(wrap_core::root_from_path(&leaf, j as u64, &dpath) == Some(droot), "the chain record's leaves do not rebuild its root");
                    let input = wrap_core::zk::ZkReleaseInput { index: j as u64, path: dpath.clone(), ..input };
                    let journal = hex::decode(rec["journal"].as_str().unwrap_or("").trim_start_matches("0x"))?;
                    let z = match &bedrock_rec {
                        None => {
                            let receipt = chain::load_receipt(&chain::receipt_path(cp))?;
                            let elf = std::fs::read(&release_guest).with_context(|| format!("reading {}", release_guest.display()))?;
                            chain::prove_release(&elf, &input, &journal, receipt)?
                        }
                        Some(bp) => {
                            // The anchor record must carry this very chain journal.
                            let brec = read_json(bp)?;
                            let aj = hex::decode(brec["journal"].as_str().unwrap_or("").trim_start_matches("0x"))?;
                            let (_, cj) = lez_bedrock::compose::parse_anchor_journal(&aj).ok_or_else(|| anyhow!("{} is not an anchor record", bp.display()))?;
                            ensure!(cj == &journal[..], "the anchor record is of another chain proof than {}", cp.display());
                            let receipt = chain::load_receipt(&chain::receipt_path(bp))?;
                            let elf = std::fs::read(&bedrock_release_guest).with_context(|| format!("reading {}", bedrock_release_guest.display()))?;
                            let z = bedrock::prove_release(&elf, &input, &aj, receipt)?;
                            out["bedrock"] = json!({"bedrock_block": brec["bedrock_block"], "top": brec["top"], "channel_id": brec["channel_id"], "signer": brec["signer"]});
                            z
                        }
                    };
                    ensure!(z["fields"]["root"] == rec["root"], "the guest proved another root than the chain record's");
                    out["chain_derived"] = json!({"index": j, "root": rec["root"], "leaves": derived.len(), "tip": rec["tip"], "anchor": rec["anchor"],
                                                  "nullifier": format!("0x{}", hex::encode(wrap_core::burn_nullifier(&pair, &secret, j as u64)))});
                    out["chain_path"] = json!(dpath.iter().map(|x| format!("0x{}", hex::encode(x))).collect::<Vec<_>>());
                    out["relayer"] = relayer.into();
                    out["zk"] = z;
                } else {
                    let elf = std::fs::read(&guest).with_context(|| format!("reading {}", guest.display()))?;
                    let z = zk::prove(&elf, &input)?;
                    ensure!(z["fields"]["root"] == out["root"] && z["fields"]["nullifier"] == out["nullifier"],
                            "the guest proved another root or nullifier than the chain's");
                    out["relayer"] = relayer.into();
                    out["zk"] = z;
                }
            }
            println!("{out}");
        }
        Cmd::ChainProve { anchor, prior, to, out, save_blocks, blocks_dir, anchor_hash, guest } => {
            let p = ctx.programs()?;
            let a = ctx.accounts()?;
            let an = ctx.anvil();
            let cfg = ctx.config(&p, &a, an.vault);
            let params = lez_chain::Params { program_id: *p.br.value(), burns_account: *burns_account(&p.br, &config_account(&p.br, &cfg)).value() };
            let elf = std::fs::read(&guest).with_context(|| format!("reading {}", guest.display()))?;
            let me = zk::image_id(&elf)?;
            let (prior_journal, prior_receipt, state, mut leaves) = match &prior {
                Some(pj) => {
                    let rec = read_json(pj)?;
                    let journal = hex::decode(rec["journal"].as_str().unwrap_or("").trim_start_matches("0x"))?;
                    let (_, _, s) = lez_chain::parse_journal(&journal).ok_or_else(|| anyhow!("{} is not a chain record", pj.display()))?;
                    (Some(journal), Some(chain::load_receipt(&chain::receipt_path(pj))?), s, rec["leaves"].as_array().cloned().unwrap_or_default())
                }
                None => {
                    let aid = anchor.ok_or_else(|| anyhow!("name the anchor block (--anchor) or a prior record (--prior)"))?;
                    let ah = match &anchor_hash {
                        Some(h) => chain::h(&json!(h))?,
                        None => retry(|| ctx.rpc.get_block(aid)).await.map_err(|e| anyhow!("getBlock {aid}: {e}"))?.ok_or_else(|| anyhow!("no block {aid}"))?.header.hash.0,
                    };
                    (None, None, lez_chain::ChainState::at_anchor(aid, ah), vec![])
                }
            };
            let to = match to {
                Some(t) => t,
                None => retry(|| ctx.rpc.get_last_block_id()).await.map_err(|e| anyhow!("getLastBlockId: {e}"))?,
            };
            ensure!(to > state.tip_id, "nothing to prove: the tip is {} and --to {to}", state.tip_id);
            let mut blocks = Vec::new();
            for id in state.tip_id + 1..=to {
                let raw = match &blocks_dir {
                    Some(d) => std::fs::read(d.join(format!("{id}.borsh"))).with_context(|| format!("block {id} in {}", d.display()))?,
                    None => {
                        let blk = retry(|| ctx.rpc.get_block(id)).await.map_err(|e| anyhow!("getBlock {id}: {e}"))?.ok_or_else(|| anyhow!("no block {id}"))?;
                        chain::raw_block(&blk, &params)?
                    }
                };
                if let Some(d) = &save_blocks {
                    std::fs::create_dir_all(d)?;
                    std::fs::write(d.join(format!("{id}.borsh")), &raw)?;
                }
                blocks.push(raw);
            }
            let mut replayed = state.clone();
            leaves.extend(chain::replay(&mut replayed, &params, &blocks)?);
            let input = lez_chain::ChainInput {
                self_image_id: me, params, anchor_id: state.anchor_id, anchor_hash: state.anchor_hash, prior: prior_journal, blocks,
            };
            let (receipt, mut rec) = chain::prove_chain(&elf, &input, prior_receipt)?;
            ensure!(rec["root"] == json!(format!("0x{}", hex::encode(replayed.tree.root))), "the guest's tree is not the host's replay");
            rec["op"] = "chain-prove".into();
            rec["params"] = json!({"program_id": format!("0x{}", hex::encode(params.program_id)), "burns_account": format!("0x{}", hex::encode(params.burns_account)),
                                   "program": p.br.to_string(), "burns": burns_account(&p.br, &config_account(&p.br, &cfg)).to_string()});
            rec["from"] = (state.tip_id + 1).into();
            rec["leaves"] = Value::Array(leaves);
            std::fs::write(&out, serde_json::to_vec_pretty(&rec)?)?;
            chain::save_receipt(&chain::receipt_path(&out), &receipt)?;
            rec["journal"] = Value::Null;
            println!("{rec}");
        }
        Cmd::AnchorProve { chain: cp, block, out, guest } => {
            let rec = read_json(&cp)?;
            let chain_journal = hex::decode(rec["journal"].as_str().unwrap_or("").trim_start_matches("0x"))?;
            let chain_receipt = chain::load_receipt(&chain::receipt_path(&cp))?;
            let (channel_id, signer, b) = bedrock::anchor_block(&read_json(&block)?)?;
            let elf = std::fs::read(&guest).with_context(|| format!("reading {}", guest.display()))?;
            let input = lez_bedrock::compose::AnchorInput { self_image_id: zk::image_id(&elf)?, channel_id, signer, chain_journal, block: b };
            let (receipt, mut r) = bedrock::prove_anchor(&elf, &input, chain_receipt)?;
            r["chain"] = json!(cp);
            std::fs::write(&out, serde_json::to_string_pretty(&r)?)?;
            chain::save_receipt(&chain::receipt_path(&out), &receipt)?;
            println!("{r}");
        }
        Cmd::ChainParams { at } => {
            let p = ctx.programs()?;
            let a = ctx.accounts()?;
            let an = ctx.anvil();
            let cfg = ctx.config(&p, &a, an.vault);
            let burns = burns_account(&p.br, &config_account(&p.br, &cfg));
            let id = match at {
                Some(b) => b,
                None => retry(|| ctx.rpc.get_last_block_id()).await.map_err(|e| anyhow!("getLastBlockId: {e}"))?,
            };
            let blk = retry(|| ctx.rpc.get_block(id)).await.map_err(|e| anyhow!("getBlock {id}: {e}"))?.ok_or_else(|| anyhow!("no block {id}"))?;
            println!("{}", json!({"op": "chain-params", "program_id": format!("0x{}", hex::encode(p.br.value())), "burns_account": format!("0x{}", hex::encode(burns.value())),
                                  "program": p.br.to_string(), "burns": burns.to_string(), "pairing": format!("0x{}", hex::encode(wrap_bridge::pairing(&p.br, &cfg))),
                                  "block_id": id, "block_hash": format!("0x{}", hex::encode(blk.header.hash.0))}));
        }
        Cmd::ChainForge { anchor, anchor_hash, leaf, out_dir } => {
            use lez_chain::block::*;
            let p = ctx.programs()?;
            let a = ctx.accounts()?;
            let an = ctx.anvil();
            let cfg = ctx.config(&p, &a, an.vault);
            let program = *p.br.value();
            let burns = *burns_account(&p.br, &config_account(&p.br, &cfg)).value();
            let leaf = chain::h(&json!(leaf))?;
            let tx = LeeTransaction::PrivacyPreserving(PrivacyPreservingTransaction {
                message: PrivacyMessage {
                    public_actions: vec![PublicActionWithId {
                        account_id: burns,
                        effects: vec![DeferredPublicEffect {
                            program_account_id: program,
                            shard_program_account_id: program,
                            data: [&[lez_chain::APPEND_VARIANT][..], &leaf].concat(),
                        }],
                    }],
                    nonces: vec![],
                    private_actions: vec![],
                    block_validity_window: ValidityWindow { from: None, to: None },
                    timestamp_validity_window: ValidityWindow { from: None, to: None },
                    program_image_claims: vec![],
                },
                witness_set: PrivacyWitnessSet { signatures_and_public_keys: vec![], proof: vec![] },
            });
            let mut b = Block {
                header: BlockHeader { block_id: anchor + 1, prev_block_hash: chain::h(&json!(anchor_hash))?, hash: [0; 32], timestamp: 0 },
                body: BlockBody { transactions: vec![tx] },
                bedrock_status: BedrockStatus::Pending,
            };
            b.header.hash = lez_chain::content_hash(&borsh::to_vec(&b)?);
            let raw = borsh::to_vec(&b)?;
            std::fs::create_dir_all(&out_dir)?;
            std::fs::write(out_dir.join(format!("{}.borsh", anchor + 1)), &raw)?;
            println!("{}", json!({"op": "chain-forge", "block_id": anchor + 1, "block_hash": format!("0x{}", hex::encode(b.header.hash)), "bytes": raw.len()}));
        }
        Cmd::ChainLayout { dir } => {
            let p = ctx.programs()?;
            let a = ctx.accounts()?;
            let an = ctx.anvil();
            let cfg = ctx.config(&p, &a, an.vault);
            let params = lez_chain::Params { program_id: *p.br.value(), burns_account: *burns_account(&p.br, &config_account(&p.br, &cfg)).value() };
            let mut leaves = 0usize;
            let mut n = 0usize;
            let mut bytes = 0usize;
            let mut kinds = (0usize, 0usize);
            for e in std::fs::read_dir(&dir)? {
                let path = e?.path();
                if path.extension().and_then(|x| x.to_str()) != Some("borsh") {
                    continue;
                }
                let raw = std::fs::read(&path)?;
                chain::check_layout(&raw, &params).with_context(|| format!("{}", path.display()))?;
                let b: lez_chain::block::Block = borsh::from_slice(&raw)?;
                leaves += lez_chain::private_burn_leaves(&b, &params).len();
                for t in &b.body.transactions {
                    match t {
                        lez_chain::block::LeeTransaction::Public(_) => kinds.0 += 1,
                        lez_chain::block::LeeTransaction::PrivacyPreserving(_) => kinds.1 += 1,
                    }
                }
                n += 1;
                bytes += raw.len();
            }
            println!("{}", json!({"op": "chain-layout", "blocks": n, "bytes": bytes, "public_txs": kinds.0, "private_txs": kinds.1, "burn_leaves": leaves, "identical": true}));
        }
        Cmd::ShowPrivate => {
            let p = ctx.programs()?;
            let a = ctx.accounts()?;
            let w = wallet.as_mut().expect("show-private opens the wallet");
            let synced = w.sync_to_latest_block().await?;
            w.store_persistent_data()?;
            let acc = w.get_account_private(a.carol).ok_or_else(|| anyhow!("the wallet does not hold carol's private account"))?;
            let shard = acc.data.shard(p.token).to_vec();
            let cm = lee_core::Commitment::new(&a.carol, &acc);
            let (proofs, _) = w.get_proofs_and_root(&[cm.clone()]).await?;
            let def = borsh::from_slice::<wrap_bridge::token::TokenHolding>(&shard).ok().map(|h| match h {
                wrap_bridge::token::TokenHolding::Fungible { definition_id, .. } => definition_id.to_string(),
                _ => "not fungible".into(),
            });
            println!(
                "{}",
                json!({"op": "show-private", "synced_to_block": synced, "account": format!("Private/{}", a.carol),
                       "wrapped_balance": holding_balance(&shard).map(|b| b.to_string()), "definition": def,
                       "commitment_on_chain": proofs.first().is_some_and(Option::is_some)})
            );
        }
        Cmd::Balance { owner, token } => {
            let p = ctx.programs()?;
            let a = ctx.accounts()?;
            let an = ctx.anvil();
            let cfg = ctx.config(&p, &a, an.vault);
            let ca = config_account(&p.br, &cfg);
            let eth_token = ctx.eth_token(&an, token);
            let bal = holding_balance(&ctx.shard(ctx.ata(&p, &cfg, &eth_token, a.of(owner)), p.token).await?).unwrap_or(0);
            let sup = definition_supply(&ctx.shard(definition_account(&p.br, &ca, &eth_token, &SALT).0, p.token).await?);
            println!("{}", json!({"owner": format!("{owner:?}").to_lowercase(), "token": format!("{token:?}").to_lowercase(), "balance": bal.to_string(), "supply": sup.map(|s| s.to_string())}));
        }
    }
    Ok(())
}

/// A read from the sequencer, tried up to five times.
async fn retry<T, E: std::fmt::Display, F, Fut>(f: F) -> std::result::Result<T, E>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, E>>,
{
    let mut last = None;
    for i in 0..5u64 {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                eprintln!("sequencer read failed (try {}): {e}", i + 1);
                last = Some(e);
                tokio::time::sleep(Duration::from_secs(2 * (i + 1))).await;
            }
        }
    }
    Err(last.expect("five tries"))
}

/// Polls `done` every 2 s until it holds or `limit` passes.
async fn wait_for<F, Fut>(done: F, limit: Duration)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    let start = std::time::Instant::now();
    while start.elapsed() < limit {
        if done().await.unwrap_or(false) {
            return;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

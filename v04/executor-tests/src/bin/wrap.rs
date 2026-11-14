//! `wrap` on LEZ v0.4.0-rc3: the client of the v0.4 port, against a LEZ
//! v0.4.0-rc3 sequencer (v04/scripts/e2e-seq.sh runs a local standalone one).
//! The v0.3 client (cli/, `wrap` on LEZ v0.3) drives the same steps on v0.3.
//!
//! It loads the three committed programs (v04/artifacts: token, sextant_lc,
//! wrap_bridge) through the program loader, writes the deposit plan the vault
//! is deployed and funded with (scripts/anvil_deposits.py), initialises
//! Sextant's light client on a devnet beacon header around the anvil block,
//! initialises the bridge, registers ETH and the ERC-20 (a USDT-shaped token),
//! opens holdings, sends claims and burns, reads balances, and gives a burn's
//! release opening, against the deposits made into WrapVault on that anvil
//! chain.
//!
//! **Every transaction is pre-checked**: the same root call, signed by the
//! same keys, runs first on rc3's own state machine (`lee::V03State`,
//! `ValidatedStateDiff::from_public_transaction_metered`, the call settlement
//! makes) with the committed binaries at the deployed program ids and every
//! account it declares as the sequencer holds it now (`getAccount`). One the
//! programs accept is sent; one they refuse is reported with its typed code
//! (`E72xx` Sextant, `E74xx` the bridge, or the token program's message) and
//! sent only with `--force`, so that the chain's own verdict is on record. On
//! v0.4 a refused transaction is included, charged, and listed in its
//! block's `reverted` list; the client reads that list and fails if the chain
//! and the pre-check disagree.
//!
//! **Claims are public.** A private claim on v0.4 is a privacy-preserving
//! transaction whose fee is paid inside its proof by a funded private
//! account; the bridge's private paths are not ported yet (docs/V04-PORT.md
//! section 2.6), so `claim --private` is refused here rather than silently
//! sent in public. (The v0.3 client also claims publicly unless `--private`.)
//!
//! A run directory (`--run`) holds what the steps share: programs.json,
//! plan.json, anvil/ (scripts/anvil_deposits.py), manifest.tsv (every
//! transaction: label, pre-check, chain verdict, block, index, hash),
//! burns.json (the applied burns, in tree order). Keys are the fixed test
//! keys of the executor tests; the payer is funded by the local genesis.
//! Nothing here reaches a public network.

use std::path::PathBuf;

use borsh::BorshSerialize;
use clap::{Parser, Subcommand, ValueEnum};
use lee::{AccountId, Actor, PublicTransaction, V03State, ValidatedStateDiff};
use lee_core::program::PdaSeed;
use serde_json::{json, Value};
use wrap_bridge_v04::{Config, Message as Bm};
use wrap_core::H256;
use wrap_executor_tests_v04::seq::{self, Rpc};
use wrap_executor_tests_v04::*;

/// The deposits a run makes, in order: (depositor = destination, seed counter, token, amount).
/// Each account holds one wrapped token (no ATA program on v0.4 yet), so
/// alice's deposits are ETH and bob's are USDT.
const PLAN: [(Who, u64, Tok, u128); 4] = [
    (Who::Alice, 0, Tok::Eth, 1_000_000_000_000_000_000),
    (Who::Alice, 1, Tok::Eth, 1_000_000_000_000_000_000),
    (Who::Bob, 0, Tok::Usdt, 100_000_000),
    (Who::Bob, 1, Tok::Usdt, 1_000_000_000),
];
/// The seed counter of the deposit alice never made.
const PHANTOM_N: u64 = 9;

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq, Debug)]
enum Tok {
    Eth,
    Usdt,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq, Debug)]
enum Who {
    Alice,
    Bob,
}

impl Who {
    fn key(self) -> Key {
        match self {
            Who::Alice => alice(),
            Who::Bob => bob(),
        }
    }
    fn seed(self) -> H256 {
        match self {
            Who::Alice => ALICE_SEED,
            Who::Bob => BOB_SEED,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Who::Alice => "alice",
            Who::Bob => "bob",
        }
    }
}

#[derive(Parser)]
#[command(name = "wrap", about = "the RFP-022 bridge prototype on a LEZ v0.4.0-rc3 sequencer")]
struct Cli {
    /// The sequencer.
    #[arg(long, env = "WRAP_RPC", global = true, default_value = "http://127.0.0.1:3077")]
    rpc: String,
    /// The run directory.
    #[arg(long, env = "WRAP_RUN", global = true, default_value = "run-v04")]
    run: PathBuf,
    /// Send even if the pre-check says the programs refuse.
    #[arg(long, global = true)]
    force: bool,
    /// The manifest label (default: the command).
    #[arg(long, global = true)]
    label: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// The fee payer's public key, hex (for the sequencer's genesis).
    Pubkey,
    /// Size and ImageID of each committed v0.4 binary.
    ImageId,
    /// Load token, sextant_lc, wrap_bridge through the program loader (programs.json).
    Deploy,
    /// The pairing the vault is deployed with and each deposit's commitment (plan.json).
    Plan,
    /// Sextant's `Init` on a devnet beacon header committing to the anvil block.
    InitSextant,
    /// The bridge's `Init` (config and burn tree).
    InitBridge,
    /// `Register` a wrapped token (signed by the admin).
    Register {
        #[arg(value_enum)]
        token: Tok,
    },
    /// `owner` opens its holding of the wrapped `token` (one per account).
    OpenHolding {
        #[arg(value_enum)]
        owner: Who,
        #[arg(value_enum)]
        token: Tok,
    },
    /// Claim deposit `index` of the plan to the account it committed to, relayed by the payer.
    Claim {
        #[arg(required_unless_present = "phantom")]
        index: Option<usize>,
        /// The root of the vault's tree after this many deposits (default: all).
        #[arg(long)]
        root_after: Option<usize>,
        /// Open the deposit with another destination than the one it committed to.
        #[arg(long, value_enum)]
        redirect_to: Option<Who>,
        /// Claim it as another token than the one deposited.
        #[arg(long, value_enum)]
        as_token: Option<Tok>,
        /// Claim the deposit alice never made, under the root of a tree holding only it
        /// (the vault never had that root; its `knownRoot` slot is proven empty).
        #[arg(long)]
        phantom: bool,
        /// A privacy-preserving claim: not on v0.4 yet (refused, see the module docs).
        #[arg(long)]
        private: bool,
    },
    /// `owner` burns `amount` of the wrapped `token`; the Ethereum recipient is
    /// the owner's anvil address, with `--fee` for a relayer, in the burn's commitment.
    Burn {
        #[arg(value_enum)]
        owner: Who,
        #[arg(value_enum)]
        token: Tok,
        amount: u128,
        #[arg(long, default_value_t = 0)]
        fee: u128,
    },
    /// `owner`'s wrapped balance and the token's supply, as the chain holds them.
    Balance {
        #[arg(value_enum)]
        owner: Who,
        #[arg(value_enum)]
        token: Tok,
    },
    /// The opening a release needs for `owner`'s `n`-th applied burn: leaf,
    /// index, path to the root the chain's burn tree holds now, nullifier.
    ReleaseProof {
        #[arg(value_enum)]
        owner: Who,
        #[arg(long, default_value_t = 0)]
        n: usize,
    },
    /// Save every block from `--from` to the tip (blocks/) and close manifest.tsv
    /// with the run's programs, config, burn leaves and the chain's burn-tree root.
    Finish {
        #[arg(long)]
        from: u64,
    },
    /// The last block id (to mark where a run starts).
    Tip,
}

fn hx(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

fn h32(v: &Value) -> H256 {
    h(v)
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("wrap: {msg}");
    std::process::exit(1)
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Copy)]
struct Programs {
    token: AccountId,
    sextant_lc: AccountId,
    wrap_bridge: AccountId,
}

struct Ctx {
    rpc: Rpc,
    run: PathBuf,
    force: bool,
    label: Option<String>,
}

/// The pre-check's verdict: accepted (cycles), or the refusal.
enum Pre {
    Accepted(u64),
    Refused(String),
}

impl Ctx {
    fn read(&self, name: &str) -> Value {
        let p = self.run.join(name);
        serde_json::from_slice(&std::fs::read(&p).unwrap_or_else(|_| fail(format!("missing {}", p.display())))).unwrap_or_else(|e| fail(format!("{}: {e}", p.display())))
    }

    fn programs(&self) -> Programs {
        serde_json::from_value(self.read("programs.json")).unwrap_or_else(|e| fail(e))
    }

    fn anvil(&self) -> Anvil {
        anvil_at(&self.run.join("anvil"))
    }

    fn config(&self, p: &Programs, vault: [u8; 20]) -> Config {
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

    fn token(&self, a: &Anvil, t: Tok) -> [u8; 20] {
        match t {
            Tok::Eth => ETH,
            Tok::Usdt => a.usdt,
        }
    }

    /// Run the call on rc3's state machine over the chain's current accounts.
    fn precheck(&mut self, p: &Programs, tx: &PublicTransaction, ids: &[AccountId]) -> Pre {
        let mut accounts = Vec::new();
        for id in ids {
            if let Some(a) = self.rpc.account(*id) {
                accounts.push((*id, a));
            }
        }
        let state = V03State::new()
            .with_named_programs([(p.token, program("token.bin")), (p.sextant_lc, program("sextant_lc.bin")), (p.wrap_bridge, program("wrap_bridge.bin"))])
            .with_public_accounts(accounts);
        let block = self.rpc.last_block() + 1;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let (charge, r) = ValidatedStateDiff::from_public_transaction_metered(tx, &state, block, now, GAS_CAP);
        match r {
            Err(e) => Pre::Refused(format!("not includable: {e}")),
            Ok(_) => match charge.revert {
                None => Pre::Accepted(charge.cycles),
                Some(e) => Pre::Refused(e),
            },
        }
    }

    /// Pre-check, send (unless refused and not forced), wait, record.
    #[allow(clippy::too_many_arguments)]
    fn send(&mut self, p: &Programs, what: &str, to: Actor, actors: &[Actor], signers: &[&Key], pdas: &[(AccountId, PdaSeed)], message: &impl BorshSerialize) -> bool {
        let label = self.label.clone().unwrap_or_else(|| what.to_string());
        let local = seq::transaction(&mut self.rpc, to, actors, signers, pdas, message, false);
        let mut ids: Vec<AccountId> = actors.iter().chain(std::iter::once(&to)).map(|a| a.account_id).collect();
        ids.extend(signers.iter().map(|k| k.id));
        ids.push(seq::payer().id);
        ids.sort();
        ids.dedup();
        let pre = self.precheck(p, &local, &ids);
        let (expect, pre_text) = match &pre {
            Pre::Accepted(c) => (true, format!("accepted ({c} cycles)")),
            Pre::Refused(e) => (false, format!("refused: {}", typed(e))),
        };
        println!("{label}: pre-check {pre_text}");
        if !expect && !self.force {
            fail(format!("{label}: the programs refuse it; not sent (--force sends it anyway)"));
        }
        let tx = seq::transaction(&mut self.rpc, to, actors, signers, pdas, message, true);
        let l = seq::submit(&mut self.rpc, &label, tx).unwrap_or_else(|e| fail(e));
        let verdict = if l.reverted { "reverted" } else { "applied" };
        println!("{label}: block {} tx {} {verdict} ({})", l.block, l.index, l.hash);
        let code = match &pre {
            Pre::Accepted(_) => "-".to_string(),
            Pre::Refused(e) => refusal(e).map_or_else(|| typed(e), |c| format!("E{c}")),
        };
        let line = format!("{label}\t{}\t{verdict}\t{}\t{}\t{}\t{code}\n", if expect { "applied" } else { "reverted" }, l.block, l.index, l.hash);
        use std::io::Write as _;
        std::fs::OpenOptions::new().create(true).append(true).open(self.run.join("manifest.tsv")).and_then(|mut f| f.write_all(line.as_bytes())).unwrap_or_else(|e| fail(e));
        if l.reverted == expect {
            fail(format!("{label}: the chain {verdict} what the pre-check {}", if expect { "accepted" } else { "refused" }));
        }
        !l.reverted
    }
}

/// The refusal's first line, from its typed code if it has one.
fn typed(e: &str) -> String {
    match refusal(e) {
        Some(code) => {
            let i = e.find(&format!("E{code}")).unwrap_or(0);
            e[i..].chars().take_while(|c| *c != '\n' && *c != '"').collect()
        }
        None => e.lines().last().unwrap_or(e).chars().take(200).collect(),
    }
}

fn plan_entry(pairing: &H256, (who, n, tok, amount): (Who, u64, Tok, u128)) -> Value {
    let c = wrap_core::commitment(pairing, who.key().id.value(), &secret(&who.seed(), n));
    json!({"from": who.name(), "token": match tok { Tok::Eth => "eth", Tok::Usdt => "usdt" }, "amount": amount.to_string(), "commitment": hx(&c), "n": n})
}

fn main() {
    let cli = Cli::parse();
    std::fs::create_dir_all(&cli.run).unwrap_or_else(|e| fail(e));
    let mut ctx = Ctx { rpc: Rpc::new(&cli.rpc), run: cli.run, force: cli.force, label: cli.label };
    match cli.cmd {
        Cmd::Pubkey => println!("{}", hex::encode(seq::payer().pk.value())),
        Cmd::ImageId => {
            for n in ["token", "sextant_lc", "wrap_bridge", "lez_chain_v04"] {
                let b = bin(&format!("{n}.bin"));
                println!("{n} {} bytes, ImageID {}", b.len(), hex::encode(image_id(&format!("{n}.bin"))));
            }
        }
        Cmd::Deploy => {
            let mut ids = Vec::new();
            let mut log = String::new();
            for (name, seed) in [("token", 2000u16), ("sextant_lc", 2100), ("wrap_bridge", 2200)] {
                let (header, steps) = seq::deploy_steps(name, &bin(&format!("{name}.bin")), seed);
                for (label, to, key, m) in steps {
                    let tx = seq::transaction(&mut ctx.rpc, to, &[], &[&key], &[], &m, true);
                    let l = seq::submit(&mut ctx.rpc, &label, tx).unwrap_or_else(|e| fail(e));
                    if l.reverted {
                        fail(format!("{label}: reverted in block {}", l.block));
                    }
                    println!("{label}: block {} tx {} applied", l.block, l.index);
                    log.push_str(&format!("{label}\tapplied\tapplied\t{}\t{}\t{}\t-\n", l.block, l.index, l.hash));
                }
                ids.push(header);
            }
            let p = Programs { token: ids[0], sextant_lc: ids[1], wrap_bridge: ids[2] };
            std::fs::write(ctx.run.join("programs.json"), serde_json::to_string(&p).unwrap()).unwrap();
            std::fs::write(ctx.run.join("deploy.tsv"), log).unwrap();
            println!("token {}\nsextant_lc {}\nwrap_bridge {}", p.token, p.sextant_lc, p.wrap_bridge);
        }
        Cmd::Plan => {
            let p = ctx.programs();
            let pairing = wrap_core::pairing(&LEZ_CHAIN_ID, &ZONE_ID, p.wrap_bridge.value(), &DEPLOYMENT_ID);
            let phantom = wrap_core::commitment(&pairing, alice().id.value(), &secret(&ALICE_SEED, PHANTOM_N));
            let deposits: Vec<Value> = PLAN.iter().map(|d| plan_entry(&pairing, *d)).collect();
            std::fs::write(ctx.run.join("plan.json"), json!({"pairing": hx(&pairing), "phantom": hx(&phantom), "deposits": deposits}).to_string()).unwrap();
            println!("pairing {}", hx(&pairing));
        }
        Cmd::Tip => println!("{}", ctx.rpc.last_block()),
        Cmd::InitSextant => {
            let p = ctx.programs();
            let a = ctx.anvil();
            // The devnet's slot starts now, so the sequencer's clock accepts the checkpoint.
            let now_s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
            let dn = devnet_at(&a.block, SLOT, now_s - 60);
            let sa = p.sextant_lc;
            let inst = sextant_program_v04::instance_account(&sa, &INSTANCE);
            let hdr = sextant_program_v04::header_account(&sa, &INSTANCE, SLOT);
            ctx.send(&p, "Sextant Init (devnet header of the anvil block)", Actor::new(inst, sa), &[Actor::new(hdr, sa)], &[], &[(sa, sextant_program_v04::instance_seed(&INSTANCE))], &init_message(&dn));
        }
        Cmd::InitBridge => {
            let p = ctx.programs();
            let a = ctx.anvil();
            let cfg = ctx.config(&p, a.vault);
            let me = p.wrap_bridge;
            if wrap_bridge_v04::pairing(&me, &cfg) != a.pairing {
                fail("the vault's pairing is not this deployment's");
            }
            let (ca, seed) = wrap_bridge_v04::config_pda(&me, &cfg);
            let burns = wrap_bridge_v04::burns_account(&me, &ca);
            ctx.send(&p, "Init", Actor::new(ca, me), &[Actor::new(burns, me)], &[], &[(me, seed)], &Bm::Init { config: cfg });
        }
        Cmd::Register { token } => {
            let p = ctx.programs();
            let a = ctx.anvil();
            let cfg = ctx.config(&p, a.vault);
            let (me, tp) = (p.wrap_bridge, p.token);
            let t = ctx.token(&a, token);
            let ca = wrap_bridge_v04::config_account(&me, &cfg);
            let entry = wrap_bridge_v04::entry_account(&me, &ca, &t);
            let def = wrap_bridge_v04::definition_account(&me, &ca, &t, &SALT).0;
            let (name, policy) = match token {
                Tok::Eth => ("Wrapped Ether (anvil)", eth_policy()),
                Tok::Usdt => ("Wrapped USDT (anvil)", usdt_policy()),
            };
            let m = Bm::Register { config: cfg, eth_token: t, name: name.into(), salt: SALT, policy };
            let actors = [Actor::new(ca, me), Actor::new(entry, me), Actor::new(def, tp), Actor::new(entry, tp)];
            ctx.send(&p, &format!("Register {token:?}").to_uppercase(), Actor::new(admin().id, me), &actors, &[&admin()], &[], &m);
        }
        Cmd::OpenHolding { owner, token } => {
            let p = ctx.programs();
            let a = ctx.anvil();
            let cfg = ctx.config(&p, a.vault);
            let t = ctx.token(&a, token);
            let ca = wrap_bridge_v04::config_account(&p.wrap_bridge, &cfg);
            let def = wrap_bridge_v04::definition_account(&p.wrap_bridge, &ca, &t, &SALT).0;
            let k = owner.key();
            let m = wrap_bridge_v04::token::Message::InitializeHolding { definition: def };
            ctx.send(&p, &format!("{} InitializeHolding {token:?}", owner.name()), Actor::new(k.id, p.token), &[Actor::new(def, p.token)], &[&k], &[], &m);
        }
        Cmd::Claim { index, root_after, redirect_to, as_token, phantom, private } => {
            if private {
                fail("a private claim is not available on LEZ v0.4 yet: its fee is paid inside the proof by a funded private account, and the bridge's private paths are not ported (docs/V04-PORT.md, section 2.6). Claim publicly (the default).");
            }
            let p = ctx.programs();
            let a = ctx.anvil();
            let cfg = ctx.config(&p, a.vault);
            let n = root_after.unwrap_or(a.deposits.len());
            let mut c = if phantom {
                // The deposit alice never made: its leaf alone is a tree the vault never had.
                let leaf = h32(&serde_json::from_slice::<Value>(&std::fs::read(ctx.run.join("anvil/proof-unknown.json")).unwrap()).unwrap()["leaf"]);
                let mut c = claim_message(&a, &cfg, &a.deposits[0], n, alice().id, secret(&ALICE_SEED, PHANTOM_N), SLOT);
                c.eth_token = ETH;
                c.amount = 1_000_000_000_000_000_000;
                c.index = 0;
                c.path = wrap_core::path(&[leaf], 0);
                c.root = a.unknown.0;
                c.proof = a.unknown.1.clone();
                c
            } else {
                let index = index.unwrap_or_else(|| fail("which deposit?"));
                let (who, k, _, _) = *PLAN.get(index).unwrap_or_else(|| fail(format!("no deposit {index} in the plan")));
                let d = a.deposits.get(index).unwrap_or_else(|| fail(format!("no deposit {index} on anvil")));
                if n <= index {
                    fail(format!("the tree after {n} deposits does not hold deposit {index}"));
                }
                claim_message(&a, &cfg, d, n, who.key().id, secret(&who.seed(), k), SLOT)
            };
            if let Some(r) = redirect_to {
                c.destination = r.key().id;
            }
            if let Some(t) = as_token {
                c.eth_token = ctx.token(&a, t);
            }
            let (to, actors, pda) = claim_actors_with(&p.wrap_bridge, &cfg, &c);
            let what = if phantom { "Claim the deposit never made (a root the vault never had)".to_string() } else { format!("Claim deposit {}", index.unwrap_or_default()) };
            ctx.send(&p, &what, to, &actors, &[], &[pda], &Bm::Claim(Box::new(c)));
        }
        Cmd::Burn { owner, token, amount, fee } => {
            let p = ctx.programs();
            let a = ctx.anvil();
            let cfg = ctx.config(&p, a.vault);
            let (me, tp) = (p.wrap_bridge, p.token);
            let t = ctx.token(&a, token);
            let ca = wrap_bridge_v04::config_account(&me, &cfg);
            let entry = wrap_bridge_v04::entry_account(&me, &ca, &t);
            let def = wrap_bridge_v04::definition_account(&me, &ca, &t, &SALT).0;
            let burns = wrap_bridge_v04::burns_account(&me, &ca);
            let recipient: [u8; 20] = h(&ctx.read("anvil/deployment.json")[owner.name()]);
            let mut record: Vec<Value> = if ctx.run.join("burns.json").exists() { ctx.read("burns.json").as_array().cloned().unwrap_or_default() } else { vec![] };
            // A fresh blinding per attempt by this owner.
            let attempts = std::fs::read_to_string(ctx.run.join("manifest.tsv")).unwrap_or_default().lines().filter(|l| l.starts_with(&format!("{} Burn", owner.name()))).count() as u64;
            let blinding = wrap_core::derive_secret(&owner.seed(), b'b', attempts);
            let ec = wrap_core::eth_commitment(&recipient, fee, &blinding);
            let leaf = wrap_core::burn_leaf(&a.pairing, &t, amount, &ec);
            let before: wrap_core::Frontier = {
                let s = ctx.rpc.actor_state(Actor::new(burns, me));
                if s.is_empty() { fail("no burn tree: init-bridge first") } else { borsh::from_slice(&s).unwrap() }
            };
            let k = owner.key();
            let m = Bm::Burn { eth_token: t, amount, eth_commitment: ec, salt: SALT, holder: k.id, config: cfg };
            let actors = [Actor::new(ca, me), Actor::new(entry, me), Actor::new(k.id, tp), Actor::new(entry, tp), Actor::new(def, tp)];
            let applied = ctx.send(&p, &format!("{} Burn {amount} {token:?}", owner.name()), Actor::new(burns, me), &actors, &[&k], &[], &m);
            if applied {
                record.push(json!({"owner": owner.name(), "token": hx(&t), "amount": amount.to_string(), "recipient": hx(&recipient), "fee": fee.to_string(),
                    "blinding": hx(&blinding), "eth_commitment": hx(&ec), "leaf": hx(&leaf), "index": before.next_index}));
                std::fs::write(ctx.run.join("burns.json"), serde_json::to_vec_pretty(&record).unwrap()).unwrap();
            }
        }
        Cmd::Balance { owner, token } => {
            let p = ctx.programs();
            let a = ctx.anvil();
            let cfg = ctx.config(&p, a.vault);
            let t = ctx.token(&a, token);
            let ca = wrap_bridge_v04::config_account(&p.wrap_bridge, &cfg);
            let def = wrap_bridge_v04::definition_account(&p.wrap_bridge, &ca, &t, &SALT).0;
            let st = |s: Vec<u8>| -> Option<wrap_bridge_v04::token::TokenState> { (!s.is_empty()).then(|| borsh::from_slice(&s).expect("a token state")) };
            let bal = match st(ctx.rpc.actor_state(Actor::new(owner.key().id, p.token))) {
                Some(wrap_bridge_v04::token::TokenState::Holding(h)) if h.definition_id == def => h.balance,
                _ => 0,
            };
            let supply = match st(ctx.rpc.actor_state(Actor::new(def, p.token))) {
                Some(wrap_bridge_v04::token::TokenState::Definition(d)) => d.total_supply,
                _ => 0,
            };
            println!("{}", json!({"owner": owner.name(), "token": format!("{token:?}").to_lowercase(), "balance": bal.to_string(), "supply": supply.to_string()}));
        }
        Cmd::ReleaseProof { owner, n } => {
            let p = ctx.programs();
            let a = ctx.anvil();
            let cfg = ctx.config(&p, a.vault);
            let ca = wrap_bridge_v04::config_account(&p.wrap_bridge, &cfg);
            let burns = wrap_bridge_v04::burns_account(&p.wrap_bridge, &ca);
            let record = ctx.read("burns.json");
            let all = record.as_array().cloned().unwrap_or_default();
            let leaves: Vec<H256> = all.iter().map(|b| h32(&b["leaf"])).collect();
            let chain: wrap_core::Frontier = borsh::from_slice(&ctx.rpc.actor_state(Actor::new(burns, p.wrap_bridge))).unwrap_or_else(|_| fail("no burn tree"));
            if chain.next_index != leaves.len() as u64 || chain.root != wrap_core::root(&leaves) {
                fail(format!("the recorded burns ({}) do not rebuild the chain's burn tree ({} leaves, root {})", leaves.len(), chain.next_index, hx(&chain.root)));
            }
            let (i, b) = all.iter().enumerate().filter(|(_, b)| b["owner"] == owner.name()).nth(n).unwrap_or_else(|| fail(format!("no burn {n} by {}", owner.name())));
            let nf = wrap_core::burn_nullifier(&a.pairing, &h32(&b["blinding"]), i as u64);
            let path: Vec<String> = wrap_core::path(&leaves, i).iter().map(|x| hx(x)).collect();
            if wrap_core::root_from_path(&leaves[i], i as u64, &wrap_core::path(&leaves, i)) != Some(chain.root) {
                fail("the path does not lead to the chain's root");
            }
            println!("{}", json!({"owner": owner.name(), "token": b["token"], "amount": b["amount"], "recipient": b["recipient"], "fee": b["fee"],
                "leaf": b["leaf"], "index": i, "path": path, "root": hx(&chain.root), "nullifier": hx(&nf), "vault": hx(&a.vault), "pairing": hx(&a.pairing),
                "secret": b["blinding"], "burns_account": burns.to_string(), "bridge_program": p.wrap_bridge.to_string(), "lez": "v0.4", "private": false}));
        }
        Cmd::Finish { from } => {
            let p = ctx.programs();
            let a = ctx.anvil();
            let cfg = ctx.config(&p, a.vault);
            let ca = wrap_bridge_v04::config_account(&p.wrap_bridge, &cfg);
            let burns = wrap_bridge_v04::burns_account(&p.wrap_bridge, &ca);
            let last = ctx.rpc.last_block();
            let dir = ctx.run.join("blocks");
            std::fs::create_dir_all(&dir).unwrap();
            for id in from..=last {
                let (raw, _) = ctx.rpc.block(id).unwrap_or_else(|| fail(format!("no block {id}")));
                std::fs::write(dir.join(format!("{id}.borsh")), &raw).unwrap();
            }
            let leaves: Vec<H256> = if ctx.run.join("burns.json").exists() { ctx.read("burns.json").as_array().unwrap().iter().map(|b| h32(&b["leaf"])).collect() } else { vec![] };
            let f: wrap_core::Frontier = borsh::from_slice(&ctx.rpc.actor_state(Actor::new(burns, p.wrap_bridge))).unwrap_or_else(|_| fail("no burn tree"));
            if (f.next_index, f.root) != (leaves.len() as u64, wrap_core::root(&leaves)) {
                fail("the program's burn tree is not the applied burns'");
            }
            let body = std::fs::read_to_string(ctx.run.join("manifest.tsv")).unwrap_or_default();
            let mut m = vec!["# label\texpected\tverdict\tblock\ttx\thash\tprecheck".to_string()];
            m.extend(body.lines().filter(|l| !l.starts_with('#')).map(str::to_string));
            m.push(format!("# programs token {} sextant_lc {} wrap_bridge {}", p.token, p.sextant_lc, p.wrap_bridge));
            m.push(format!("# config {ca} burns {burns} pairing {}", hx(&a.pairing)));
            m.push(format!("# blocks {from}..={last}"));
            m.push(format!("# burn leaves (applied burns, in order) {}", leaves.iter().map(|l| hx(l)).collect::<Vec<_>>().join(" ")));
            m.push(format!("# burn tree root {} (the program's own frontier, read back: {} leaves, root {})", hx(&wrap_core::root(&leaves)), f.next_index, hx(&f.root)));
            std::fs::write(ctx.run.join("manifest.tsv"), m.join("\n") + "\n").unwrap();
            println!("blocks {from}..={last}, {} applied burns, root {}", leaves.len(), hx(&f.root));
        }
    }
}

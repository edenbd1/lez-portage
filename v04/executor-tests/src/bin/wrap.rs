//! `wrap` on LEZ v0.4.0-rc3: the client of the v0.4 port, against a LEZ
//! v0.4.0-rc3 sequencer (v04/scripts/e2e-seq.sh runs a local standalone one).
//! The v0.3 client (cli/, `wrap` on LEZ v0.3) drives the same steps on v0.3.
//!
//! It loads the four committed programs (v04/artifacts: token, ata,
//! sextant_lc, wrap_bridge) through the program loader, writes the deposit plan the vault
//! is deployed and funded with (scripts/anvil_deposits.py), initialises
//! Sextant's light client on a devnet beacon header around the anvil block,
//! initialises the bridge, registers ETH and the ERC-20 (a USDT-shaped token),
//! opens ATA holdings (a claim also creates its destination's), sends claims
//! and burns, reads balances, and gives a burn's
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
//! **Claims are public by default; `claim --private` is the private claim**
//! (src/private.rs): one privacy-preserving transaction whose root call,
//! `ClaimPrivate`, runs privately at the destination's own wrap_bridge actor
//! (the opening never leaves the proof), whose public part names only a
//! blinded alias of the destination, and whose fee is paid inside the proof
//! by a funded private account (on the local network, one funded at genesis).
//! It is pre-checked by simulating it (`simulate_privacy_preserving_transaction`),
//! proven here, and measured (private-claim.json). Only a deposit committed
//! to a private account (carol's) can be claimed privately.
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
/// Holdings are ATA holdings, so alice receives wrapped ETH and wrapped USDT
/// in the same account. Deposit 4 is committed to carol's *private* account
/// (`private::carol`), claimed with `claim 4 --private`.
const PLAN: [(Who, u64, Tok, u128); 5] = [
    (Who::Alice, 0, Tok::Eth, 1_000_000_000_000_000_000),
    (Who::Alice, 1, Tok::Eth, 1_000_000_000_000_000_000),
    (Who::Bob, 0, Tok::Usdt, 100_000_000),
    (Who::Alice, 2, Tok::Usdt, 1_000_000_000),
    (Who::Carol, 0, Tok::Eth, 1_000_000_000_000_000_000),
];
/// Carol's wallet seed (her deposit secrets), test only.
const CAROL_SEED: H256 = *b"carol's wallet seed, test only!!";
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
    /// A private account (no public key): she only claims privately.
    Carol,
}

impl Who {
    fn key(self) -> Key {
        match self {
            Who::Alice => alice(),
            Who::Bob => bob(),
            Who::Carol => fail("carol is a private account: she has no public key"),
        }
    }
    fn id(self) -> AccountId {
        match self {
            Who::Carol => private::carol().account_id(),
            w => w.key().id,
        }
    }
    fn seed(self) -> H256 {
        match self {
            Who::Alice => ALICE_SEED,
            Who::Bob => BOB_SEED,
            Who::Carol => CAROL_SEED,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Who::Alice => "alice",
            Who::Bob => "bob",
            Who::Carol => "carol",
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
    /// Load token, ata, sextant_lc, wrap_bridge through the program loader (programs.json).
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
    /// Open `owner`'s ATA holding of the wrapped `token` (ATA `Create`,
    /// permissionless, relayed by the payer; a claim does it too).
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
        /// A privacy-preserving claim (carol's deposit 4): proven here, real
        /// proving unless RISC0_DEV_MODE=1, its fee paid in-proof by a private
        /// payer funded at genesis; measured into private-claim.json.
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
        /// Carol's private burn: a privacy-preserving transaction with a
        /// public root call, from her private holding (real proving unless
        /// RISC0_DEV_MODE=1), measured into private-burn.json.
        #[arg(long)]
        private: bool,
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
    ata: AccountId,
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
            ata_program: p.ata,
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
            .with_named_programs([
                (p.token, program("token.bin")),
                (p.ata, program("ata.bin")),
                (p.sextant_lc, program("sextant_lc.bin")),
                (p.wrap_bridge, program("wrap_bridge.bin")),
            ])
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

/// What a proven and settled privacy-preserving transaction measured.
struct PrivateRun {
    tx: lee::PrivacyPreservingTransaction,
    landed: seq::Landed,
    record: Value,
}

impl Ctx {
    /// The fee leg, the proof, the submission and the measurements of one
    /// privacy-preserving transaction: `input` without its fee; `payer` (one
    /// of the local network's genesis-funded private accounts) pays inside the
    /// proof from its pending genesis credit, under the fresh alias `fee_alias`.
    #[allow(clippy::too_many_arguments)]
    fn private_tx(
        &mut self,
        label: &str,
        mut input: lee::ProvingInput,
        catalog: &lee::privacy_preserving_transaction::circuit::ProgramCatalog,
        payer: &private::Keys,
        fee_alias: H256,
        present_private: impl Fn(Actor) -> Option<lee::SenderPresentation> + Clone,
        select: impl FnMut(lee_core::execution_state::Placement, &lee_core::program::MessageBody) -> bool + Clone,
        secrets: &[(&str, Vec<u8>)],
    ) -> PrivateRun {
        use lee::{SenderPresentation, Simulation};
        use lee_core::{DummyOutput, EphemeralSecretKey, FeeTransfer, MessageWitness, PrivateAccountKind, Recipient, RecipientEncryption};
        let inbox = Actor::native_balance(private::fee_inbox());
        input.public_context.actors.insert(inbox);
        let payer_id = payer.account_id();
        let (body, credit, path) = self.rpc.genesis_credit(payer_id);
        input.claims.push(MessageWitness { body, rho: None, membership_proof: path, filler: DummyOutput::default() });
        input.private_witnesses.push(payer.init_witness(&[]));
        let public: Vec<Actor> = input.public_context.actors.iter().copied().collect();
        let states = public.iter().map(|a| (*a, self.rpc.actor_view(*a))).collect();
        let sim = Simulation { public_actor_states: states, admitted_accounts: None };
        let present = move |a: Actor| {
            if a == Actor::native_balance(payer_id) {
                SenderPresentation::Blinded(fee_alias)
            } else {
                present_private(a).unwrap_or(SenderPresentation::Canonical)
            }
        };
        let public_cycles = lee::simulate_privacy_preserving_transaction(&input, &sim, catalog, present.clone(), select.clone())
            .unwrap_or_else(|e| fail(format!("{label}: the transaction does not simulate (the pre-check): {e}")));
        let (exec, stor) = self.rpc.fee_quote();
        let fee_at_head = fee_core::assess::private_fee_required(public_cycles, exec, stor);
        // Headroom for base fees rising while it proves: eight times the quoted
        // base fees and half again the cycles; never more than the payer holds.
        let reserve = fee_core::assess::private_fee_required(public_cycles + public_cycles / 2 + 100_000, exec * 8, stor * 8).min(credit).max(fee_at_head);
        println!("{label}: pre-check (simulation) accepted: public part {public_cycles} cycles; base fees exec {exec} stor {stor}; fee at head {fee_at_head}, reserve {reserve} of the payer's {credit}");
        input.recoveries.push(RecipientEncryption {
            recipient: Recipient { npk: payer.npk(), vpk: payer.vpk(), kind: PrivateAccountKind::Regular, opening: Some(fee_alias) },
            esk: EphemeralSecretKey(wrap_core::sha256(&[b"rfp022/v0.4/fee-esk", &fee_alias])),
        });
        input.fee = Some(FeeTransfer { payer: payer_id, recipient: private::fee_inbox(), amount: reserve, refund_to: None });
        let dev = std::env::var("RISC0_DEV_MODE").is_ok_and(|v| v == "1" || v == "true");
        println!("{label}: proving ({})", if dev { "RISC0_DEV_MODE=1: a fake receipt" } else { "real proving" });
        let t = std::time::Instant::now();
        let tx = private::prove(input, &sim, catalog, present, select).unwrap_or_else(|e| fail(format!("{label}: proving failed: {e}")));
        let secs = t.elapsed().as_secs_f64();
        let rss = private::peak_rss();
        let bytes = borsh::to_vec(&common::transaction::LeeTransaction::PrivacyPreserving(tx.clone())).unwrap().len();
        println!("{label}: proved in {secs:.1} s, peak resident {:.2} GB, {bytes} bytes", rss as f64 / 1e9);
        let raw = borsh::to_vec(&tx).unwrap();
        let leaked: Vec<&str> = secrets.iter().filter(|(_, v)| raw.windows(v.len()).any(|y| y == v.as_slice())).map(|(n, _)| *n).collect();
        if !leaked.is_empty() {
            fail(format!("{label}: the transaction carries the {leaked:?}"));
        }
        let l = seq::submit_private(&mut self.rpc, label, &tx).unwrap_or_else(|e| fail(e));
        let verdict = if l.reverted { "reverted" } else { "applied" };
        let (_, b) = self.rpc.block(l.block).expect("the block");
        let fees = seq::block_fees(&b).unwrap_or_default();
        println!("{label}: block {} tx {} {verdict} ({}); block fees exec {} stor {} revenue {}", l.block, l.index, l.hash, fees.gas_used_exec, fees.gas_used_stor, fees.revenue_base);
        let line = format!("{label}\tapplied\t{verdict}\t{}\t{}\t{}\t-\n", l.block, l.index, l.hash);
        use std::io::Write as _;
        std::fs::OpenOptions::new().create(true).append(true).open(self.run.join("manifest.tsv")).and_then(|mut f| f.write_all(line.as_bytes())).unwrap_or_else(|e| fail(e));
        let (_, proof) = tx.witness_set.clone().into_raw_parts();
        let inner: Option<risc0_zkvm::InnerReceipt> = borsh::from_slice(&proof.into_inner()).ok();
        let succinct = matches!(inner, Some(risc0_zkvm::InnerReceipt::Succinct(_)));
        let record = json!({
            "label": label, "verdict": verdict, "block": l.block, "tx": l.index, "hash": l.hash.to_string(), "dev_mode": dev,
            "receipt_succinct": succinct, "prove_s": (secs * 10.0).round() / 10.0, "peak_rss_bytes": rss, "tx_bytes": bytes,
            "public_cycles": public_cycles, "fee_at_head": fee_at_head.to_string(), "reserve": reserve.to_string(),
            "block_gas_exec": fees.gas_used_exec, "block_gas_stor": fees.gas_used_stor, "block_revenue_base": fees.revenue_base.to_string(),
            "not_published": secrets.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        });
        PrivateRun { tx, landed: l, record }
    }

    fn programs_private(p: &Programs) -> private::Programs {
        private::Programs { token: p.token, ata: p.ata, sextant_lc: p.sextant_lc, wrap_bridge: p.wrap_bridge }
    }

    /// Carol's private account as her wallet last decrypted it (carol.account).
    fn save_carol(&self, a: &lee_core::account::Account) {
        std::fs::write(self.run.join("carol.account"), borsh::to_vec(a).unwrap()).unwrap();
    }

    /// The private claim of `c` to carol's private account (private.rs).
    fn claim_private(&mut self, p: &Programs, cfg: &Config, c: &wrap_bridge_v04::Claim, label: &str) {
        let pp = Self::programs_private(p);
        let dest = private::carol();
        let dest_id = dest.account_id();
        if c.destination != dest_id {
            fail("the private claim's destination is not carol's private account");
        }
        let (root, msg, public) = private::claim_call(&pp, cfg, c);
        let alias = private::claim_alias(&c.secret);
        let input = private::claim_input(root, msg, &public, &dest, alias);
        let present = move |a: Actor| (a.account_id == dest_id).then_some(lee::SenderPresentation::Blinded(alias));
        let fee_alias = wrap_core::sha256(&[b"rfp022/v0.4/fee-alias/claim", &c.secret]);
        let payer = &private::fee_payers()[0];
        let run = self.private_tx(label, input, &pp.catalog(), payer, fee_alias, present, private::promote_alias(dest_id.blinded(&alias)),
            &[("destination", dest_id.value().to_vec()), ("secret", c.secret.to_vec())]);
        let held = private::private_balance(&dest, &run.tx, p.token);
        let ca = wrap_bridge_v04::config_account(&p.wrap_bridge, cfg);
        let def = wrap_bridge_v04::definition_account(&p.wrap_bridge, &ca, &c.eth_token, &c.salt).0;
        let nf = wrap_core::nullifier(&wrap_bridge_v04::pairing(&p.wrap_bridge, cfg), &c.secret, c.index);
        let spent = self.rpc.actor_state(Actor::new(wrap_bridge_v04::nullifier_account(&p.wrap_bridge, &ca, &nf), p.wrap_bridge)) == vec![1];
        let mut out = run.record;
        out["nullifier"] = json!(hx(&nf));
        out["nullifier_spent"] = json!(spent);
        out["destination_alias"] = json!(dest_id.blinded(&alias).to_string());
        out["carol_holding"] = json!(held.map(|(d, b)| json!({"definition": d.to_string(), "is_wrapped_eth": d == def, "balance": b.to_string()})));
        std::fs::write(self.run.join("private-claim.json"), serde_json::to_string_pretty(&out).unwrap()).unwrap();
        println!("{out}");
        if run.landed.reverted || !spent || held.map(|(d, b)| d == def && b == c.amount) != Some(true) {
            fail(format!("{label}: not settled as a claim of {} to carol", c.amount));
        }
        self.save_carol(&dest.decrypt(&run.tx).expect("carol's note"));
    }

    /// Carol's private burn: a public root call to the burn tree naming a
    /// fresh blinded alias of her as the holder; her own token actor runs
    /// privately (an update of her committed account) and moves the tokens
    /// into the entry's escrow, which burns them.
    fn burn_private(&mut self, p: &Programs, cfg: &Config, t: [u8; 20], amount: u128, ec: H256, blinding: &H256, label: &str) -> bool {
        let pp = Self::programs_private(p);
        let dest = private::carol();
        let id = dest.account_id();
        let state: lee_core::account::Account = borsh::from_slice(&std::fs::read(self.run.join("carol.account")).unwrap_or_else(|_| fail("carol has no private account yet: claim 4 --private first"))).unwrap();
        let commitment = lee_core::Commitment::new(&id, &state);
        let v = self.rpc.call("getProofsAndRoot", json!([[commitment], []])).unwrap_or_else(|e| fail(e));
        let proofs: Vec<Option<lee_core::MembershipProof>> = serde_json::from_value(v[0].clone()).unwrap_or_else(|e| fail(e));
        let proof = proofs.into_iter().next().flatten().unwrap_or_else(|| fail("carol's account is not committed"));
        let opening = private::burn_alias(blinding);
        let alias = id.blinded(&opening);
        let (root, msg, public) = private::burn_call(&pp, cfg, t, amount, ec, alias);
        let input = private::burn_input(root, msg, &public, dest.update_witness(state, proof, &[opening]));
        let present = move |a: Actor| (a.account_id == id).then_some(lee::SenderPresentation::Blinded(opening));
        let fee_alias = wrap_core::sha256(&[b"rfp022/v0.4/fee-alias/burn", blinding]);
        let payer = &private::fee_payers()[1];
        let run = self.private_tx(label, input, &pp.catalog(), payer, fee_alias, present, |_, _| false, &[("holder", id.value().to_vec())]);
        let held = private::private_balance(&dest, &run.tx, p.token);
        let mut out = run.record;
        out["holder_alias"] = json!(alias.to_string());
        out["carol_holding_after"] = json!(held.map(|(d, b)| json!({"definition": d.to_string(), "balance": b.to_string()})));
        std::fs::write(self.run.join("private-burn.json"), serde_json::to_string_pretty(&out).unwrap()).unwrap();
        println!("{out}");
        if !run.landed.reverted {
            self.save_carol(&dest.decrypt(&run.tx).expect("carol's note"));
        }
        !run.landed.reverted
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
    let c = wrap_core::commitment(pairing, who.id().value(), &secret(&who.seed(), n));
    json!({"from": who.name(), "token": match tok { Tok::Eth => "eth", Tok::Usdt => "usdt" }, "amount": amount.to_string(), "commitment": hx(&c), "n": n})
}

fn main() {
    let cli = Cli::parse();
    std::fs::create_dir_all(&cli.run).unwrap_or_else(|e| fail(e));
    let mut ctx = Ctx { rpc: Rpc::new(&cli.rpc), run: cli.run, force: cli.force, label: cli.label };
    match cli.cmd {
        Cmd::Pubkey => println!("{}", hex::encode(seq::payer().pk.value())),
        Cmd::ImageId => {
            for n in ["token", "ata", "sextant_lc", "wrap_bridge", "lez_chain_v04"] {
                let b = bin(&format!("{n}.bin"));
                println!("{n} {} bytes, ImageID {}", b.len(), hex::encode(image_id(&format!("{n}.bin"))));
            }
        }
        Cmd::Deploy => {
            let mut ids = Vec::new();
            let mut log = String::new();
            for (name, seed) in [("token", 2000u16), ("ata", 2050), ("sextant_lc", 2100), ("wrap_bridge", 2200)] {
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
            let p = Programs { token: ids[0], ata: ids[1], sextant_lc: ids[2], wrap_bridge: ids[3] };
            std::fs::write(ctx.run.join("programs.json"), serde_json::to_string(&p).unwrap()).unwrap();
            std::fs::write(ctx.run.join("deploy.tsv"), log).unwrap();
            println!("token {}\nata {}\nsextant_lc {}\nwrap_bridge {}", p.token, p.ata, p.sextant_lc, p.wrap_bridge);
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
            let h = wrap_bridge_v04::ata::holding(&p.ata, &p.token, &k.id, &def);
            let seed = wrap_bridge_v04::ata::seed(&p.token, &k.id, &def);
            let pdas: Vec<(AccountId, PdaSeed)> = if ctx.rpc.account(h).is_some() { vec![] } else { vec![(p.ata, seed)] };
            let m = wrap_bridge_v04::ata::Message::Create { token_program_id: p.token, owner: k.id, definition: def };
            ctx.send(&p, &format!("{} ATA Create {token:?}", owner.name()), Actor::new(h, p.ata), &[Actor::new(h, p.token), Actor::new(def, p.token)], &[], &pdas, &m);
        }
        Cmd::Claim { index, root_after, redirect_to, as_token, phantom, private } => {
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
                claim_message(&a, &cfg, d, n, who.id(), secret(&who.seed(), k), SLOT)
            };
            if let Some(r) = redirect_to {
                c.destination = r.id();
            }
            if let Some(t) = as_token {
                c.eth_token = ctx.token(&a, t);
            }
            if private {
                let index = index.unwrap_or_else(|| fail("which deposit?"));
                if PLAN[index].0 != Who::Carol {
                    fail(format!("deposit {index} is committed to a public account: only carol's (deposit 4) is claimed privately"));
                }
                ctx.claim_private(&p, &cfg, &c, &format!("Claim deposit {index} privately (carol)"));
                return;
            }
            let (to, actors, pda) = claim_actors_with(&p.wrap_bridge, &cfg, &c);
            // The destination's ATA holding, created by the claim if it is fresh.
            let ca = wrap_bridge_v04::config_account(&p.wrap_bridge, &cfg);
            let def = wrap_bridge_v04::definition_account(&p.wrap_bridge, &ca, &c.eth_token, &c.salt).0;
            let h = wrap_bridge_v04::ata::holding(&p.ata, &p.token, &c.destination, &def);
            let mut pdas = vec![pda];
            if ctx.rpc.account(h).is_none() {
                pdas.push((p.ata, wrap_bridge_v04::ata::seed(&p.token, &c.destination, &def)));
            }
            let what = if phantom { "Claim the deposit never made (a root the vault never had)".to_string() } else { format!("Claim deposit {}", index.unwrap_or_default()) };
            ctx.send(&p, &what, to, &actors, &[], &pdas, &Bm::Claim(Box::new(c)));
        }
        Cmd::Burn { owner, token, amount, fee, private } => {
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
            let label = format!("{} Burn {amount} {token:?}{}", owner.name(), if private { " privately" } else { "" });
            let applied = if private {
                if owner != Who::Carol {
                    fail("only carol, whose account is private, burns privately");
                }
                let label = ctx.label.clone().unwrap_or(label);
                ctx.burn_private(&p, &cfg, t, amount, ec, &blinding, &label)
            } else {
                let k = owner.key();
                let m = Bm::Burn { eth_token: t, amount, eth_commitment: ec, salt: SALT, holder: k.id, config: cfg, own_holding: false };
                let h = wrap_bridge_v04::ata::holding(&p.ata, &tp, &k.id, &def);
                let actors = [Actor::new(ca, me), Actor::new(entry, me), Actor::new(k.id, p.ata), Actor::new(h, tp), Actor::new(def, tp)];
                ctx.send(&p, &label, Actor::new(burns, me), &actors, &[&k], &[], &m)
            };
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
            let h = wrap_bridge_v04::ata::holding(&p.ata, &p.token, &owner.key().id, &def);
            let bal = match st(ctx.rpc.actor_state(Actor::new(h, p.token))) {
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
                "secret": b["blinding"], "burns_account": burns.to_string(), "bridge_program": p.wrap_bridge.to_string(), "lez": "v0.4", "private": owner == Who::Carol}));
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

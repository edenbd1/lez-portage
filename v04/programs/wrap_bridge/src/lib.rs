//! wrap_bridge on LEZ v0.4.0-rc1 (the actor model).
//!
//! The same bridge as `programs/wrap_bridge` (LEZ v0.3): Claim mints a wrapped
//! token against a deposit escrowed in `WrapVault` (a Merkle path to a vault
//! root, the root's `knownRoot` slot attested by Sextant, a nullifier spent),
//! Burn takes it out of circulation and appends a burn leaf. What changes is
//! the execution model (docs/V04-PORT.md):
//!
//! - v0.3 ran `plan` once over a list of account rows and `apply` once per
//!   effect. On v0.4 a run handles **one message to one actor** (an account
//!   paired with this program) and may change only that actor's state; every
//!   other change is a message to another actor, synchronous (same
//!   transaction, depth first; a refusal anywhere fails the transaction) or
//!   asynchronous.
//! - A program cannot read another program's state. Checks that used to be
//!   effects on other rows become messages to the actor that owns the state.
//! - Authority over a PDA is a seed presented on a message to it (the
//!   sender's program derives the address), and it travels down the sync
//!   chain that message starts.
//!
//! Actors of this program, one per account (all PDAs of this program):
//!
//! - **config** `sha256("config" ‖ borsh(Config))`: the global freeze flags.
//! - **burns** `sha256("burns" ‖ config)`: the burn tree's frontier, created by Init.
//! - **entry** `sha256("token" ‖ config ‖ eth_token)`: one registered token. The
//!   same account's *token* actor is the wrapped token's **mint authority** and
//!   its **burn escrow** (a holding of the wrapped token, empty at rest).
//! - **nullifier** `sha256("nullifier" ‖ config ‖ nf)`: written once per claimed deposit.
//!
//! and in the token program, the **definition** `sha256("definition" ‖ config ‖
//! eth_token ‖ salt)` (a PDA of this program, created by Register with zero supply).
//!
//! Message flow (`→` sync, `⇢` async; every actor is declared by the transaction):
//!
//! ```text
//! Init      root → config(Init)                     → burns(InitBurns)
//! Register  root → admin(Register) → config(CheckLive)
//!                                  → entry(Register) → token:definition(NewDefinition) → token:entry(InitHolding)
//! Claim     root → nullifier(Claim) → sextant:header(Attest)
//!                                   → config(CheckLive)
//!                                   → entry(Mint) → token:entry(Mint) → token:definition(IncreaseSupply) ⇢ token:destination(IncreaseBalance)
//! Burn      root → burns(Burn)      → config(CheckLive)
//!                                   → entry(Burn) → token:holder(Transfer) ⇢ token:entry(IncreaseBalance)
//!                                                 → token:entry(Burn) → token:definition(DecreaseSupply)
//! ```
//!
//! Burn is accepted **only as a transaction's root call** (`from == None`): its
//! leaf is then a function of the root message alone, which a block carries,
//! and the block's `reverted` list says whether it took effect. That is what
//! makes a public burn derivable from LEZ blocks on v0.4 (chain/v04).

pub mod token;

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ProgramEvent, ReceiveInput, Response, SyncMessage},
};
use sextant_core::mpt::AccountStorageProof;
use sextant_core::statement::{Cmp, Field, Predicate, Proof, Query};
use wrap_core::{Frontier, H256};

pub const E_ACCOUNTS: u32 = 7490;
pub const E_FROZEN: u32 = 7401;
pub const E_CLAIMED: u32 = 7402;
pub const E_PATH: u32 = 7403;
pub const E_PROOF: u32 = 7404;
pub const E_NOT_ADMIN: u32 = 7405;
pub const E_NOT_FREEZER: u32 = 7406;
pub const E_UNSUPPORTED: u32 = 7407;
pub const E_AMOUNT: u32 = 7408;
pub const E_CAP: u32 = 7409;
pub const E_EXISTS: u32 = 7410;
pub const E_NO_CONFIG: u32 = 7411;
pub const E_TREE_FULL: u32 = 7412;
pub const E_NOT_ROOT: u32 = 7414;
pub const E_DECODE: u32 = 7415;

#[track_caller]
pub fn refuse(code: u32, why: &str) -> ! {
    panic!("E{code}: {why}")
}

#[track_caller]
fn refuse_unless(ok: bool, code: u32, why: &str) {
    if !ok {
        refuse(code, why);
    }
}

/// What a deployment fixes forever; the config address is a hash of it.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Config {
    pub deployment_id: H256,
    pub lez_chain_id: H256,
    pub zone_id: H256,
    /// The Sextant deployment (its Attest half, ../sextant_attest) and instance.
    pub sextant: AccountId,
    pub instance_id: H256,
    pub chain_id: u64,
    pub vault: [u8; 20],
    /// The canonical token program (lez-programs). No ATA program: it has no
    /// actor-model port yet, so a claim mints to the committed account's own
    /// token actor (one token per account per token program, as on v0.3).
    pub token_program: AccountId,
    pub admin: AccountId,
    pub freezer: AccountId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Live {
    pub frozen_mint: bool,
    pub frozen_burn: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Policy {
    pub denominations: Vec<u128>,
    pub mint_cap: u128,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TokenEntry {
    pub salt: H256,
    pub definition: AccountId,
    pub policy: Policy,
    pub minted: u128,
    pub burned: u128,
    pub frozen_mint: bool,
    pub frozen_burn: bool,
}

/// What anyone may send (a transaction's root call, or another program).
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum Message {
    /// To the config PDA. Writes once, and creates the burn tree.
    Init { config: Config },
    /// To the admin's actor (authorized).
    Register { config: Config, eth_token: [u8; 20], name: String, salt: H256, policy: Policy },
    /// To the admin's actor (authorized). Counters and freeze flags carry over.
    SetPolicy { config: Config, eth_token: [u8; 20], policy: Policy },
    /// To the freezer's actor (authorized). `eth_token: None` is global.
    Freeze { config: Config, eth_token: Option<[u8; 20]>, mint: bool, burn: bool },
    /// To this deposit's nullifier PDA. Anyone (a relayer) may send it.
    Claim(Box<Claim>),
    /// To the burn tree PDA, as the transaction's root call only. `holder`
    /// authorizes (signs); its token actor must hold the wrapped token. The
    /// first three fields are the leaf's (chain/v04 reads them from the
    /// block without decoding the rest; pinned by a test).
    Burn { eth_token: [u8; 20], amount: u128, eth_commitment: H256, salt: H256, holder: AccountId, config: Config },
}

/// The borsh variant index of [`Message::Burn`].
pub const BURN_VARIANT: u8 = 5;

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct Claim {
    pub config: Config,
    pub eth_token: [u8; 20],
    pub salt: H256,
    pub amount: u128,
    pub slot: u64,
    pub root: H256,
    pub proof: AccountStorageProof,
    pub index: u64,
    pub path: Vec<H256>,
    /// The account the deposit committed to: its token actor receives the mint.
    pub destination: AccountId,
    pub secret: H256,
}

/// What only this program's own actors send each other (decoded only when
/// the sender is this program).
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum Internal {
    InitBurns,
    CheckLive { mint: bool },
    SetLive { mint: bool, burn: bool },
    Register { config: Config, eth_token: [u8; 20], name: String, salt: H256, policy: Policy },
    SetPolicy(Policy),
    FreezeToken { mint: bool, burn: bool },
    Mint { config: Config, eth_token: [u8; 20], salt: H256, amount: u128, to: AccountId },
    Burn { config: Config, eth_token: [u8; 20], salt: H256, amount: u128, holder: AccountId },
}

/// The Attest half of Sextant on v0.4 (`v04/programs/sextant_attest`): the
/// message its header actor accepts.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum AttestMessage {
    Attest { instance_id: H256, slot: u64, query: Query },
}

// ---- addresses ----

pub fn pda(me: &AccountId, parts: &[&[u8]]) -> (AccountId, PdaSeed) {
    let seed = PdaSeed::new(wrap_core::sha256(parts));
    (AccountId::for_public_pda(me, &seed), seed)
}

pub fn config_pda(me: &AccountId, c: &Config) -> (AccountId, PdaSeed) {
    pda(me, &[b"config", &borsh::to_vec(c).expect("infallible")])
}

pub fn config_account(me: &AccountId, c: &Config) -> AccountId {
    config_pda(me, c).0
}

pub fn entry_pda(me: &AccountId, config: &AccountId, eth_token: &[u8; 20]) -> (AccountId, PdaSeed) {
    pda(me, &[b"token", config.value(), eth_token])
}

pub fn entry_account(me: &AccountId, config: &AccountId, eth_token: &[u8; 20]) -> AccountId {
    entry_pda(me, config, eth_token).0
}

pub fn definition_account(me: &AccountId, config: &AccountId, eth_token: &[u8; 20], salt: &H256) -> (AccountId, PdaSeed) {
    pda(me, &[b"definition", config.value(), eth_token, salt])
}

pub fn nullifier_pda(me: &AccountId, config: &AccountId, nf: &H256) -> (AccountId, PdaSeed) {
    pda(me, &[b"nullifier", config.value(), nf])
}

pub fn nullifier_account(me: &AccountId, config: &AccountId, nf: &H256) -> AccountId {
    nullifier_pda(me, config, nf).0
}

pub fn burns_pda(me: &AccountId, config: &AccountId) -> (AccountId, PdaSeed) {
    pda(me, &[b"burns", config.value()])
}

pub fn burns_account(me: &AccountId, config: &AccountId) -> AccountId {
    burns_pda(me, config).0
}

/// Sextant's header record PDA (lez-sextant `programs/sextant`, unchanged).
pub fn header_account(sextant: &AccountId, instance_id: &H256, slot: u64) -> AccountId {
    let seed = PdaSeed::new(wrap_core::sha256(&[b"header", instance_id, &slot.to_le_bytes()]));
    AccountId::for_public_pda(sextant, &seed)
}

pub fn pairing(me: &AccountId, c: &Config) -> H256 {
    wrap_core::pairing(&c.lez_chain_id, &c.zone_id, me.value(), &c.deployment_id)
}

pub fn query(c: &Config, proof: AccountStorageProof) -> Query {
    Query {
        chain_id: c.chain_id,
        proof: Proof::Storage(proof),
        predicates: vec![Predicate { field: Field::SlotValue, cmp: Cmp::Eq, value: wrap_core::word_u64(1) }],
    }
}

// ---- events ----

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct Claimed {
    pub nullifier: H256,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct Burned {
    pub leaf: H256,
}

pub fn event<T: BorshSerialize>(name: &str, body: &T) -> ProgramEvent {
    let sel = wrap_core::sha256(&[b"wrap_bridge::", name.as_bytes()]);
    ProgramEvent { selector: sel[..8].try_into().expect("8 bytes"), data: borsh::to_vec(body).expect("infallible") }
}

// ---- the handler ----

fn put<T: BorshSerialize>(t: &T) -> Vec<u8> {
    borsh::to_vec(t).expect("infallible")
}

fn decode<T: BorshDeserialize>(b: &[u8], what: &str) -> T {
    borsh::from_slice(b).unwrap_or_else(|_| refuse(E_DECODE, what))
}

fn own(input: &ReceiveInput, account_id: AccountId) -> Actor {
    Actor::new(account_id, input.receiver.program_account_id)
}

fn expect_receiver(input: &ReceiveInput, id: AccountId, what: &str) {
    refuse_unless(input.receiver.account_id == id, E_ACCOUNTS, what);
}

fn check_policy(p: &Policy) {
    refuse_unless(!p.denominations.is_empty() && p.denominations.iter().all(|d| *d > 0), E_AMOUNT, "a policy needs at least one positive amount");
}

/// The guest's entry point.
pub fn handle_message(input: &ReceiveInput) -> Response {
    if input.from_own_program() {
        internal(input, decode(&input.message, "not a wrap_bridge internal message"))
    } else {
        external(input, decode(&input.message, "not a wrap_bridge message"))
    }
}

fn external(input: &ReceiveInput, m: Message) -> Response {
    let me = input.receiver.program_account_id;
    match m {
        Message::Init { config } => {
            let (ca, _) = config_pda(&me, &config);
            expect_receiver(input, ca, "Init goes to the config PDA");
            refuse_unless(input.pre_state.is_empty(), E_EXISTS, "this configuration is already initialised");
            let (burns, burns_seed) = burns_pda(&me, &ca);
            Response::set_state(put(&Live::default())).send(SyncMessage::new(own(input, burns), &Internal::InitBurns).with_pda_seeds([burns_seed]))
        }
        Message::Register { config, eth_token, name, salt, policy } => {
            refuse_unless(input.receiver.account_id == config.admin && input.is_authorized, E_NOT_ADMIN, "only the admin authority registers a token");
            check_policy(&policy);
            let ca = config_account(&me, &config);
            let (entry, entry_seed) = entry_pda(&me, &ca, &eth_token);
            Response::keep_state()
                .send_sync(own(input, ca), &Internal::CheckLive { mint: false })
                .send(SyncMessage::new(own(input, entry), &Internal::Register { config, eth_token, name, salt, policy }).with_pda_seeds([entry_seed]))
        }
        Message::SetPolicy { config, eth_token, policy } => {
            refuse_unless(input.receiver.account_id == config.admin && input.is_authorized, E_NOT_ADMIN, "only the admin authority sets a policy");
            check_policy(&policy);
            let ca = config_account(&me, &config);
            Response::keep_state().send_sync(own(input, entry_account(&me, &ca, &eth_token)), &Internal::SetPolicy(policy))
        }
        Message::Freeze { config, eth_token, mint, burn } => {
            refuse_unless(input.receiver.account_id == config.freezer && input.is_authorized, E_NOT_FREEZER, "only the freeze authority freezes");
            let ca = config_account(&me, &config);
            match eth_token {
                None => Response::keep_state().send_sync(own(input, ca), &Internal::SetLive { mint, burn }),
                Some(t) => Response::keep_state().send_sync(own(input, entry_account(&me, &ca, &t)), &Internal::FreezeToken { mint, burn }),
            }
        }
        Message::Claim(c) => {
            let Claim { config, eth_token, salt, amount, slot, root, proof, index, path, destination, secret } = *c;
            refuse_unless(amount > 0, E_AMOUNT, "nothing to claim");
            let ca = config_account(&me, &config);
            let pair = pairing(&me, &config);
            let commitment = wrap_core::commitment(&pair, destination.value(), &secret);
            let leaf = wrap_core::leaf(config.chain_id, &config.vault, &pair, &eth_token, amount, &commitment);
            refuse_unless(wrap_core::root_from_path(&leaf, index, &path) == Some(root), E_PATH, "no such deposit under this root: check the token, amount, destination, secret and path");
            refuse_unless(proof.address == config.vault, E_PROOF, "the storage proof is not the configured vault's");
            refuse_unless(proof.slot == wrap_core::known_root_slot(&root), E_PROOF, "the storage proof is not knownRoot[root]");
            let nf = wrap_core::nullifier(&pair, &secret, index);
            expect_receiver(input, nullifier_account(&me, &ca, &nf), "Claim goes to this deposit's nullifier PDA");
            refuse_unless(input.pre_state.is_empty(), E_CLAIMED, "this deposit was already claimed");
            let header = Actor::new(header_account(&config.sextant, &config.instance_id, slot), config.sextant);
            let attest = AttestMessage::Attest { instance_id: config.instance_id, slot, query: query(&config, proof) };
            let entry = entry_account(&me, &ca, &eth_token);
            Response::set_state(vec![1])
                .send_sync(header, &attest)
                .send_sync(own(input, ca), &Internal::CheckLive { mint: true })
                .send_sync(own(input, entry), &Internal::Mint { config, eth_token, salt, amount, to: destination })
                .event(event("Claimed", &Claimed { nullifier: nf }))
        }
        Message::Burn { eth_token, amount, eth_commitment, salt, holder, config } => {
            refuse_unless(input.from.is_none(), E_NOT_ROOT, "Burn is accepted only as a transaction's root call");
            let ca = config_account(&me, &config);
            expect_receiver(input, burns_account(&me, &ca), "Burn goes to the burn tree PDA");
            refuse_unless(!input.pre_state.is_empty(), E_NO_CONFIG, "no burn tree: send Init first");
            let leaf = wrap_core::burn_leaf(&pairing(&me, &config), &eth_token, amount, &eth_commitment);
            let mut f: Frontier = decode(&input.pre_state, "a frontier");
            f.insert(&leaf).unwrap_or_else(|| refuse(E_TREE_FULL, "the burn tree is full: migrate to a new deployment"));
            let entry = entry_account(&me, &ca, &eth_token);
            Response::set_state(put(&f))
                .send_sync(own(input, ca), &Internal::CheckLive { mint: false })
                .send_sync(own(input, entry), &Internal::Burn { config, eth_token, salt, amount, holder })
                .event(event("Burned", &Burned { leaf }))
        }
    }
}

fn entry_of(pre: &[u8], salt: &H256) -> TokenEntry {
    refuse_unless(!pre.is_empty(), E_UNSUPPORTED, "this token is not registered in this configuration");
    let e: TokenEntry = decode(pre, "a token entry");
    refuse_unless(&e.salt == salt, E_UNSUPPORTED, "this is not the registered wrapped token: re-read its salt");
    e
}

fn internal(input: &ReceiveInput, m: Internal) -> Response {
    let me = input.receiver.program_account_id;
    let pre = &input.pre_state;
    match m {
        Internal::InitBurns => {
            refuse_unless(pre.is_empty(), E_EXISTS, "the burn tree exists");
            Response::set_state(put(&Frontier::default()))
        }
        Internal::CheckLive { mint } => {
            refuse_unless(!pre.is_empty(), E_NO_CONFIG, "no such configuration: send Init first");
            let l: Live = decode(pre, "live flags");
            refuse_unless(!(if mint { l.frozen_mint } else { l.frozen_burn }), E_FROZEN, "the bridge is frozen in this direction");
            Response::keep_state()
        }
        Internal::SetLive { mint, burn } => {
            refuse_unless(!pre.is_empty(), E_NO_CONFIG, "no such configuration: send Init first");
            Response::set_state(put(&Live { frozen_mint: mint, frozen_burn: burn }))
        }
        Internal::Register { config, eth_token, name, salt, policy } => {
            refuse_unless(pre.is_empty(), E_EXISTS, "this token is already registered: use SetPolicy");
            let ca = config_account(&me, &config);
            let entry = input.receiver.account_id;
            let (did, dseed) = definition_account(&me, &ca, &eth_token, &salt);
            let e = TokenEntry { salt, definition: did, policy, minted: 0, burned: 0, frozen_mint: false, frozen_burn: false };
            // The entry's own token actor is the mint authority and the
            // initial (zero) holding: the entry's seed grant, inherited down
            // this sync chain, authorizes its initialization.
            let new_def = token::Message::NewDefinition { name, total_supply: 0, mint_authority: Some(entry), holding: entry };
            Response::set_state(put(&e)).send(SyncMessage::new(Actor::new(did, config.token_program), &new_def).with_pda_seeds([dseed]))
        }
        Internal::SetPolicy(p) => {
            refuse_unless(!pre.is_empty(), E_UNSUPPORTED, "this token is not registered in this configuration");
            let mut e: TokenEntry = decode(pre, "a token entry");
            e.policy = p;
            Response::set_state(put(&e))
        }
        Internal::FreezeToken { mint, burn } => {
            refuse_unless(!pre.is_empty(), E_UNSUPPORTED, "this token is not registered in this configuration");
            let mut e: TokenEntry = decode(pre, "a token entry");
            e.frozen_mint = mint;
            e.frozen_burn = burn;
            Response::set_state(put(&e))
        }
        Internal::Mint { config, eth_token, salt, amount, to } => {
            let ca = config_account(&me, &config);
            let (entry, seed) = entry_pda(&me, &ca, &eth_token);
            expect_receiver(input, entry, "Mint goes to the token's entry");
            let mut e = entry_of(pre, &salt);
            refuse_unless(!e.frozen_mint, E_FROZEN, "minting of this token is frozen");
            let outstanding = e.minted - e.burned;
            refuse_unless(outstanding.checked_add(amount).is_some_and(|o| o <= e.policy.mint_cap), E_CAP, "the token's supply cap is reached: retry later");
            e.minted += amount;
            let mint = token::Message::Mint { definition: e.definition, to, amount };
            Response::set_state(put(&e)).send(SyncMessage::new(Actor::new(entry, config.token_program), &mint).with_pda_seeds([seed]))
        }
        Internal::Burn { config, eth_token, salt, amount, holder } => {
            let ca = config_account(&me, &config);
            let (entry, seed) = entry_pda(&me, &ca, &eth_token);
            expect_receiver(input, entry, "Burn goes to the token's entry");
            let mut e = entry_of(pre, &salt);
            refuse_unless(!e.frozen_burn, E_FROZEN, "burning of this token is frozen");
            refuse_unless(e.policy.denominations.contains(&amount), E_AMOUNT, "burn one of the token's permitted amounts");
            e.burned += amount;
            let tp = config.token_program;
            // The holder's tokens move into the entry's escrow holding, which
            // refuses any other definition's tokens, then the escrow burns
            // them: the burn is of the wrapped token, by construction.
            Response::set_state(put(&e))
                .send_sync(Actor::new(holder, tp), &token::Message::Transfer { to: entry, amount })
                .send(SyncMessage::new(Actor::new(entry, tp), &token::Message::Burn { amount }).with_pda_seeds([seed]))
        }
    }
}

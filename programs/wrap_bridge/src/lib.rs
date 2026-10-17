//! wrap_bridge: mint a wrapped token on LEZ against a deposit escrowed in
//! `WrapVault` on Ethereum, and burn it to entitle a release.
//!
//! **Claim** (inbound). The claimant shows that `root` is a root of the vault's
//! deposit tree (`knownRoot[root] == 1`, a storage proof checked by Sextant's
//! `Attest` under a header the light client finalized, as a chained call), and
//! that the leaf `wrap_core::leaf(.., token, amount, commitment(destination,
//! secret))` sits at `index` under `root` (a Merkle path, checked here). It
//! spends `nullifier(secret, index)` and mints `amount` to `destination`'s
//! associated token account, or to `destination` itself (a private account),
//! through the token program. The destination is bound by the commitment, so a
//! relayer submitting the claim cannot redirect it (Security #3), and the
//! nullifier is unlinkable to the deposit without the secret.
//!
//! In a public transaction the instruction carries the opening and so links
//! the claim to its deposit. The private path runs the same plan inside a
//! privacy-preserving transaction to a private destination: the instruction,
//! the path and the destination stay in the proof's private inputs, and only
//! the deferred effects on public accounts are published (the root and its
//! storage proof in Sextant's check, the nullifier, the token and amount in
//! the counters and the supply). See docs/PRIVATE-CLAIM-MEASURED.md.
//!
//! **Burn** (outbound). The holder burns a permitted amount and appends
//! `burn_leaf(token, amount, eth_commitment)` to this program's burn tree; the
//! Ethereum recipient is hidden in `eth_commitment`. `Burn` takes the tokens
//! from the signing owner's ATA; `BurnFrom` from the authorized account that
//! holds them, which on the private path is a private account: run inside a
//! privacy-preserving transaction, the holder never appears, and only the
//! burn leaf, the token, the amount and the supply change are public.
//!
//! Accounts (all PDAs of this program, except the token program's shards):
//!
//! - **config** `sha256("config" ‖ borsh(Config))`: its address commits to the
//!   whole immutable configuration (the Sextant deployment and instance, the
//!   EVM chain and vault, the token program, the admin and freeze
//!   authorities); it holds only the global freeze flags.
//! - **entry** `sha256("token" ‖ config ‖ eth_token)`: one registered token, its
//!   definition salt, permitted amounts, cap and counters.
//! - **definition** `sha256("definition" ‖ config ‖ eth_token ‖ salt)`, in the
//!   token program's shard: the wrapped token, created by `Register`, minted
//!   only by this program presenting the seed.
//! - **nullifier** `sha256("nullifier" ‖ config ‖ nf)`: written once per claimed
//!   deposit.
//! - **burns** `sha256("burns" ‖ config)`: the burn tree's frontier.

pub mod token;

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, ProgramShardSelector},
    program::{AccountMeta, ChainedCall, PdaSeed, Plan, PlanInput, ProgramEvent},
};
use sextant_core::mpt::AccountStorageProof;
use sextant_core::statement::{Cmp, Field, Predicate, Proof, Query};
use sextant_program::header_account;
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
pub const E_NOT_HOLDER: u32 = 7413;

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

/// What a deployment fixes forever: its address is a hash of this (F.16: a
/// second configuration is a second set of accounts, with no access to these).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Config {
    pub deployment_id: H256,
    /// The LEZ chain and zone this deployment is on, as the vault pins them.
    pub lez_chain_id: H256,
    pub zone_id: H256,
    /// The Sextant (RFP-021) deployment and instance it trusts.
    pub sextant: AccountId,
    pub instance_id: H256,
    /// The EVM chain and vault it is paired with (F.15).
    pub chain_id: u64,
    pub vault: [u8; 20],
    /// The token program the wrapped tokens live in, and the associated-token-
    /// account program whose holdings they are minted to and burned from (one
    /// LEZ account holds one token, so each owner gets one ATA per token).
    pub token_program: AccountId,
    pub ata_program: AccountId,
    /// RFP-001 / RFP-002 stand-ins: one account each.
    pub admin: AccountId,
    pub freezer: AccountId,
}

/// The global freeze flags, in the config account.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Live {
    pub frozen_mint: bool,
    pub frozen_burn: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Policy {
    /// The amounts a burn may take (the vault enforces the same set on deposit).
    pub denominations: Vec<u128>,
    /// Ceiling on outstanding wrapped supply (minted minus burned).
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

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    /// Rows: [config]. Anyone; writes once.
    Init { config: Config },
    /// Rows: [config, entry, admin (signer), definition, supply]. Creates the
    /// wrapped token with zero supply; refused if its definition already exists
    /// (choose another salt).
    Register { config: Config, eth_token: [u8; 20], name: String, salt: H256, policy: Policy },
    /// Rows: [config, entry, admin (signer)]. Counters and freeze flags carry over.
    SetPolicy { config: Config, eth_token: [u8; 20], policy: Policy },
    /// Rows: [config or entry, freezer (signer)]. `eth_token: None` is global.
    Freeze { config: Config, eth_token: Option<[u8; 20]>, mint: bool, burn: bool },
    /// Rows: [header (Sextant's shard), config, entry, nullifier, definition,
    /// the destination owner's ATA for this token, or the destination itself].
    Claim(Box<Claim>),
    /// Rows: [config, entry, burns, definition, owner (signer), the owner's ATA].
    Burn { config: Config, eth_token: [u8; 20], salt: H256, amount: u128, eth_commitment: H256 },
    /// Rows: [config, entry, burns, definition, holding (authorized)]. Burns
    /// from an account that holds the wrapped token itself rather than
    /// through an ATA: on the private path, the private account a private
    /// claim minted into, authorized inside the privacy circuit by its
    /// owner's key, so that no public account appears in the burn.
    BurnFrom { config: Config, eth_token: [u8; 20], salt: H256, amount: u128, eth_commitment: H256 },
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct Claim {
    pub config: Config,
    pub eth_token: [u8; 20],
    pub salt: H256,
    pub amount: u128,
    /// The Sextant header record the storage proof is checked against: any
    /// finalized header after the deposit.
    pub slot: u64,
    /// A vault root containing the deposit, and its `knownRoot` storage proof.
    pub root: H256,
    pub proof: AccountStorageProof,
    pub index: u64,
    pub path: Vec<H256>,
    /// The account the deposit committed to; the tokens go to its ATA, or to
    /// it directly (a private account in a privacy-preserving claim).
    pub destination: AccountId,
    pub secret: H256,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum Effect {
    InitLive,
    /// The config exists and the given direction is not frozen globally.
    CheckLive { mint: bool },
    SetLive { mint: bool, burn: bool },
    Register(TokenEntry),
    SetPolicy(Policy),
    FreezeToken { mint: bool, burn: bool },
    Mint { salt: H256, amount: u128 },
    Burn { salt: H256, amount: u128 },
    Spend,
    Append(H256),
}

// ---- addresses ----

fn pda(me: &AccountId, parts: &[&[u8]]) -> (AccountId, PdaSeed) {
    let seed = PdaSeed::new(wrap_core::sha256(parts));
    (AccountId::for_public_pda(me, &seed), seed)
}

pub fn config_account(me: &AccountId, c: &Config) -> AccountId {
    pda(me, &[b"config", &borsh::to_vec(c).expect("infallible")]).0
}

pub fn entry_account(me: &AccountId, config: &AccountId, eth_token: &[u8; 20]) -> AccountId {
    pda(me, &[b"token", config.value(), eth_token]).0
}

pub fn definition_account(me: &AccountId, config: &AccountId, eth_token: &[u8; 20], salt: &H256) -> (AccountId, PdaSeed) {
    pda(me, &[b"definition", config.value(), eth_token, salt])
}

pub fn supply_account(me: &AccountId, config: &AccountId, eth_token: &[u8; 20], salt: &H256) -> AccountId {
    pda(me, &[b"supply", config.value(), eth_token, salt]).0
}

pub fn nullifier_account(me: &AccountId, config: &AccountId, nf: &H256) -> AccountId {
    pda(me, &[b"nullifier", config.value(), nf]).0
}

pub fn burns_account(me: &AccountId, config: &AccountId) -> AccountId {
    pda(me, &[b"burns", config.value()]).0
}

/// The pairing the vault was deployed with, for this program and config.
pub fn pairing(me: &AccountId, c: &Config) -> H256 {
    wrap_core::pairing(&c.lez_chain_id, &c.zone_id, me.value(), &c.deployment_id)
}

/// The query `Attest` must accept: the vault's `knownRoot[root]` slot holds 1.
pub fn query(c: &Config, proof: AccountStorageProof) -> Query {
    Query {
        chain_id: c.chain_id,
        proof: Proof::Storage(proof),
        predicates: vec![Predicate { field: Field::SlotValue, cmp: Cmp::Eq, value: wrap_core::word_u64(1) }],
    }
}

// ---- events (LP-0012) ----

/// A claim's event names the nullifier only: the token and amount are public
/// in the supply change anyway, and nothing in it points at a deposit.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct Claimed {
    pub nullifier: H256,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct Burned {
    pub leaf: H256,
}

fn event<T: BorshSerialize>(name: &str, body: &T) -> ProgramEvent {
    let sel = wrap_core::sha256(&[b"wrap_bridge::", name.as_bytes()]);
    ProgramEvent { selector: sel[..8].try_into().expect("8 bytes"), data: borsh::to_vec(body).expect("infallible") }
}

// ---- plan ----

fn rows<const N: usize>(input: &PlanInput) -> [AccountMeta; N] {
    <[AccountMeta; N]>::try_from(input.accounts.clone()).unwrap_or_else(|_| refuse(E_ACCOUNTS, "wrong number of accounts for this instruction"))
}

fn expect_row(row: &AccountMeta, id: AccountId, program: AccountId, what: &str) {
    refuse_unless(row.account_id == id && row.program_account_id == program, E_ACCOUNTS, what);
}

fn check_policy(p: &Policy) {
    refuse_unless(!p.denominations.is_empty() && p.denominations.iter().all(|d| *d > 0), E_AMOUNT, "a policy needs at least one positive amount");
}

#[must_use]
pub fn plan(input: &PlanInput, ix: Instruction) -> Plan {
    let me = input.self_account_id;
    let mut plan = Plan::new(input);
    match ix {
        Instruction::Init { config } => {
            let [cfg] = rows::<1>(input);
            expect_row(&cfg, config_account(&me, &config), me, "the row is the config PDA");
            plan.effect(&cfg, &Effect::InitLive);
        }
        Instruction::Register { config, eth_token, name, salt, policy } => {
            let [cfg, entry, admin, def, supply] = rows::<5>(input);
            let ca = config_account(&me, &config);
            expect_row(&cfg, ca, me, "the first row is the config PDA");
            expect_row(&entry, entry_account(&me, &ca, &eth_token), me, "the second row is the token's entry PDA");
            refuse_unless(admin.account_id == config.admin && admin.is_authorized, E_NOT_ADMIN, "only the admin authority registers a token");
            let (did, _) = definition_account(&me, &ca, &eth_token, &salt);
            expect_row(&def, did, config.token_program, "the fourth row is the wrapped token's definition, in the token program's shard");
            expect_row(&supply, supply_account(&me, &ca, &eth_token, &salt), config.token_program, "the fifth row is the supply holding");
            check_policy(&policy);
            plan.effect(&cfg, &Effect::CheckLive { mint: false });
            plan.effect(
                &entry,
                &Effect::Register(TokenEntry { salt, definition: did, policy, minted: 0, burned: 0, frozen_mint: false, frozen_burn: false }),
            );
            plan.call(ChainedCall::new(
                config.token_program,
                vec![ProgramShardSelector::from(&def), ProgramShardSelector::from(&supply)],
                &token::Instruction::NewFungibleDefinition { name, total_supply: 0 },
            ));
        }
        Instruction::SetPolicy { config, eth_token, policy } => {
            let [cfg, entry, admin] = rows::<3>(input);
            let ca = config_account(&me, &config);
            expect_row(&cfg, ca, me, "the first row is the config PDA");
            expect_row(&entry, entry_account(&me, &ca, &eth_token), me, "the second row is the token's entry PDA");
            refuse_unless(admin.account_id == config.admin && admin.is_authorized, E_NOT_ADMIN, "only the admin authority sets a policy");
            check_policy(&policy);
            plan.effect(&entry, &Effect::SetPolicy(policy));
        }
        Instruction::Freeze { config, eth_token, mint, burn } => {
            let [target, freezer] = rows::<2>(input);
            let ca = config_account(&me, &config);
            refuse_unless(freezer.account_id == config.freezer && freezer.is_authorized, E_NOT_FREEZER, "only the freeze authority freezes");
            match eth_token {
                None => {
                    expect_row(&target, ca, me, "the first row is the config PDA");
                    plan.effect(&target, &Effect::SetLive { mint, burn });
                }
                Some(t) => {
                    expect_row(&target, entry_account(&me, &ca, &t), me, "the first row is the token's entry PDA");
                    plan.effect(&target, &Effect::FreezeToken { mint, burn });
                }
            }
        }
        Instruction::Claim(c) => {
            let Claim { config, eth_token, salt, amount, slot, root, proof, index, path, destination, secret } = *c;
            let [hdr, cfg, entry, nul, def, dest] = rows::<6>(input);
            let ca = config_account(&me, &config);
            expect_row(&hdr, header_account(&config.sextant, &config.instance_id, slot), config.sextant, "the first row is Sextant's header record for this slot");
            expect_row(&cfg, ca, me, "the second row is the config PDA");
            expect_row(&entry, entry_account(&me, &ca, &eth_token), me, "the third row is the token's entry PDA");
            let (did, dseed) = definition_account(&me, &ca, &eth_token, &salt);
            expect_row(&def, did, config.token_program, "the fifth row is the wrapped token's definition");
            // The tokens go to the committed destination's associated token
            // account (a public owner), or to the committed account itself: a
            // fresh private account, which a privacy-preserving claim creates
            // and which no one else can name (one account holds one token).
            let ata = token::ata_account(&config.ata_program, &destination, &did, &config.token_program);
            refuse_unless(
                (dest.account_id == ata || dest.account_id == destination) && dest.program_account_id == config.token_program,
                E_ACCOUNTS,
                "the sixth row is the destination's associated token account for this token, or the destination itself",
            );
            refuse_unless(amount > 0, E_AMOUNT, "nothing to claim");

            // The deposit: the leaf the vault appended for this opening, under `root`.
            let pair = pairing(&me, &config);
            let commitment = wrap_core::commitment(&pair, destination.value(), &secret);
            let leaf = wrap_core::leaf(config.chain_id, &config.vault, &pair, &eth_token, amount, &commitment);
            refuse_unless(wrap_core::root_from_path(&leaf, index, &path) == Some(root), E_PATH, "no such deposit under this root: check the token, amount, destination, secret and path");
            // The root: the vault's own `knownRoot[root]` slot, proven by Sextant below.
            refuse_unless(proof.address == config.vault, E_PROOF, "the storage proof is not the configured vault's");
            refuse_unless(proof.slot == wrap_core::known_root_slot(&root), E_PROOF, "the storage proof is not knownRoot[root]");

            let nf = wrap_core::nullifier(&pair, &secret, index);
            expect_row(&nul, nullifier_account(&me, &ca, &nf), me, "the fourth row is this deposit's nullifier PDA");

            plan.effect(&cfg, &Effect::CheckLive { mint: true });
            plan.effect(&entry, &Effect::Mint { salt, amount });
            plan.effect(&nul, &Effect::Spend);
            plan.call(ChainedCall::new(
                config.sextant,
                vec![ProgramShardSelector::from(&hdr)],
                &sextant_program::Instruction::Attest { instance_id: config.instance_id, slot, query: query(&config, proof) },
            ));
            plan.call(
                ChainedCall::new(
                    config.token_program,
                    vec![ProgramShardSelector::from(&def), ProgramShardSelector::from(&dest)],
                    &token::Instruction::Mint { amount_to_mint: amount },
                )
                .with_pda_seeds(vec![dseed]),
            );
            plan.event(event("Claimed", &Claimed { nullifier: nf }));
        }
        Instruction::Burn { config, eth_token, salt, amount, eth_commitment } => {
            let [cfg, entry, burns, def, owner, ata] = rows::<6>(input);
            let ca = config_account(&me, &config);
            expect_row(&cfg, ca, me, "the first row is the config PDA");
            expect_row(&entry, entry_account(&me, &ca, &eth_token), me, "the second row is the token's entry PDA");
            expect_row(&burns, burns_account(&me, &ca), me, "the third row is the burn tree PDA");
            let (did, _) = definition_account(&me, &ca, &eth_token, &salt);
            expect_row(&def, did, config.token_program, "the fourth row is the wrapped token's definition");
            refuse_unless(owner.is_authorized, E_NOT_HOLDER, "the fifth row is the burning owner, who signs");
            expect_row(&ata, token::ata_account(&config.ata_program, &owner.account_id, &did, &config.token_program), config.token_program, "the sixth row is the owner's associated token account for this token");
            let leaf = wrap_core::burn_leaf(&pairing(&me, &config), &eth_token, amount, &eth_commitment);
            plan.effect(&cfg, &Effect::CheckLive { mint: false });
            plan.effect(&entry, &Effect::Burn { salt, amount });
            plan.effect(&burns, &Effect::Append(leaf));
            // The ATA program authorizes the holding by its own seed, given the owner's signature.
            plan.call(ChainedCall::new(
                config.ata_program,
                vec![ProgramShardSelector::from(&owner), ProgramShardSelector::from(&ata), ProgramShardSelector::from(&def)],
                &token::AtaInstruction::Burn { token_program_id: config.token_program, kind: token::TokenKind::Fungible, amount },
            ));
            plan.event(event("Burned", &Burned { leaf }));
        }
        Instruction::BurnFrom { config, eth_token, salt, amount, eth_commitment } => {
            let [cfg, entry, burns, def, holding] = rows::<5>(input);
            let ca = config_account(&me, &config);
            expect_row(&cfg, ca, me, "BurnFrom: the first row is the config PDA");
            expect_row(&entry, entry_account(&me, &ca, &eth_token), me, "BurnFrom: the second row is the token's entry PDA");
            expect_row(&burns, burns_account(&me, &ca), me, "BurnFrom: the third row is the burn tree PDA");
            let (did, _) = definition_account(&me, &ca, &eth_token, &salt);
            expect_row(&def, did, config.token_program, "BurnFrom: the fourth row is the wrapped token's definition");
            // The token program refuses an unauthorized holding too; refusing
            // here first gives the typed code, and does not rest on it alone.
            refuse_unless(holding.is_authorized, E_NOT_HOLDER, "the fifth row is the burning holding, authorized by its owner");
            refuse_unless(holding.program_account_id == config.token_program, E_ACCOUNTS, "the fifth row is the holding's token-program shard");
            let leaf = wrap_core::burn_leaf(&pairing(&me, &config), &eth_token, amount, &eth_commitment);
            plan.effect(&cfg, &Effect::CheckLive { mint: false });
            plan.effect(&entry, &Effect::Burn { salt, amount });
            plan.effect(&burns, &Effect::Append(leaf));
            plan.call(ChainedCall::new(
                config.token_program,
                vec![ProgramShardSelector::from(&def), ProgramShardSelector::from(&holding)],
                &token::Instruction::Burn { amount_to_burn: amount, kind: token::TokenKind::Fungible },
            ));
            plan.event(event("Burned", &Burned { leaf }));
        }
    }
    plan
}

// ---- apply ----

fn put<T: BorshSerialize>(t: &T) -> Vec<u8> {
    borsh::to_vec(t).expect("infallible")
}

fn entry_of(pre: &[u8], salt: &H256) -> TokenEntry {
    refuse_unless(!pre.is_empty(), E_UNSUPPORTED, "this token is not registered in this configuration");
    let e: TokenEntry = borsh::from_slice(pre).expect("a token entry");
    refuse_unless(&e.salt == salt, E_UNSUPPORTED, "this is not the registered wrapped token: re-read its salt");
    e
}

#[must_use]
pub fn apply(effect: Effect, pre: &[u8]) -> Option<Vec<u8>> {
    match effect {
        Effect::InitLive => {
            refuse_unless(pre.is_empty(), E_EXISTS, "this configuration is already initialised");
            Some(put(&Live::default()))
        }
        Effect::CheckLive { mint } => {
            refuse_unless(!pre.is_empty(), E_NO_CONFIG, "no such configuration: send Init first");
            let l: Live = borsh::from_slice(pre).expect("live flags");
            refuse_unless(!(if mint { l.frozen_mint } else { l.frozen_burn }), E_FROZEN, "the bridge is frozen in this direction");
            None
        }
        Effect::SetLive { mint, burn } => {
            refuse_unless(!pre.is_empty(), E_NO_CONFIG, "no such configuration: send Init first");
            Some(put(&Live { frozen_mint: mint, frozen_burn: burn }))
        }
        Effect::Register(e) => {
            refuse_unless(pre.is_empty(), E_EXISTS, "this token is already registered: use SetPolicy");
            Some(put(&e))
        }
        Effect::SetPolicy(p) => {
            refuse_unless(!pre.is_empty(), E_UNSUPPORTED, "this token is not registered in this configuration");
            let mut e: TokenEntry = borsh::from_slice(pre).expect("a token entry");
            e.policy = p;
            Some(put(&e))
        }
        Effect::FreezeToken { mint, burn } => {
            refuse_unless(!pre.is_empty(), E_UNSUPPORTED, "this token is not registered in this configuration");
            let mut e: TokenEntry = borsh::from_slice(pre).expect("a token entry");
            e.frozen_mint = mint;
            e.frozen_burn = burn;
            Some(put(&e))
        }
        Effect::Mint { salt, amount } => {
            let mut e = entry_of(pre, &salt);
            refuse_unless(!e.frozen_mint, E_FROZEN, "minting of this token is frozen");
            // No denomination check: the vault only accepted a permitted amount,
            // and a later policy change must not strand that deposit (R.4).
            let outstanding = e.minted - e.burned;
            refuse_unless(outstanding.checked_add(amount).is_some_and(|o| o <= e.policy.mint_cap), E_CAP, "the token's supply cap is reached: retry later");
            e.minted += amount;
            Some(put(&e))
        }
        Effect::Burn { salt, amount } => {
            let mut e = entry_of(pre, &salt);
            refuse_unless(!e.frozen_burn, E_FROZEN, "burning of this token is frozen");
            refuse_unless(e.policy.denominations.contains(&amount), E_AMOUNT, "burn one of the token's permitted amounts");
            e.burned += amount;
            Some(put(&e))
        }
        Effect::Spend => {
            refuse_unless(pre.is_empty(), E_CLAIMED, "this deposit was already claimed");
            Some(vec![1])
        }
        Effect::Append(leaf) => {
            let mut f: Frontier = if pre.is_empty() { Frontier::default() } else { borsh::from_slice(pre).expect("a frontier") };
            f.insert(&leaf).unwrap_or_else(|| refuse(E_TREE_FULL, "the burn tree is full: migrate to a new deployment"));
            Some(put(&f))
        }
    }
}

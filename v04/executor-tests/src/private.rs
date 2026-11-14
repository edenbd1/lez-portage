//! The private claim on LEZ v0.4.0-rc3: one privacy-preserving transaction
//! whose root call is `ClaimPrivate` to the destination's own wrap_bridge
//! actor, run privately (the opening never leaves the proof), and whose fee
//! is paid inside the proof by a funded private account.
//!
//! What the transaction makes public (its public part, settled by the
//! sequencer): `Attest` to Sextant's header record (the vault root and its
//! slot), `CheckLive` to the config, `Spend` to the deposit's nullifier PDA,
//! `Mint` to the token's entry, the token program's `Mint` and
//! `IncreaseSupply`, the definition's `InitHolding` reply and its
//! `IncreaseBalance` credit, both addressed to a **blinded alias** of the
//! destination (`AccountId::blinded`), and the fee leg (one native credit to
//! the fee inbox, from a blinded alias of the payer). The destination's
//! wrap_bridge actor and token actor run privately: its holding is
//! initialized (if fresh) and credited inside the proof (the credit is
//! promoted, `select_promotion`), and its new state is an encrypted note only
//! its keys open.
//!
//! The fee leg follows rc3's rules, as lez-sextant's private update does
//! (lez-sextant branch `v0.4`, `seq_drive private`): the fee inbox is a
//! declared public actor; a funded private payer, witnessed in the same proof,
//! credits one native reserve across the boundary to it
//! (`ProvingInput::fee`), presented under a fresh blinded alias whose recovery
//! entry lets settlement refund the unspent part; the reserve is priced off a
//! simulation of the public part at the sequencer's quoted base fees. On the
//! local network the payers are the two private accounts `networks/local.json`
//! funds at genesis; the transaction claims that pending credit itself.

use std::collections::BTreeSet;

use lee::{
    privacy_preserving_transaction::{circuit::ProgramCatalog, message::Message as PMessage, witness_set::WitnessSet as PWitness},
    program::Program,
    AccountId, Actor, PrivacyPreservingTransaction, ProvingInput, PublicExecutionContext, SenderPresentation, Simulation,
};
use lee_core::{
    account::Account,
    encryption::ViewingPublicKey,
    execution_state::{Placement, TransactionEntry},
    program::MessageBody,
    AuthorizationSecretKey, EncryptionScheme, NullifierPublicKey, NullifierSecretKey, NullifierWitness, PrivateWitness, RecipientEncryption, RegularKey, RootCall,
    SharedSecretKey, WitnessKind, DUMMY_COMMITMENT_HASH,
};
use wrap_bridge_v04::{Claim, Config, Message};
use wrap_core::H256;

/// A private account's keys (lee's `test_utils::TestPrivateKeys`, which only a
/// debug build of lee enables).
pub struct Keys {
    pub ask: AuthorizationSecretKey,
    pub d: [u8; 32],
    pub z: [u8; 32],
}

impl Keys {
    #[must_use]
    pub fn new(seed: u8) -> Self {
        Self { ask: AuthorizationSecretKey([seed; 32]), d: [seed.wrapping_add(1); 32], z: [seed.wrapping_add(2); 32] }
    }
    #[must_use]
    pub fn nsk(&self) -> NullifierSecretKey {
        NullifierSecretKey::from(&self.ask)
    }
    #[must_use]
    pub fn npk(&self) -> NullifierPublicKey {
        NullifierPublicKey::from(&self.nsk())
    }
    #[must_use]
    pub fn vpk(&self) -> ViewingPublicKey {
        ViewingPublicKey::from_seed(&self.d, &self.z)
    }
    #[must_use]
    pub fn account_id(&self) -> AccountId {
        AccountId::for_regular_private_account(&self.npk(), &self.vpk())
    }
    /// A fresh private account (no commitment yet, so no membership proof),
    /// authorized, also addressable at the blinded aliases `openings` open.
    #[must_use]
    pub fn init_witness(&self, openings: &[H256]) -> PrivateWitness {
        PrivateWitness {
            vpk: self.vpk(),
            random_seed: [0; 32],
            kind: WitnessKind::Regular(RegularKey::Authorized(self.ask.clone())),
            nullifier: NullifierWitness::Init { commitment_root: DUMMY_COMMITMENT_HASH },
            address_openings: openings.iter().copied().collect::<BTreeSet<_>>(),
        }
    }
    /// This account's new state, from the transaction's encrypted notes.
    #[must_use]
    pub fn decrypt(&self, tx: &PrivacyPreservingTransaction) -> Option<Account> {
        let commitments: Vec<_> = tx.message.execution.private_actions.iter().map(|a| a.commitment).collect();
        tx.message.execution.private_actions.iter().find_map(|a| {
            let secret = SharedSecretKey::decapsulate(&a.encrypted_post_state.epk, &self.d, &self.z)?;
            let (_, state) = EncryptionScheme::decrypt(&a.encrypted_post_state.ciphertext, &secret, &a.nullifier)?;
            commitments.contains(&lee_core::Commitment::new(&self.account_id(), &state)).then_some(state)
        })
    }
}

/// The private destination of the local runs (a fixed test key).
#[must_use]
pub fn carol() -> Keys {
    Keys::new(0x61)
}

/// The private fee payers: the two private accounts of LEZ's local network
/// (`lez/testnet_initial_state`, `SSK_PRIV_ACC_A` and `_B`, public test keys
/// of a local genesis), which `networks/local.json` funds at genesis
/// (`supply_private_account`) as pending native credits.
#[must_use]
pub fn fee_payers() -> [Keys; 2] {
    const SSK_A: [u8; 32] = [93, 13, 190, 240, 250, 33, 108, 195, 176, 40, 144, 61, 4, 28, 58, 112, 53, 161, 42, 238, 155, 27, 23, 176, 208, 121, 15, 229, 165, 180, 99, 143];
    const SSK_B: [u8; 32] = [48, 175, 124, 10, 230, 240, 166, 14, 249, 254, 157, 226, 208, 124, 122, 177, 203, 139, 192, 180, 43, 120, 55, 151, 50, 21, 113, 22, 254, 83, 148, 56];
    [SSK_A, SSK_B].map(|ssk| {
        let h = key_protocol::key_management::secret_holders::SecretSpendingKey(ssk).produce_private_key_holder(None);
        Keys { ask: h.authorization_secret_key.clone(), d: h.viewing_secret_key.d, z: h.viewing_secret_key.z }
    })
}

pub fn no_seal(body: &MessageBody) -> Result<RecipientEncryption, lee::error::LeeError> {
    Err(lee::error::LeeError::InvalidInput(format!("no seal for an async message to {:?}", body.to)))
}

#[must_use]
pub fn fee_inbox() -> AccountId {
    fee_core::compute_fee_inbox_account_id(fee_core::fee_account_id())
}

/// The peak resident set, bytes, of this process and of its finished children
/// (the prover runs in an `r0vm` child process).
#[must_use]
pub fn peak_rss() -> u64 {
    let get = |who| {
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(who, &mut u) };
        let v = u.ru_maxrss as u64; // macOS: bytes; Linux: KiB
        if cfg!(target_os = "macos") { v } else { v * 1024 }
    };
    get(libc::RUSAGE_SELF).max(get(libc::RUSAGE_CHILDREN))
}

/// The deployment a private claim runs against.
#[derive(Clone, Copy)]
pub struct Programs {
    pub token: AccountId,
    pub ata: AccountId,
    pub sextant_lc: AccountId,
    pub wrap_bridge: AccountId,
}

impl Programs {
    /// The programs a private claim's proof runs privately (wrap_bridge and
    /// the token program at the destination) and the public ones it simulates.
    #[must_use]
    pub fn catalog(&self) -> ProgramCatalog {
        ProgramCatalog::from([
            (self.token, crate::program("token.bin")),
            (self.ata, crate::program("ata.bin")),
            (self.sextant_lc, crate::program("sextant_lc.bin")),
            (self.wrap_bridge, crate::program("wrap_bridge.bin")),
        ])
    }
}

/// The blinding factor of the destination's alias for a claim: derived from
/// the deposit's secret, so the depositor's wallet recomputes it and nobody
/// else can.
#[must_use]
pub fn claim_alias(secret: &H256) -> H256 {
    wrap_core::sha256(&[b"rfp022/v0.4/private-claim/alias", secret])
}

/// The private claim's root call, its message and the public actors it
/// reaches (without the fee inbox).
#[must_use]
pub fn claim_call(p: &Programs, cfg: &Config, c: &Claim) -> (Actor, Vec<u8>, Vec<Actor>) {
    let me = p.wrap_bridge;
    let ca = wrap_bridge_v04::config_account(&me, cfg);
    let pair = wrap_bridge_v04::pairing(&me, cfg);
    let nf = wrap_core::nullifier(&pair, &c.secret, c.index);
    let nul = wrap_bridge_v04::nullifier_account(&me, &ca, &nf);
    let hdr = wrap_bridge_v04::header_account(&cfg.sextant, &cfg.instance_id, c.slot);
    let entry = wrap_bridge_v04::entry_account(&me, &ca, &c.eth_token);
    let def = wrap_bridge_v04::definition_account(&me, &ca, &c.eth_token, &c.salt).0;
    let tp = cfg.token_program;
    let public = vec![
        Actor::new(hdr, cfg.sextant),
        Actor::new(ca, me),
        Actor::new(entry, me),
        Actor::new(nul, me),
        Actor::new(entry, tp),
        Actor::new(def, tp),
    ];
    let m = Message::ClaimPrivate { claim: Box::new(c.clone()), alias: claim_alias(&c.secret) };
    (Actor::new(c.destination, me), borsh::to_vec(&m).expect("infallible"), public)
}

/// Everything about the destination is presented under its alias; the payer's
/// native actor under its own fresh alias; the rest as it is.
pub fn presenter(dest: AccountId, alias: H256, payer: Option<(AccountId, H256)>) -> impl FnMut(Actor) -> SenderPresentation + Clone {
    move |a: Actor| {
        if a.account_id == dest {
            SenderPresentation::Blinded(alias)
        } else if payer.is_some_and(|(id, _)| a == Actor::native_balance(id)) {
            SenderPresentation::Blinded(payer.expect("checked").1)
        } else {
            SenderPresentation::Canonical
        }
    }
}

/// Promote (run inside the proof) exactly the async messages to the
/// destination's alias: the mint's credit.
pub fn promote_alias(alias_id: AccountId) -> impl FnMut(Placement, &MessageBody) -> bool + Clone {
    move |_, body: &MessageBody| body.to.account_id == alias_id
}

/// The base input of a private claim (no fee leg): the destination fresh and
/// authorized, addressable at its alias.
#[must_use]
pub fn claim_input(root: Actor, message: Vec<u8>, public: &[Actor], dest: &Keys, alias: H256) -> ProvingInput {
    ProvingInput {
        root: TransactionEntry::Sync(RootCall { to: root, message }),
        claims: vec![],
        fee: None,
        public_context: PublicExecutionContext::new(public.iter().copied(), []),
        private_witnesses: vec![dest.init_witness(&[alias])],
        dummy_inputs: vec![],
        ciphertext_padding: None,
        recoveries: vec![],
        private_promotions: BTreeSet::new(),
    }
}

/// Prove `input` and assemble the transaction.
pub fn prove(
    input: ProvingInput,
    sim: &Simulation,
    catalog: &ProgramCatalog,
    present: impl FnMut(Actor) -> SenderPresentation,
    select: impl FnMut(Placement, &MessageBody) -> bool,
) -> Result<PrivacyPreservingTransaction, lee::error::LeeError> {
    let (output, proof) = lee::execute_and_prove(input, sim, catalog, present, select, no_seal)?;
    let message = PMessage::from_circuit_output(std::collections::BTreeMap::new(), output);
    Ok(PrivacyPreservingTransaction::new(message.clone(), PWitness::for_message(&message, proof, &[])))
}

/// The wrapped balance of `keys`' own (private) token actor after `tx`.
#[must_use]
pub fn private_balance(keys: &Keys, tx: &PrivacyPreservingTransaction, token_program: AccountId) -> Option<(AccountId, u128)> {
    let a = keys.decrypt(tx)?;
    match borsh::from_slice::<wrap_bridge_v04::token::TokenState>(a.data.actor_state(token_program)).ok()? {
        wrap_bridge_v04::token::TokenState::Holding(h) => Some((h.definition_id, h.balance)),
        wrap_bridge_v04::token::TokenState::Definition(_) => None,
    }
}

/// Fake proofs (the circuit still runs in full; the receipt is not a STARK).
pub fn dev_mode() {
    // SAFETY: set once, before any prover thread starts.
    unsafe { std::env::set_var("RISC0_DEV_MODE", "1") };
}

/// The program binary of the circuit's catalog entry for `name`.
#[must_use]
pub fn catalog_program(name: &str) -> Program {
    crate::program(name)
}

// ---- the private burn ----

impl Keys {
    /// This account at its committed state `account` (membership `proof`),
    /// authorized, addressable at the aliases `openings` open.
    #[must_use]
    pub fn update_witness(&self, account: Account, proof: lee_core::MembershipProof, openings: &[H256]) -> PrivateWitness {
        PrivateWitness {
            vpk: self.vpk(),
            random_seed: [0; 32],
            kind: WitnessKind::Regular(RegularKey::Authorized(self.ask.clone())),
            nullifier: NullifierWitness::Update { account, membership_proof: proof },
            address_openings: openings.iter().copied().collect::<BTreeSet<_>>(),
        }
    }
}

/// The blinding factor of a private holder's alias for one burn.
#[must_use]
pub fn burn_alias(blinding: &H256) -> H256 {
    wrap_core::sha256(&[b"rfp022/v0.4/private-burn/alias", blinding])
}

/// The private burn: a **public** root call (`Burn` to the burn tree, so its
/// leaf is in the block and chain/v04 counts it), whose holder is a blinded
/// alias of a private account; the holder's own token actor runs privately
/// and moves the tokens into the entry's escrow (`own_holding`). Returns the
/// root, the message and the public actors (the root's included).
#[must_use]
pub fn burn_call(p: &Programs, cfg: &Config, eth_token: [u8; 20], amount: u128, eth_commitment: H256, holder_alias: AccountId) -> (Actor, Vec<u8>, Vec<Actor>) {
    let me = p.wrap_bridge;
    let ca = wrap_bridge_v04::config_account(&me, cfg);
    let entry = wrap_bridge_v04::entry_account(&me, &ca, &eth_token);
    let def = wrap_bridge_v04::definition_account(&me, &ca, &eth_token, &crate::SALT).0;
    let burns = wrap_bridge_v04::burns_account(&me, &ca);
    let tp = cfg.token_program;
    let root = Actor::new(burns, me);
    let m = Message::Burn { eth_token, amount, eth_commitment, salt: crate::SALT, holder: holder_alias, config: cfg.clone(), own_holding: true };
    let public = vec![root, Actor::new(ca, me), Actor::new(entry, me), Actor::new(entry, tp), Actor::new(def, tp)];
    (root, borsh::to_vec(&m).expect("infallible"), public)
}

/// The base input of a private burn (no fee leg).
#[must_use]
pub fn burn_input(root: Actor, message: Vec<u8>, public: &[Actor], holder: PrivateWitness) -> ProvingInput {
    ProvingInput {
        root: TransactionEntry::Sync(RootCall { to: root, message }),
        claims: vec![],
        fee: None,
        public_context: PublicExecutionContext::new(public.iter().copied(), []),
        private_witnesses: vec![holder],
        dummy_inputs: vec![],
        ciphertext_padding: None,
        recoveries: vec![],
        private_promotions: BTreeSet::new(),
    }
}

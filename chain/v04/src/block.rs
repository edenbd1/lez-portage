//! The borsh layout of a LEZ v0.4 block (rc1 to rc3), mirrored field for field from
//! `common::block`, `common::transaction` and `lee`/`lee_core` at the
//! v0.4.0-rc1 tag (1806e873; the same at rc3, bafd07be), with byte arrays for ids, keys, digests
//! and seeds. v04/executor-tests (tests/chain_v04.rs) decodes real blocks from
//! a v0.4.0-rc3 sequencer with both these types and LEZ's own, and checks
//! that both consume every byte and re-encode to the same bytes.
//!
//! Sets and maps are `BTreeSet`/`BTreeMap` as in LEZ. borsh (built, as LEZ
//! builds it, without `de_strict_order`) collects an out-of-order or repeated
//! encoding into its sorted form, which re-encodes to other bytes; the scan
//! (`super::scan`) refuses such an encoding instead, since it hashes the bytes
//! it read. LEZ writes the ascending encoding only. One difference: LEZ refuses, while
//! decoding, a public key that is not a valid secp256k1 x-coordinate; these
//! types read any 32 bytes (a block with one is invalid on LEZ either way).

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};

pub use wrap_core::H256;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub body: BlockBody,
    pub bedrock_status: BedrockStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BlockHeader {
    pub block_id: u64,
    pub prev_block_hash: H256,
    pub hash: H256,
    pub timestamp: u64,
}

/// New in v0.4: `reverted`, the indices of the charged transactions whose
/// action reverted (ascending). The block hash covers it.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BlockBody {
    pub transactions: Vec<LeeTransaction>,
    pub reverted: Vec<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum BedrockStatus {
    Pending,
    Safe,
    Finalized,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum LeeTransaction {
    Public(PublicTransaction),
    PrivacyPreserving(PrivacyPreservingTransaction),
}

/// `lee_core::account::Actor`: an account paired with a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
pub struct Actor {
    pub account_id: H256,
    pub program_account_id: H256,
}

/// `lee_core::execution_state::PublicExecutionContext`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PublicExecutionContext {
    pub actors: BTreeSet<Actor>,
    pub authorized_accounts: BTreeSet<H256>,
    pub promotions: BTreeSet<u64>,
}

/// `lee::PublicAccountEvidence`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum PublicAccountEvidence {
    Key([u8; 32]),
    Pda { program: H256, seed: H256 },
}

/// `lee_core::RootCall`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct RootCall {
    pub to: Actor,
    pub message: Vec<u8>,
}

/// `lee::fees::FeeDeclaration`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct FeeDeclaration {
    pub payer: H256,
    pub gas_limit: u64,
    pub tip: u64,
    pub max_fee: u128,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Signature {
    pub value: [u8; 64],
}

// ---- public (lee::public_transaction) ----

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PublicTransaction {
    pub message: PublicMessage,
    pub witness_set: PublicWitnessSet,
}

/// `lee::TransactionMessage<PublicExecution>`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PublicMessage {
    pub public_context: PublicExecutionContext,
    pub execution: PublicExecution,
    pub nonces: BTreeMap<H256, u128>,
    pub admission_evidence: Vec<PublicAccountEvidence>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PublicExecution {
    pub root: RootCall,
    pub fee: Option<FeeDeclaration>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PublicWitnessSet {
    pub signatures_and_public_keys: Vec<(Signature, [u8; 32])>,
}

// ---- privacy-preserving (lee::privacy_preserving_transaction) ----

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PrivacyPreservingTransaction {
    pub message: PrivacyMessage,
    pub witness_set: PrivacyWitnessSet,
}

/// `lee::TransactionMessage<lee_core::ProvenExecution>`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PrivacyMessage {
    pub public_context: PublicExecutionContext,
    pub execution: ProvenExecution,
    pub nonces: BTreeMap<H256, u128>,
    pub admission_evidence: Vec<PublicAccountEvidence>,
}

/// `lee_core::ProvenExecution`: what the privacy circuit's receipt commits to.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ProvenExecution {
    pub boundary: Vec<BoundaryStep>,
    pub async_messages: Vec<SealedMessage>,
    pub recovery_bindings: Vec<RecoveryBinding>,
    pub public_root: Option<RootCall>,
    pub private_actions: Vec<PrivateAction>,
    pub validity: ValidityWindows,
    pub program_image_claims: Vec<ProgramImageClaim>,
    pub fee: Option<ProvenFee>,
}

/// `lee_core::execution_state::BoundaryStep`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum BoundaryStep {
    PrivateToPublic(Delivery),
    PublicToPrivate(Delivery),
    EndPrivateSubtree,
    EndPublicSubtree,
}

/// `lee_core::execution_state::Delivery<Actor>`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Delivery {
    pub from: Actor,
    pub to: Actor,
    pub message: Vec<u8>,
    pub inherited_authorizations: BTreeSet<H256>,
    pub inherits_entry_authorizations: bool,
    pub pda_seeds: BTreeSet<H256>,
}

/// `lee_core::encryption::EncryptedNote`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct EncryptedNote {
    pub ciphertext: Vec<u8>,
    pub epk: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SealedMessage {
    pub commitment: H256,
    pub note: EncryptedNote,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct RecoveryBinding {
    pub address: H256,
    pub note: EncryptedNote,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PrivateAction {
    pub nullifier: H256,
    pub root: H256,
    pub commitment: H256,
    pub encrypted_post_state: EncryptedNote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ValidityWindow {
    pub from: Option<u64>,
    pub to: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ValidityWindows {
    pub blocks: ValidityWindow,
    pub timestamps: ValidityWindow,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum ProgramImageClaim {
    Disclosed { account_id: H256, image_id: [u32; 8] },
    Undisclosed { root: H256 },
}

/// New in v0.4: the private transaction's fee is paid inside its proof.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ProvenFee {
    pub refund_to: Option<H256>,
    pub fallback: Option<FeeFallback>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct FeeFallback {
    pub action: PrivateAction,
    pub boundary: Vec<BoundaryStep>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PrivacyWitnessSet {
    pub signatures_and_public_keys: Vec<(Signature, [u8; 32])>,
    /// The privacy circuit's receipt (`circuit::Proof(Vec<u8>)`).
    pub proof: Vec<u8>,
}

impl LeeTransaction {
    /// The transaction's root call, if it reveals one: always for a public
    /// transaction, `public_root` for a private one.
    pub fn root_call(&self) -> Option<&RootCall> {
        match self {
            Self::Public(t) => Some(&t.message.execution.root),
            Self::PrivacyPreserving(t) => t.message.execution.public_root.as_ref(),
        }
    }
}

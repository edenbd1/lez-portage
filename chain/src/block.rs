//! The borsh layout of a LEZ v0.3.1 block, mirrored field for field from
//! `common::block` and `lee` (logos-execution-zone 1a003bfb): only what it
//! takes to decode a block exactly, with plain byte arrays for ids, keys and
//! digests. `cli/tests/chain_layout.rs` decodes real blocks from a v0.3.1
//! sequencer with both these types and LEZ's own, and checks that both
//! consume every byte and re-encode to the same bytes.
//!
//! One difference: LEZ refuses, while decoding, a public key that is not a
//! valid secp256k1 x-coordinate; these types read any 32 bytes. A block with
//! such a key is invalid on LEZ; accepting it here only matters if invalid
//! blocks are, and block validity is outside this crate's statement either way.

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

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BlockBody {
    pub transactions: Vec<LeeTransaction>,
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

// ---- public (lee::public_transaction) ----

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PublicTransaction {
    pub message: PublicMessage,
    pub witness_set: PublicWitnessSet,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PublicMessage {
    pub program_account_id: H256,
    pub shard_selectors: Vec<ProgramShardSelector>,
    pub nonces: Vec<u128>,
    pub instruction_data: Vec<u8>,
    pub fee: Option<FeeDeclaration>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ProgramShardSelector {
    pub account_id: H256,
    pub program_account_id: H256,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct FeeDeclaration {
    pub payer: H256,
    pub gas_limit: u64,
    pub tip: u64,
    pub max_fee: u128,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PublicWitnessSet {
    pub signatures_and_public_keys: Vec<(Signature, PublicKey)>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Signature {
    pub value: [u8; 64],
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PublicKey(pub [u8; 32]);

// ---- privacy-preserving (lee::privacy_preserving_transaction) ----

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PrivacyPreservingTransaction {
    pub message: PrivacyMessage,
    pub witness_set: PrivacyWitnessSet,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PrivacyMessage {
    pub public_actions: Vec<PublicActionWithId>,
    pub nonces: Vec<u128>,
    pub private_actions: Vec<PrivateAction>,
    pub block_validity_window: ValidityWindow,
    pub timestamp_validity_window: ValidityWindow,
    pub program_image_claims: Vec<ProgramImageClaim>,
}

/// `lee::privacy_preserving_transaction::message::PublicActionWithID`: the
/// effects the execution deferred to settlement on one public account.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PublicActionWithId {
    pub account_id: H256,
    pub effects: Vec<DeferredPublicEffect>,
}

/// `lee_core::execution_state::DeferredPublicEffect`: `data` is the borsh of
/// the planning program's own effect type, applied by that program.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct DeferredPublicEffect {
    pub program_account_id: H256,
    pub shard_program_account_id: H256,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PrivateAction {
    pub nullifier: H256,
    pub root: H256,
    pub commitment: H256,
    pub encrypted_post_state: EncryptedAccountData,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct EncryptedAccountData {
    pub ciphertext: Vec<u8>,
    pub epk: Vec<u8>,
    pub view_tag: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ValidityWindow {
    pub from: Option<u64>,
    pub to: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum ProgramImageClaim {
    Disclosed { account_id: H256, image_id: [u32; 8] },
    Undisclosed { root: H256 },
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PrivacyWitnessSet {
    pub signatures_and_public_keys: Vec<(Signature, PublicKey)>,
    /// The privacy circuit's receipt, as LEZ serialises it.
    pub proof: Vec<u8>,
}

//! LEZ v0.4.0-rc1 blocks (docs/V04-PORT.md).
//!
//! Two things change for the chain-derived release:
//!
//! - **The block body lists reverted transactions.** `BlockBody` gains
//!   `reverted: Vec<u64>` after `transactions` (`common::block`, v0.4.0-rc1),
//!   and the block hash covers it (`HashableBlockData` gains the same field,
//!   still after `transactions`, so the hash is still
//!   `sha256(PREFIX ‖ raw[0..40] ‖ raw[72..80] ‖ raw[80..len-1])`, the
//!   formula of [`lez_chain::content_hash`]). A node replaying the block refuses it
//!   if the list differs from what settlement reverts
//!   (`chain_state::apply`, `BlockIngestError::RevertMismatch`). So a
//!   transaction in a valid block took effect exactly when its index is not
//!   listed, and that is readable from the block alone.
//! - **Burns are read from root calls.** v0.4's wrap_bridge accepts `Burn`
//!   only as a transaction's root call (`from == None`), to the burn-tree PDA,
//!   and the leaf is a function of that message. A public transaction always
//!   reveals its root call; a private one reveals it in `public_root` when its
//!   root is public. A root call is the whole statement of what was asked, and
//!   "not reverted" is the statement that it all took effect.
//!
//! So on v0.4 the tree holds **public burns**, which v0.3.1 blocks could not
//! tell applied from refused. A burn whose root is private (a private holder
//! calling wrap_bridge from inside the proof) is not counted; it cannot be
//! one on this program (Burn refuses a non-root sender).
//!
//! As on v0.3.1, what this does not establish: that the block is valid, and
//! that it is final on Bedrock.

pub mod block;
pub mod scan;

use borsh::{BorshDeserialize, BorshSerialize};
use wrap_core::{H256, DEPTH};

pub use lez_chain::ChainError;

/// `wrap_bridge_v04::BURN_VARIANT`, the borsh index of `Message::Burn`
/// (`Init, Register, SetPolicy, Freeze, Claim, Burn`). Pinned by a test
/// against the program's enum (v04/executor-tests).
pub const BURN_VARIANT: u8 = 5;

pub const CHAIN_TAG: H256 = wrap_core::tag(b"rfp022/lez-chain/v04");

/// Which deployment's burns count: the bridge program's account id, its
/// burn-tree PDA, and the pairing its burn leaves are under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Params {
    pub program_id: H256,
    pub burns_account: H256,
    pub pairing: H256,
}

pub use lez_chain::ChainState;

/// The burn leaves of a decoded block, the typed reference for [`scan::scan_block`].
pub fn burn_leaves(b: &block::Block, params: &Params) -> Vec<H256> {
    let mut out = Vec::new();
    for (i, tx) in b.body.transactions.iter().enumerate() {
        if b.body.reverted.contains(&(i as u64)) {
            continue;
        }
        let Some(root) = tx.root_call() else { continue };
        if root.to.account_id != params.burns_account || root.to.program_account_id != params.program_id {
            continue;
        }
        let m = &root.message;
        if m.len() < 69 || m[0] != BURN_VARIANT {
            continue;
        }
        let eth_token: [u8; 20] = m[1..21].try_into().expect("20 bytes");
        let amount = u128::from_le_bytes(m[21..37].try_into().expect("16 bytes"));
        let eth_commitment: H256 = m[37..69].try_into().expect("32 bytes");
        out.push(wrap_core::burn_leaf(&params.pairing, &eth_token, amount, &eth_commitment));
    }
    out
}

/// Apply one v0.4 block: decode it strictly, check its hash and its link to
/// the tip, append its burn leaves. On `Err` the state is unchanged.
pub fn apply_block(state: &mut ChainState, params: &Params, raw: &[u8]) -> Result<Vec<H256>, ChainError> {
    let (h, leaves) = scan::scan_block(raw, params).map_err(|_| ChainError::Undecodable)?;
    if lez_chain::content_hash(raw) != h.hash {
        return Err(ChainError::HashMismatch { block_id: h.block_id });
    }
    let expected_id = state.tip_id.checked_add(1).ok_or(ChainError::NotNext { expected_id: u64::MAX, got_id: h.block_id })?;
    if h.block_id != expected_id {
        return Err(ChainError::NotNext { expected_id, got_id: h.block_id });
    }
    if h.prev_block_hash != state.tip_hash {
        return Err(ChainError::BrokenLink { block_id: h.block_id });
    }
    let mut tree = state.tree.clone();
    for l in &leaves {
        tree.insert(l).ok_or(ChainError::TreeFull)?;
    }
    state.tree = tree;
    state.tip_id = h.block_id;
    state.tip_hash = h.hash;
    Ok(leaves)
}

/// The v0.4 chain journal, `36 * 32` bytes: as [`crate::journal`] with its
/// own tag, and the pairing after the burn-tree account:
///
/// ```text
/// 0 CHAIN_TAG   1 image id   2 program_id   3 burns_account   4 pairing
/// 5 anchor_id   6 anchor_hash   7 tip_id   8 tip_hash   9 next_index   10 root   11.. filled
/// ```
pub const JOURNAL_LEN: usize = 32 * (11 + DEPTH);

pub fn journal(self_image_id: &H256, params: &Params, s: &ChainState) -> Vec<u8> {
    let mut out = Vec::with_capacity(JOURNAL_LEN);
    for w in [
        CHAIN_TAG,
        *self_image_id,
        params.program_id,
        params.burns_account,
        params.pairing,
        wrap_core::word_u64(s.anchor_id),
        s.anchor_hash,
        wrap_core::word_u64(s.tip_id),
        s.tip_hash,
        wrap_core::word_u64(s.tree.next_index),
        s.tree.root,
    ] {
        out.extend_from_slice(&w);
    }
    for f in &s.tree.filled {
        out.extend_from_slice(f);
    }
    out
}

/// What the v0.4 chain guest reads first (then `blocks` blocks, each `u32 len ‖ bytes`).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ChainHead {
    pub self_image_id: H256,
    pub params: Params,
    pub anchor_id: u64,
    pub anchor_hash: H256,
    pub blocks: u32,
}

/// The guest's stdin for `blocks`.
pub fn encode_stream(head: &ChainHead, blocks: &[Vec<u8>]) -> Vec<u8> {
    let h = borsh::to_vec(head).expect("infallible");
    let mut out = Vec::new();
    out.extend_from_slice(&(h.len() as u32).to_le_bytes());
    out.extend_from_slice(&h);
    for b in blocks {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b);
    }
    out
}

/// The guest's statement: the journal after `blocks` from the anchor.
pub fn chain_journal(head: &ChainHead, blocks: &[Vec<u8>]) -> Result<Vec<u8>, ChainError> {
    let mut s = ChainState::at_anchor(head.anchor_id, head.anchor_hash);
    for b in blocks {
        apply_block(&mut s, &head.params, b)?;
    }
    Ok(journal(&head.self_image_id, &head.params, &s))
}

#[cfg(test)]
mod tests;

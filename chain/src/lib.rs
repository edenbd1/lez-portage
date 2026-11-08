//! lez-chain: what the chain-derived release proves about LEZ, as plain
//! functions.
//!
//! A LEZ v0.3.1 block is `borsh(Block)`: an 80-byte header (`block_id`,
//! `prev_block_hash`, `hash`, `timestamp`), the transactions, and the
//! sequencer's local Bedrock status (one byte, not hashed). Its hash is
//!
//! ```text
//! hash = sha256("/LEE/v0.3/Message/Block/" ‖ 8 zero bytes ‖ block_id ‖ prev_block_hash ‖ timestamp ‖ borsh(transactions))
//! ```
//!
//! (`common::block::HashableBlockData::compute_hash`), so the hash commits to
//! every transaction byte. The sequencer inscribes the same `borsh(Block)` on
//! its Bedrock channel.
//!
//! [`apply_block`] decodes a block strictly ([`block::Block`] mirrors LEZ's
//! layout; trailing bytes are refused), recomputes its hash, checks that it is
//! the next block after the state's tip, and appends to the burn tree the
//! `wrap_bridge` burn leaves its **privacy-preserving** transactions carry:
//! an `Append(leaf)` effect of the bridge program on the bridge's burn-tree
//! account. Why only those, and why that is enough:
//!
//! - LEZ v0.3.1 classifies every privacy-preserving transaction as fee-exempt
//!   (`chain_state::classify`), and an exempt transaction whose settlement
//!   fails makes the whole block invalid (`chain_state::apply::settle_transaction`:
//!   any `Err` "invalidates the block"). So in a valid block, every effect of
//!   every privacy-preserving transaction was applied: its `Append` is in the
//!   burn tree.
//! - A public transaction is charged, and a charged transaction whose action
//!   fails is still included: it "reverts, keeping the fee"
//!   (`settle_charged_transaction`). The block does not record which ones
//!   reverted, so a public `Burn`'s leaf cannot be told applied from refused
//!   without re-executing LEZ state. Public burns are left out: the tree
//!   built here holds private burns only. It is a different tree from the
//!   program's own burn tree (which holds both), with its own indices.
//!
//! What this does not establish, stated plainly: that the block is valid
//! (its private transactions' proofs, nullifiers and commitment roots, and its
//! public effects, are checked by the sequencer and by LEZ nodes re-executing
//! it, not here), and that it is final on Bedrock. Both rest outside this
//! crate; docs/BEDROCK-FINALITY.md.

pub mod block;
pub mod scan;

use borsh::{BorshDeserialize, BorshSerialize};
use wrap_core::{Frontier, H256, DEPTH};

/// `common::block::HashableBlockData::compute_hash`'s domain prefix.
pub const BLOCK_PREFIX: &[u8; 32] = b"/LEE/v0.3/Message/Block/\x00\x00\x00\x00\x00\x00\x00\x00";

/// The borsh variant index of `wrap_bridge::Effect::Append(H256)`, the tenth
/// variant (`InitLive, CheckLive, SetLive, Register, SetPolicy, FreezeToken,
/// Mint, Burn, Spend, Append`). Pinned by a test against the program's enum.
pub const APPEND_VARIANT: u8 = 9;

/// The journal's first word.
pub const CHAIN_TAG: H256 = wrap_core::tag(b"rfp022/lez-chain/v1");

/// Which deployment's burns count: the bridge program's account id (the one
/// its pairing names) and its burn-tree account (`burns_account(program,
/// config)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Params {
    pub program_id: H256,
    pub burns_account: H256,
}

/// Where the chain stands after the blocks applied so far.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ChainState {
    /// The trusted starting point: a block id and hash, fixed at deployment.
    pub anchor_id: u64,
    pub anchor_hash: H256,
    /// The last block applied (the anchor before any).
    pub tip_id: u64,
    pub tip_hash: H256,
    /// The tree of private burn leaves, in chain order.
    pub tree: Frontier,
}

impl ChainState {
    pub fn at_anchor(anchor_id: u64, anchor_hash: H256) -> Self {
        Self { anchor_id, anchor_hash, tip_id: anchor_id, tip_hash: anchor_hash, tree: Frontier::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainError {
    /// Not a complete `borsh(Block)` of the v0.3.1 layout.
    Undecodable,
    /// The header's hash is not the hash of the block's contents.
    HashMismatch { block_id: u64 },
    /// The block is not the one after the tip.
    NotNext { expected_id: u64, got_id: u64 },
    BrokenLink { block_id: u64 },
    TreeFull,
    /// `prior` is not a journal of this chain: another guest, deployment or anchor.
    ForeignPrior,
}

/// The hash the header of `raw` (a strict `borsh(Block)`) must carry:
/// `sha256(PREFIX ‖ block_id ‖ prev ‖ timestamp ‖ borsh(transactions))`,
/// from `raw`'s own bytes (the header is 80 bytes; the last byte is the
/// unhashed Bedrock status).
pub fn content_hash(raw: &[u8]) -> H256 {
    wrap_core::sha256(&[BLOCK_PREFIX, &raw[..40], &raw[72..80], &raw[80..raw.len() - 1]])
}

/// The burn leaves a decoded block's privacy-preserving transactions append
/// to `params.burns_account`, in order.
pub fn private_burn_leaves(b: &block::Block, params: &Params) -> Vec<H256> {
    let mut out = Vec::new();
    for tx in &b.body.transactions {
        let block::LeeTransaction::PrivacyPreserving(pp) = tx else { continue };
        for action in &pp.message.public_actions {
            if action.account_id != params.burns_account {
                continue;
            }
            for e in &action.effects {
                if e.program_account_id == params.program_id
                    && e.shard_program_account_id == params.program_id
                    && e.data.len() == 33
                    && e.data[0] == APPEND_VARIANT
                {
                    out.push(e.data[1..].try_into().expect("32 bytes"));
                }
            }
        }
    }
    out
}

/// Apply one block: decode it, check its hash and its link to the tip, append
/// its private burn leaves. Returns the leaves appended. On `Err` the state is
/// unchanged.
pub fn apply_block(state: &mut ChainState, params: &Params, raw: &[u8]) -> Result<Vec<H256>, ChainError> {
    let (h, leaves) = scan::scan_block(raw, params).map_err(|_| ChainError::Undecodable)?;
    if content_hash(raw) != h.hash {
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

/// The chain guest's journal, `35 * 32` bytes, ABI words:
///
/// ```text
/// 0 CHAIN_TAG   1 the guest's own image id   2 program_id   3 burns_account
/// 4 anchor_id   5 anchor_hash   6 tip_id   7 tip_hash
/// 8 tree.next_index   9 tree.root   10.. tree.filled[0..24]
/// ```
///
/// Word 1 is how a later step, and the release guest, know which program
/// wrote it: each verifies the receipt under that id, and Ethereum pins it.
pub const JOURNAL_WORDS: usize = 10 + DEPTH;
pub const JOURNAL_LEN: usize = 32 * JOURNAL_WORDS;

pub fn journal(self_image_id: &H256, params: &Params, s: &ChainState) -> Vec<u8> {
    let mut out = Vec::with_capacity(JOURNAL_LEN);
    for w in [
        CHAIN_TAG,
        *self_image_id,
        params.program_id,
        params.burns_account,
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

fn word_to_u64(w: &[u8]) -> Option<u64> {
    if w[..24].iter().any(|b| *b != 0) {
        return None;
    }
    Some(u64::from_be_bytes(w[24..32].try_into().ok()?))
}

/// The inverse of [`journal`]: `(self_image_id, params, state)`, or `None`.
pub fn parse_journal(j: &[u8]) -> Option<(H256, Params, ChainState)> {
    if j.len() != JOURNAL_LEN {
        return None;
    }
    let w = |i: usize| -> H256 { j[32 * i..32 * (i + 1)].try_into().expect("32 bytes") };
    if w(0) != CHAIN_TAG {
        return None;
    }
    let filled = (0..DEPTH).map(|k| w(10 + k)).collect();
    let state = ChainState {
        anchor_id: word_to_u64(&w(4))?,
        anchor_hash: w(5),
        tip_id: word_to_u64(&w(6))?,
        tip_hash: w(7),
        tree: Frontier { next_index: word_to_u64(&w(8))?, filled, root: w(9) },
    };
    Some((w(1), Params { program_id: w(2), burns_account: w(3) }, state))
}

/// What the chain guest reads. `prior` is the journal of an earlier receipt of
/// this same guest (verified inside, as an assumption, under
/// `self_image_id`); without it the chain starts at the anchor.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ChainInput {
    pub self_image_id: H256,
    pub params: Params,
    pub anchor_id: u64,
    pub anchor_hash: H256,
    pub prior: Option<Vec<u8>>,
    pub blocks: Vec<Vec<u8>>,
}

/// What the chain guest reads first: everything but the blocks, and how many
/// blocks follow. The guest then reads the blocks one by one into one buffer
/// ([`encode_stream`]), so that a block's bytes are touched once.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ChainHead {
    pub self_image_id: H256,
    pub params: Params,
    pub anchor_id: u64,
    pub anchor_hash: H256,
    pub prior: Option<Vec<u8>>,
    pub blocks: u32,
}

impl ChainInput {
    pub fn head(&self) -> ChainHead {
        ChainHead {
            self_image_id: self.self_image_id,
            params: self.params,
            anchor_id: self.anchor_id,
            anchor_hash: self.anchor_hash,
            prior: self.prior.clone(),
            blocks: self.blocks.len() as u32,
        }
    }
}

/// The guest's stdin: `u32 len ‖ borsh(ChainHead)`, then `u32 len ‖ block`
/// for each block (lengths little-endian).
pub fn encode_stream(i: &ChainInput) -> Vec<u8> {
    let head = borsh::to_vec(&i.head()).expect("infallible");
    let mut out = Vec::with_capacity(4 + head.len() + i.blocks.iter().map(|b| 4 + b.len()).sum::<usize>());
    out.extend_from_slice(&(head.len() as u32).to_le_bytes());
    out.extend_from_slice(&head);
    for b in &i.blocks {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b);
    }
    out
}

/// Where the chain starts: the anchor, or `prior`'s state if `prior` is a
/// journal of this chain (the same image id, params and anchor). The guest
/// checks `prior`'s receipt with `env::verify` before calling this.
pub fn start(h: &ChainHead) -> Result<ChainState, ChainError> {
    match &h.prior {
        None => Ok(ChainState::at_anchor(h.anchor_id, h.anchor_hash)),
        Some(p) => {
            let (id, params, s) = parse_journal(p).ok_or(ChainError::ForeignPrior)?;
            if id != h.self_image_id || params != h.params || s.anchor_id != h.anchor_id || s.anchor_hash != h.anchor_hash {
                return Err(ChainError::ForeignPrior);
            }
            Ok(s)
        }
    }
}

/// The chain guest's statement, minus the receipt check of `prior`: the
/// journal after applying `blocks` to the anchor or to `prior`'s state. `Err`
/// if `prior` is not this chain's, or a block does not apply.
pub fn chain_journal(i: &ChainInput) -> Result<Vec<u8>, ChainError> {
    let mut state = start(&i.head())?;
    for raw in &i.blocks {
        apply_block(&mut state, &i.params, raw)?;
    }
    Ok(journal(&i.self_image_id, &i.params, &state))
}

/// The release guest's journal tail after the 352-byte release preimage, ABI
/// words: the chain receipt's image id, program id, burn-tree account, anchor
/// id and hash, and the tip id and hash its tree root is as of.
pub const RELEASE_TAIL_WORDS: usize = 7;

pub fn release_tail(chain_image_id: &H256, params: &Params, s: &ChainState) -> Vec<u8> {
    [
        *chain_image_id,
        params.program_id,
        params.burns_account,
        wrap_core::word_u64(s.anchor_id),
        s.anchor_hash,
        wrap_core::word_u64(s.tip_id),
        s.tip_hash,
    ]
    .concat()
}

/// The release guest's statement (`zk/burn_release`), minus the receipt
/// check of `chain_journal` (the guest verifies it, as an assumption, under the
/// image id the journal names for itself): the burn `input` opens is in the
/// chain journal's tree, and the journal is the vault's 352-byte release
/// preimage of that tree's root and the burn's nullifier, then
/// [`release_tail`]. `None` if the opening is malformed, if its path does not
/// lead to the chain's root, or if `chain_journal` is not a chain journal.
#[cfg(feature = "release")]
pub fn release_journal(input: &wrap_core::zk::ZkReleaseInput, chain_journal: &[u8]) -> Option<Vec<u8>> {
    let (chain_image_id, params, state) = parse_journal(chain_journal)?;
    let pre = wrap_core::zk::zk_release_journal(input)?;
    if pre[32 * 9..32 * 10] != state.tree.root {
        return None;
    }
    Some([pre, release_tail(&chain_image_id, &params, &state)].concat())
}

#[cfg(test)]
mod tests;

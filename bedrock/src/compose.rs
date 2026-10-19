//! The Bedrock anchor of a LEZ chain proof, and the release over it.
//!
//! [`anchor_journal`] is the statement of the guest `zk/bedrock_anchor`, minus
//! the receipt check of the chain journal (the guest verifies it, as an
//! assumption, under the image id the chain journal names for itself): the
//! LEZ block that journal ends at is the inscription [`crate::anchor`] finds,
//! on the pinned channel, signed by the pinned sequencer key, in a Bedrock
//! block with `depth` headers built on it, the last of which is `top_id`.
//!
//! [`release_journal`] is the statement of `zk/bedrock_release`: the release of
//! lez-chain over the chain journal inside an anchor journal, then the anchor's
//! Bedrock fields.

extern crate std;

use alloc::vec::Vec;

use borsh::{BorshDeserialize, BorshSerialize};
use wrap_core::H256;

use crate::{AnchorBlock, Error, Params};

/// The anchor journal's first word.
pub const ANCHOR_TAG: H256 = wrap_core::tag(b"rfp022/bedrock-anchor/v1");

/// The anchor journal: 9 words, then the chain journal (35 words). ABI words:
///
/// ```text
/// 0 ANCHOR_TAG   1 the anchor guest's own image id   2 channel_id   3 signer
/// 4 the inscription's Bedrock block id   5 its slot   6 the top header id   7 its slot   8 depth
/// 9.. the chain journal (lez_chain::journal), whose tip is the inscribed LEZ block
/// ```
pub const HEAD_WORDS: usize = 9;
pub const JOURNAL_LEN: usize = 32 * HEAD_WORDS + lez_chain::JOURNAL_LEN;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeError {
    /// Not a chain journal.
    ChainJournal,
    Bedrock(Error),
    /// The inscription is not a strict `borsh(Block)` of LEZ v0.3.1, or its
    /// hash is not the hash of its contents.
    NotALezBlock,
    /// The inscribed LEZ block is not the chain journal's tip.
    NotTheTip,
}

/// What the anchor guest reads.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AnchorInput {
    pub self_image_id: H256,
    pub channel_id: H256,
    pub signer: H256,
    pub chain_journal: Vec<u8>,
    pub block: AnchorBlock,
}

/// The anchor fields, parsed back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchorHead {
    pub self_image_id: H256,
    pub params: Params,
    pub block_id: H256,
    pub block_slot: u64,
    pub top_id: H256,
    pub top_slot: u64,
    pub depth: u32,
}

pub fn anchor_journal(i: &AnchorInput) -> Result<Vec<u8>, ComposeError> {
    let (_, chain_params, state) = lez_chain::parse_journal(&i.chain_journal).ok_or(ComposeError::ChainJournal)?;
    let params = Params { channel_id: i.channel_id, signer: i.signer };
    let a = crate::anchor(&params, &i.block, 0).map_err(ComposeError::Bedrock)?;
    let lez = a.inscription.data;
    let (h, _) = lez_chain::scan::scan_block(lez, &chain_params).map_err(|_| ComposeError::NotALezBlock)?;
    if lez_chain::content_hash(lez) != h.hash {
        return Err(ComposeError::NotALezBlock);
    }
    if h.block_id != state.tip_id || h.hash != state.tip_hash {
        return Err(ComposeError::NotTheTip);
    }
    let mut out = Vec::with_capacity(JOURNAL_LEN);
    for w in [
        ANCHOR_TAG,
        i.self_image_id,
        i.channel_id,
        i.signer,
        a.block_id,
        wrap_core::word_u64(a.block_slot),
        a.top_id,
        wrap_core::word_u64(a.top_slot),
        wrap_core::word_u64(a.depth as u64),
    ] {
        out.extend_from_slice(&w);
    }
    out.extend_from_slice(&i.chain_journal);
    Ok(out)
}

fn word_u64(w: &[u8]) -> Option<u64> {
    if w[..24].iter().any(|b| *b != 0) {
        return None;
    }
    Some(u64::from_be_bytes(w[24..32].try_into().ok()?))
}

/// The inverse of [`anchor_journal`]: the anchor fields and the chain journal.
pub fn parse_anchor_journal(j: &[u8]) -> Option<(AnchorHead, &[u8])> {
    if j.len() != JOURNAL_LEN {
        return None;
    }
    let w = |k: usize| -> H256 { j[32 * k..32 * (k + 1)].try_into().expect("32 bytes") };
    if w(0) != ANCHOR_TAG {
        return None;
    }
    let chain = &j[32 * HEAD_WORDS..];
    lez_chain::parse_journal(chain)?;
    let depth = word_u64(&w(8))?;
    Some((
        AnchorHead {
            self_image_id: w(1),
            params: Params { channel_id: w(2), signer: w(3) },
            block_id: w(4),
            block_slot: word_u64(&w(5))?,
            top_id: w(6),
            top_slot: word_u64(&w(7))?,
            depth: u32::try_from(depth).ok()?,
        },
        chain,
    ))
}

/// The release journal's tail after lez_chain's (352-byte preimage + 7 words),
/// ABI words: the anchor receipt's image id, channel_id, signer, the
/// inscription's Bedrock block id, the top header id, its slot, depth.
pub const RELEASE_TAIL_WORDS: usize = 7;
pub const RELEASE_JOURNAL_LEN: usize = 352 + 32 * (lez_chain::RELEASE_TAIL_WORDS + RELEASE_TAIL_WORDS);

/// The release guest's statement (`zk/bedrock_release`), minus the receipt
/// check of `anchor_journal`: lez-chain's release over the chain journal the
/// anchor journal carries, then the anchor's Bedrock fields. `None` if
/// `anchor_journal` is not one, or the opening is not a burn of its tree.
#[cfg(feature = "release")]
pub fn release_journal(input: &wrap_core::zk::ZkReleaseInput, anchor_journal: &[u8]) -> Option<Vec<u8>> {
    let (a, chain) = parse_anchor_journal(anchor_journal)?;
    let mut out = lez_chain::release_journal(input, chain)?;
    for w in [
        a.self_image_id,
        a.params.channel_id,
        a.params.signer,
        a.block_id,
        a.top_id,
        wrap_core::word_u64(a.top_slot),
        wrap_core::word_u64(a.depth as u64),
    ] {
        out.extend_from_slice(&w);
    }
    Some(out)
}

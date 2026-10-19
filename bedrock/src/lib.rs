//! lez-bedrock: what the Bedrock-anchored release proves about Bedrock.
//!
//! Everything here was read from the Bedrock node at `3926a532` (the revision
//! LEZ v0.3.1 pins; the 0.3.1 release binary speaks the same formats) and is
//! checked against that node's own types on real blocks by `bedrock/tools`.
//!
//! **Header** (`core/src/header/mod.rs`). Its canonical encoding is 296 bytes:
//!
//! ```text
//! slot u64 LE ‖ parent_block 32 ‖ body_root 32 ‖ proof 128 ‖ entropy 32 ‖ leader_key 32 ‖ voucher_cm 32
//! ```
//!
//! and its id hashes the same fields in another order:
//!
//! ```text
//! HeaderId = blake2b256("BLOCK_ID_V1" ‖ parent ‖ slot LE ‖ body_root ‖ voucher_cm ‖ entropy ‖ proof ‖ leader_key)
//! ```
//!
//! **Body** (`core/src/block/mod.rs`, `core/src/utils/merkle.rs`):
//!
//! ```text
//! body_root = blake2b256("BODY_ROOT_V1" ‖ canonical(uncle_headers) ‖ tx_root)
//! tx_root   = binary Merkle root of the transaction hashes, padded with zero leaves to a power of two,
//!             node = blake2b256(left ‖ right); one transaction: its hash; none: 32 zero bytes
//! tx_hash   = blake2b256("MANTLE_TXHASH_V1" ‖ canonical(ops))
//! ```
//!
//! A block holds at most 1,024 transactions, so a path has at most 10 nodes.
//! The hash covers the operations, not their proofs: the inscription's
//! signature is checked separately, against the hash.
//!
//! **Inscription** (`core/src/mantle/ops/channel/inscribe.rs`). `canonical(ops)`
//! is a one-byte count, then each operation as `opcode ‖ payload`. LEZ's
//! sequencer publishes a block as a transaction whose **first** operation is
//! the inscription (opcode `0x11`), followed by the fee transfer the node's
//! wallet adds (`lez/sequencer/actors/bedrock/src/actor.rs`, `PublishBlock`):
//!
//! ```text
//! InscriptionOp = channel_id 32 ‖ u32 LE length ‖ inscription bytes ‖ parent MsgId 32 ‖ signer Ed25519 32
//! ```
//!
//! The signer signs the transaction hash (`verify_strict`, weak keys refused).
//!
//! What the functions here do **not** establish, stated plainly: that a header
//! is on Bedrock's canonical chain (its proof of leadership needs note-set
//! roots no header commits to; docs/BEDROCK-FINALITY.md), that the leader's
//! block signature is valid (the node's block API does not return it), that
//! the signer was the channel's accredited key at that slot (channel state is
//! ledger state), and that the inscribed LEZ block is valid.

#![no_std]

extern crate alloc;

use blake2::{digest::consts::U32, Blake2b, Digest};

#[cfg(feature = "chain")]
pub mod compose;

#[cfg(test)]
mod tests;

pub type H256 = [u8; 32];

type Blake2b256 = Blake2b<U32>;

/// A header's canonical encoding.
pub const HEADER_LEN: usize = 8 + 32 + 32 + 128 + 32 + 32 + 32;
/// A signed uncle header's: the header and the leader's Ed25519 signature.
pub const SIGNED_HEADER_LEN: usize = HEADER_LEN + 64;
/// `lb_cryptarchia_engine::MAX_UNCLES`; the uncle list's length prefix is one byte.
pub const MAX_UNCLES: usize = 4;
/// `MAX_BLOCK_TRANSACTIONS` = 1,024 = 2^10.
pub const MAX_PATH: usize = 10;
/// `InscriptionOp::CODE`.
pub const OP_CHANNEL_INSCRIBE: u8 = 0x11;
/// `inscribe::MAX_BYTES`: 7/8 of a 2 MiB block.
pub const MAX_INSCRIPTION: usize = 1024 * 1024 * 2 * 7 / 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A header is not 296 bytes.
    HeaderLength,
    /// A header at slot 0: only genesis is there (`verify_header_alone`).
    GenesisSlot,
    /// The uncle list is not a canonical list of signed headers.
    Uncles,
    /// The Merkle path is longer than a block allows, or its index does not fit it.
    Path,
    /// The body root of the header is not the one the uncles and the path lead to.
    BodyRoot,
    /// The transaction does not open with a well-formed ChannelInscribe.
    NotAnInscription,
    /// The inscription is on another channel.
    Channel,
    /// The inscription's signer is not the pinned sequencer key.
    Signer,
    /// The signer's signature over the transaction hash does not verify.
    Signature,
    /// A descendant header's parent is not the header before it.
    Parent,
    /// A descendant header's slot is not after its parent's.
    Slot,
    /// Fewer descendants than asked for.
    Depth,
}

fn blake2b256(parts: &[&[u8]]) -> H256 {
    let mut h = Blake2b256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// A header, by reference into its canonical bytes.
#[derive(Clone, Copy)]
pub struct Header<'a>(&'a [u8; HEADER_LEN]);

impl<'a> Header<'a> {
    pub fn parse(raw: &'a [u8]) -> Result<Self, Error> {
        Ok(Header(raw.try_into().map_err(|_| Error::HeaderLength)?))
    }
    pub fn slot(&self) -> u64 {
        u64::from_le_bytes(self.0[0..8].try_into().expect("8 bytes"))
    }
    pub fn parent(&self) -> H256 {
        self.0[8..40].try_into().expect("32 bytes")
    }
    pub fn body_root(&self) -> H256 {
        self.0[40..72].try_into().expect("32 bytes")
    }
    /// `HeaderId`: the canonical fields, reordered as `Header::update_hasher` hashes them.
    pub fn id(&self) -> H256 {
        let r = self.0;
        let (slot, parent, body) = (&r[0..8], &r[8..40], &r[40..72]);
        let (proof, entropy, leader, voucher) = (&r[72..200], &r[200..232], &r[232..264], &r[264..296]);
        blake2b256(&[b"BLOCK_ID_V1", parent, slot, body, voucher, entropy, proof, leader])
    }
}

/// `blake2b256("MANTLE_TXHASH_V1" ‖ ops)`.
pub fn tx_hash(ops: &[u8]) -> H256 {
    blake2b256(&[b"MANTLE_TXHASH_V1", ops])
}

/// `merkle::node`.
pub fn node(l: &H256, r: &H256) -> H256 {
    blake2b256(&[l, r])
}

/// The transaction root a leaf at `index` leads to by `path` (siblings, leaf
/// level first). The tree has `2^path.len()` leaves, so `index` must be below
/// that, and a path has at most [`MAX_PATH`] nodes.
///
/// The leaf is a transaction hash this crate computed from the transaction's
/// bytes, of a preimage longer than 64 bytes (an inscription carries a LEZ
/// block), so it cannot also be an inner node of a deeper tree without a
/// Blake2b collision: the tree's nodes carry no domain tag, the leaf's
/// preimage length is what tells them apart.
pub fn merkle_root(leaf: &H256, index: u32, path: &[H256]) -> Result<H256, Error> {
    if path.len() > MAX_PATH || (index as u64) >> path.len() != 0 {
        return Err(Error::Path);
    }
    let mut h = *leaf;
    for (k, s) in path.iter().enumerate() {
        h = if (index >> k) & 1 == 0 { node(&h, s) } else { node(s, &h) };
    }
    Ok(h)
}

/// The uncle list's canonical encoding is a one-byte count, then that many
/// signed headers. Only its length is checked: the body root's preimage ends
/// with the 32-byte transaction root, so the bytes before it cannot shift it.
pub fn check_uncles(uncles: &[u8]) -> Result<(), Error> {
    let (n, rest) = uncles.split_first().ok_or(Error::Uncles)?;
    if *n as usize > MAX_UNCLES || rest.len() != *n as usize * SIGNED_HEADER_LEN {
        return Err(Error::Uncles);
    }
    for k in 0..*n as usize {
        let h = Header::parse(&rest[k * SIGNED_HEADER_LEN..k * SIGNED_HEADER_LEN + HEADER_LEN])?;
        if h.slot() == 0 {
            return Err(Error::Uncles);
        }
    }
    Ok(())
}

/// `block::body_root`.
pub fn body_root(uncles: &[u8], tx_root: &H256) -> H256 {
    blake2b256(&[b"BODY_ROOT_V1", uncles, tx_root])
}

/// A ChannelInscribe operation, by reference into the transaction's bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inscription<'a> {
    pub channel_id: H256,
    pub data: &'a [u8],
    pub parent: H256,
    pub signer: H256,
}

/// The transaction's first operation, which must be a ChannelInscribe. The
/// operations after it are not read: the encoding is read from the start, so
/// the first one's bounds do not depend on them, and the transaction hash
/// covers them all.
pub fn first_inscription(ops: &[u8]) -> Result<Inscription<'_>, Error> {
    let e = Error::NotAnInscription;
    let count = *ops.first().ok_or(e)?;
    if count == 0 || *ops.get(1).ok_or(e)? != OP_CHANNEL_INSCRIBE {
        return Err(e);
    }
    let b = &ops[2..];
    let channel_id: H256 = b.get(..32).ok_or(e)?.try_into().map_err(|_| e)?;
    let len = u32::from_le_bytes(b.get(32..36).ok_or(e)?.try_into().map_err(|_| e)?) as usize;
    if len > MAX_INSCRIPTION {
        return Err(e);
    }
    let data = b.get(36..36 + len).ok_or(e)?;
    let tail = b.get(36 + len..36 + len + 64).ok_or(e)?;
    if count == 1 && b.len() != 36 + len + 64 {
        return Err(e);
    }
    Ok(Inscription {
        channel_id,
        data,
        parent: tail[..32].try_into().expect("32 bytes"),
        signer: tail[32..].try_into().expect("32 bytes"),
    })
}

/// `UnverifiedEd25519PublicKey::verify`: ed25519-dalek's `verify_strict` over
/// the transaction hash, a weak (small-order) key refused first, as the
/// inscription's `preverify` does.
pub fn verify_signature(signer: &H256, tx_hash: &H256, sig: &[u8; 64]) -> Result<(), Error> {
    let key = ed25519_dalek::VerifyingKey::from_bytes(signer).map_err(|_| Error::Signature)?;
    if key.is_weak() {
        return Err(Error::Signature);
    }
    key.verify_strict(tx_hash, &ed25519_dalek::Signature::from_bytes(sig)).map_err(|_| Error::Signature)
}

/// What a deployment pins: the LEZ sequencer's channel and its signing key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    pub channel_id: H256,
    pub signer: H256,
}

/// The Bedrock block that carries the inscription, and the headers built on it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "chain", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
pub struct AnchorBlock {
    /// Its header, canonical (296 bytes).
    pub header: alloc::vec::Vec<u8>,
    /// Its uncle list, canonical.
    pub uncles: alloc::vec::Vec<u8>,
    /// The inscription transaction's operations, canonical.
    pub ops: alloc::vec::Vec<u8>,
    /// The transaction's position in the block and its Merkle path.
    pub tx_index: u32,
    pub path: alloc::vec::Vec<H256>,
    /// The inscription's Ed25519 signature (its operation proof).
    pub signature: alloc::vec::Vec<u8>,
    /// Headers (canonical) of descendants, the child of `header` first.
    pub descendants: alloc::vec::Vec<alloc::vec::Vec<u8>>,
}

/// What [`anchor`] establishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchored<'a> {
    pub block_id: H256,
    pub block_slot: u64,
    pub tx_hash: H256,
    pub inscription: Inscription<'a>,
    /// The last descendant (the block itself if none) and how many there are.
    pub top_id: H256,
    pub top_slot: u64,
    pub depth: u32,
}

/// The inscription is the first operation of a transaction in the block whose
/// header is `a.header`, on `p.channel_id`, by `p.signer`, signed; and
/// `a.descendants` is a chain of headers on top of that block. `min_depth`
/// descendants at least.
pub fn anchor<'a>(p: &Params, a: &'a AnchorBlock, min_depth: u32) -> Result<Anchored<'a>, Error> {
    let h = Header::parse(&a.header)?;
    if h.slot() == 0 {
        return Err(Error::GenesisSlot);
    }
    let block_id = h.id();
    check_uncles(&a.uncles)?;
    let txh = tx_hash(&a.ops);
    let root = merkle_root(&txh, a.tx_index, &a.path)?;
    if body_root(&a.uncles, &root) != h.body_root() {
        return Err(Error::BodyRoot);
    }
    let ins = first_inscription(&a.ops)?;
    if ins.channel_id != p.channel_id {
        return Err(Error::Channel);
    }
    if ins.signer != p.signer {
        return Err(Error::Signer);
    }
    let sig: &[u8; 64] = a.signature.as_slice().try_into().map_err(|_| Error::Signature)?;
    verify_signature(&ins.signer, &txh, sig)?;
    let (mut top_id, mut top_slot) = (block_id, h.slot());
    for d in &a.descendants {
        let d = Header::parse(d)?;
        if d.parent() != top_id {
            return Err(Error::Parent);
        }
        if d.slot() <= top_slot {
            return Err(Error::Slot);
        }
        top_id = d.id();
        top_slot = d.slot();
    }
    let depth = a.descendants.len() as u32;
    if depth < min_depth {
        return Err(Error::Depth);
    }
    Ok(Anchored { block_id, block_slot: h.slot(), tx_hash: txh, inscription: ins, top_id, top_slot, depth })
}

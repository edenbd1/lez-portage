//! A strict, allocation-free reading of `borsh(Block)` for the guest: it
//! walks the same layout as [`crate::block`] field by field, refuses anything
//! borsh would refuse there (a short read, an enum or option tag out of
//! range, trailing bytes), skips byte strings without copying them, and
//! returns the header and the private burn leaves. In the zkVM every byte
//! copied costs cycles and every page touched costs paging cycles, and the
//! privacy proofs inside a block are about 250 kB each.
//!
//! The typed decoding stays the reference: tests check that both agree on
//! the leaves and refuse the same malformed blocks.

use crate::{Params, APPEND_VARIANT, H256};

pub struct Header {
    pub block_id: u64,
    pub prev_block_hash: H256,
    pub hash: H256,
}

struct Cur<'a> {
    b: &'a [u8],
    i: usize,
}

type R<T> = Result<T, ()>;

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        let end = self.i.checked_add(n).ok_or(())?;
        let s = self.b.get(self.i..end).ok_or(())?;
        self.i = end;
        Ok(s)
    }
    fn u8(&mut self) -> R<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> R<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| ())?))
    }
    fn u64(&mut self) -> R<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(|_| ())?))
    }
    fn h256(&mut self) -> R<H256> {
        self.take(32)?.try_into().map_err(|_| ())
    }
    /// A `Vec<u8>`: its bytes, uncopied.
    fn bytes(&mut self) -> R<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    /// A `Vec<T>` of fixed-size elements, skipped.
    fn skip_vec(&mut self, size: usize) -> R<()> {
        let n = self.u32()? as usize;
        self.take(n.checked_mul(size).ok_or(())?)?;
        Ok(())
    }
    /// An `Option<T>` of a fixed-size `T`, skipped.
    fn skip_option(&mut self, size: usize) -> R<()> {
        match self.u8()? {
            0 => Ok(()),
            1 => self.take(size).map(|_| ()),
            _ => Err(()),
        }
    }
}

/// Signature (64) and public key (32).
const WITNESS: usize = 96;

fn public_tx(c: &mut Cur) -> R<()> {
    c.take(32)?; // program_account_id
    c.skip_vec(64)?; // shard_selectors
    c.skip_vec(16)?; // nonces
    c.bytes()?; // instruction_data
    c.skip_option(32 + 8 + 8 + 16)?; // fee: payer, gas_limit, tip, max_fee
    c.skip_vec(WITNESS)
}

fn privacy_tx(c: &mut Cur, params: &Params, leaves: &mut Vec<H256>) -> R<()> {
    for _ in 0..c.u32()? {
        let account = c.h256()?;
        for _ in 0..c.u32()? {
            let program = c.h256()?;
            let shard = c.h256()?;
            let data = c.bytes()?;
            if account == params.burns_account
                && program == params.program_id
                && shard == params.program_id
                && data.len() == 33
                && data[0] == APPEND_VARIANT
            {
                leaves.push(data[1..].try_into().map_err(|_| ())?);
            }
        }
    }
    c.skip_vec(16)?; // nonces
    for _ in 0..c.u32()? {
        c.take(96)?; // nullifier, root, commitment
        c.bytes()?; // ciphertext
        c.bytes()?; // epk
        c.u8()?; // view_tag
    }
    for _ in 0..2 {
        // block and timestamp validity windows: from, to
        c.skip_option(8)?;
        c.skip_option(8)?;
    }
    for _ in 0..c.u32()? {
        match c.u8()? {
            0 => c.take(32 + 32)?, // Disclosed { account_id, image_id: [u32; 8] }
            1 => c.take(32)?,      // Undisclosed { root }
            _ => return Err(()),
        };
    }
    c.skip_vec(WITNESS)?;
    c.bytes()?; // proof
    Ok(())
}

/// The header and the private burn leaves of `raw`, or `Err` if `raw` is not
/// exactly one `borsh(Block)` of the v0.3.1 layout.
pub fn scan_block(raw: &[u8], params: &Params) -> Result<(Header, Vec<H256>), ()> {
    let mut c = Cur { b: raw, i: 0 };
    let block_id = c.u64()?;
    let prev_block_hash = c.h256()?;
    let hash = c.h256()?;
    c.u64()?; // timestamp
    let mut leaves = Vec::new();
    for _ in 0..c.u32()? {
        match c.u8()? {
            0 => public_tx(&mut c)?,
            1 => privacy_tx(&mut c, params, &mut leaves)?,
            _ => return Err(()),
        }
    }
    if c.u8()? > 2 {
        return Err(()); // bedrock_status
    }
    if c.i != raw.len() {
        return Err(());
    }
    Ok((Header { block_id, prev_block_hash, hash }, leaves))
}

//! A strict, allocation-light reading of a v0.4.0-rc1 `borsh(Block)` for the
//! guest: it walks the layout of [`super::block`] field by field, refuses what
//! borsh would refuse there (a short read, a tag out of range, a bool that is
//! not 0 or 1, trailing bytes), and a set or map not strictly ascending (the
//! encoding LEZ writes; borsh would normalize it, see `super::block`), skips
//! byte strings without copying them, and returns the header and the burn
//! leaves the block's root calls carry, minus those of reverted transactions.
//!
//! One check beyond borsh: the `reverted` list must be strictly ascending and
//! index a user transaction (not the trailing fee and clock transactions). A
//! valid block satisfies both (`chain_state::apply` builds the list in
//! transaction order and refuses a block whose list differs).

use super::Params;
use crate::H256;

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

/// Signature (64) and public key (32).
const WITNESS: usize = 96;
const ACTOR: usize = 64;

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
    fn bool(&mut self) -> R<()> {
        if self.u8()? > 1 {
            return Err(());
        }
        Ok(())
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
    /// A `BTreeSet<T>` (or the keys of a `BTreeMap<T, V>` with `val` bytes of
    /// value) of fixed-size `T` whose order is the byte order: skipped, and
    /// refused unless strictly ascending (canonical).
    fn skip_sorted(&mut self, key: usize, val: usize) -> R<()> {
        let n = self.u32()? as usize;
        let mut prev: Option<&[u8]> = None;
        for _ in 0..n {
            let k = self.take(key)?;
            if prev.is_some_and(|p| p >= k) {
                return Err(());
            }
            prev = Some(k);
            self.take(val)?;
        }
        Ok(())
    }
    /// A `BTreeSet<u64>`, ascending as numbers.
    fn skip_sorted_u64(&mut self) -> R<()> {
        let n = self.u32()?;
        let mut prev: Option<u64> = None;
        for _ in 0..n {
            let v = self.u64()?;
            if prev.is_some_and(|p| p >= v) {
                return Err(());
            }
            prev = Some(v);
        }
        Ok(())
    }
    fn skip_option(&mut self, size: usize) -> R<()> {
        match self.u8()? {
            0 => Ok(()),
            1 => self.take(size).map(|_| ()),
            _ => Err(()),
        }
    }
}

fn public_context(c: &mut Cur) -> R<()> {
    c.skip_sorted(ACTOR, 0)?; // actors
    c.skip_sorted(32, 0)?; // authorized_accounts
    c.skip_sorted_u64() // promotions
}

/// `nonces` (BTreeMap<AccountId, u128>) and `admission_evidence`.
fn message_tail(c: &mut Cur) -> R<()> {
    c.skip_sorted(32, 16)?;
    for _ in 0..c.u32()? {
        match c.u8()? {
            0 => c.take(32)?,      // Key(PublicKey)
            1 => c.take(32 + 32)?, // Pda { program, seed }
            _ => return Err(()),
        };
    }
    Ok(())
}

/// A root call: `(to.account_id, to.program_account_id, message)`.
fn root_call<'a>(c: &mut Cur<'a>) -> R<(&'a [u8], &'a [u8], &'a [u8])> {
    let account = c.take(32)?;
    let program = c.take(32)?;
    let message = c.bytes()?;
    Ok((account, program, message))
}

fn note(c: &mut Cur) -> R<()> {
    c.bytes()?; // ciphertext
    c.bytes()?; // epk
    Ok(())
}

fn private_action(c: &mut Cur) -> R<()> {
    c.take(96)?; // nullifier, root, commitment
    note(c)
}

fn boundary(c: &mut Cur) -> R<()> {
    for _ in 0..c.u32()? {
        match c.u8()? {
            0 | 1 => {
                c.take(2 * ACTOR)?; // from, to
                c.bytes()?; // message
                c.skip_sorted(32, 0)?; // inherited_authorizations
                c.bool()?; // inherits_entry_authorizations
                c.skip_sorted(32, 0)?; // pda_seeds
            }
            2 | 3 => {}
            _ => return Err(()),
        }
    }
    Ok(())
}

/// The leaf a root call carries, if it is a `Burn` to this deployment's burn
/// tree: `burn_leaf(pairing, eth_token, amount, eth_commitment)`, from the
/// message's first three fields (`wrap_bridge::Message::Burn`'s layout).
fn burn_leaf(params: &Params, (account, program, message): (&[u8], &[u8], &[u8])) -> Option<H256> {
    if account != params.burns_account || program != params.program_id {
        return None;
    }
    if message.len() < 1 + 20 + 16 + 32 || message[0] != super::BURN_VARIANT {
        return None;
    }
    let eth_token: [u8; 20] = message[1..21].try_into().ok()?;
    let amount = u128::from_le_bytes(message[21..37].try_into().ok()?);
    let eth_commitment: H256 = message[37..69].try_into().ok()?;
    Some(wrap_core::burn_leaf(&params.pairing, &eth_token, amount, &eth_commitment))
}

fn public_tx<'a>(c: &mut Cur<'a>) -> R<(&'a [u8], &'a [u8], &'a [u8])> {
    public_context(c)?;
    let root = root_call(c)?;
    c.skip_option(32 + 8 + 8 + 16)?; // fee: payer, gas_limit, tip, max_fee
    message_tail(c)?;
    c.skip_vec(WITNESS)?;
    Ok(root)
}

fn privacy_tx<'a>(c: &mut Cur<'a>) -> R<Option<(&'a [u8], &'a [u8], &'a [u8])>> {
    public_context(c)?;
    boundary(c)?;
    for _ in 0..c.u32()? {
        c.take(32)?; // async_messages: commitment
        note(c)?;
    }
    for _ in 0..c.u32()? {
        c.take(32)?; // recovery_bindings: address
        note(c)?;
    }
    let root = match c.u8()? {
        0 => None,
        1 => Some(root_call(c)?),
        _ => return Err(()),
    };
    for _ in 0..c.u32()? {
        private_action(c)?;
    }
    for _ in 0..4 {
        c.skip_option(8)?; // validity: blocks.from/to, timestamps.from/to
    }
    for _ in 0..c.u32()? {
        match c.u8()? {
            0 => c.take(32 + 32)?, // Disclosed { account_id, image_id }
            1 => c.take(32)?,      // Undisclosed { root }
            _ => return Err(()),
        };
    }
    match c.u8()? {
        0 => {}
        1 => {
            c.skip_option(32)?; // refund_to
            match c.u8()? {
                0 => {}
                1 => {
                    private_action(c)?;
                    boundary(c)?;
                }
                _ => return Err(()),
            }
        }
        _ => return Err(()),
    }
    message_tail(c)?;
    c.skip_vec(WITNESS)?;
    c.bytes()?; // proof
    Ok(root)
}

/// The header and the burn leaves of `raw`, in transaction order, or `Err`
/// if `raw` is not exactly one `borsh(Block)` of the v0.4.0-rc1 layout.
pub fn scan_block(raw: &[u8], params: &Params) -> Result<(Header, Vec<H256>), ()> {
    let mut c = Cur { b: raw, i: 0 };
    let block_id = c.u64()?;
    let prev_block_hash = c.h256()?;
    let hash = c.h256()?;
    c.u64()?; // timestamp
    let n = c.u32()?;
    let mut candidates: Vec<(u64, H256)> = Vec::new();
    for i in 0..n {
        let root = match c.u8()? {
            0 => Some(public_tx(&mut c)?),
            1 => privacy_tx(&mut c)?,
            _ => return Err(()),
        };
        if let Some(leaf) = root.and_then(|r| burn_leaf(params, r)) {
            candidates.push((u64::from(i), leaf));
        }
    }
    // `reverted`: strictly ascending, each a user transaction (the last two
    // are the forced fee and clock transactions).
    let users = u64::from(n.saturating_sub(2));
    let m = c.u32()?;
    let mut reverted = Vec::with_capacity(m as usize);
    for _ in 0..m {
        let r = c.u64()?;
        if r >= users || reverted.last().is_some_and(|p| *p >= r) {
            return Err(());
        }
        reverted.push(r);
    }
    if c.u8()? > 2 {
        return Err(()); // bedrock_status
    }
    if c.i != raw.len() {
        return Err(());
    }
    let leaves = candidates.into_iter().filter(|(i, _)| reverted.binary_search(i).is_err()).map(|(_, l)| l).collect();
    Ok((Header { block_id, prev_block_hash, hash }, leaves))
}

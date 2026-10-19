//! The zero-knowledge release (feature `zk`): the statement the guest
//! `zk/burn_membership` proves, as plain functions, and its input. Kept out of
//! the default build so that wrap_bridge.bin, which links this crate, stays
//! byte for byte the committed, reproducible build.

use borsh::{BorshDeserialize, BorshSerialize};

use crate::*;

/// The 352 bytes `WrapVault.releaseDigest` hashes: `abi.encode(RELEASE_TAG,
/// chainid, vault, PAIRING, token, amount, recipient, relayer, fee,
/// lezBurnRoot, nullifier)`, eleven words. It is also the journal of the
/// zero-knowledge release guest (`zk/burn_membership`), so the digest a RISC
/// Zero receipt commits to, `sha256(journal)`, is the vault's own digest.
#[allow(clippy::too_many_arguments)]
pub fn release_preimage(
    chain_id: u64,
    vault: &[u8; 20],
    pairing: &H256,
    token: &[u8; 20],
    amount: u128,
    recipient: &[u8; 20],
    relayer: &[u8; 20],
    fee: u128,
    lez_burn_root: &H256,
    nullifier: &H256,
) -> Vec<u8> {
    [
        RELEASE_TAG,
        word_u64(chain_id),
        word_addr(vault),
        *pairing,
        word_addr(token),
        word_u128(amount),
        word_addr(recipient),
        word_addr(relayer),
        word_u128(fee),
        *lez_burn_root,
        *nullifier,
    ]
    .concat()
}

/// What the zero-knowledge release guest reads. The first four fields are the
/// release's context, public in its journal; the rest is the burn's opening,
/// which stays in the prover: the secret, the leaf's index and its path, and
/// with them which burn this is.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ZkReleaseInput {
    pub chain_id: u64,
    pub vault: [u8; 20],
    pub pairing: H256,
    pub relayer: [u8; 20],
    pub token: [u8; 20],
    pub amount: u128,
    pub recipient: [u8; 20],
    pub fee: u128,
    pub secret: H256,
    pub index: u64,
    pub path: Vec<H256>,
}

/// The statement the zero-knowledge release guest proves, as a function: the
/// burn leaf the opening describes (`burn_leaf` of the token, the amount and
/// `eth_commitment(recipient, fee, secret)`) sits at `index` under the root
/// its path leads to; the journal is the release preimage of that root and of
/// the burn's nullifier. `None` if the path is malformed.
pub fn zk_release_journal(i: &ZkReleaseInput) -> Option<Vec<u8>> {
    let leaf = burn_leaf(&i.pairing, &i.token, i.amount, &eth_commitment(&i.recipient, i.fee, &i.secret));
    let root = root_from_path(&leaf, i.index, &i.path)?;
    let nullifier = burn_nullifier(&i.pairing, &i.secret, i.index);
    Some(release_preimage(i.chain_id, &i.vault, &i.pairing, &i.token, i.amount, &i.recipient, &i.relayer, i.fee, &root, &nullifier))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guest's journal is the release preimage of the root and nullifier
    /// the opening leads to: its sha256 is the vault's release digest.
    #[test]
    fn zk_journal_is_the_release_preimage() {
        let a = |b: u8| {
            let mut x = [0u8; 20];
            x[19] = b;
            x
        };
        let (pairing, secret) = ([0x11; 32], [0x55; 32]);
        let mut leaves: Vec<H256> = (0u8..3).map(|i| sha256(&[&[i]])).collect();
        let me = burn_leaf(&pairing, &a(2), 7, &eth_commitment(&a(3), 1, &secret));
        leaves.insert(1, me);
        let input = ZkReleaseInput {
            chain_id: 31337, vault: a(1), pairing, relayer: a(4), token: a(2), amount: 7, recipient: a(3), fee: 1,
            secret, index: 1, path: path(&leaves, 1),
        };
        let j = zk_release_journal(&input).unwrap();
        assert_eq!(j.len(), 352);
        let r = root(&leaves);
        let nf = burn_nullifier(&pairing, &secret, 1);
        assert_eq!(sha256(&[&j]), release_digest(31337, &a(1), &pairing, &a(2), 7, &a(3), &a(4), 1, &r, &nf));
        assert_eq!(&j[288..320], &r);
        // Another index, another recipient or a short path does not prove this root.
        let mut bad = input.clone();
        bad.index = 2;
        assert_ne!(&zk_release_journal(&bad).unwrap()[288..320], &r);
        let mut bad = input.clone();
        bad.recipient = a(9);
        assert_ne!(&zk_release_journal(&bad).unwrap()[288..320], &r);
        let mut bad = input;
        bad.path.pop();
        assert_eq!(zk_release_journal(&bad), None);
    }
}

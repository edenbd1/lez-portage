//! wrap-core: what the Ethereum vault and the LEZ bridge program must compute
//! identically.
//!
//! Inbound, a deposit appends
//!
//! ```text
//! leaf = sha256(abi.encode(LEAF_TAG, chain_id, vault, pairing, token, amount, commitment))
//! commitment = sha256(COMMIT_TAG ‖ pairing ‖ destination ‖ secret)
//! ```
//!
//! to the vault's depth-24 SHA-256 tree, whose every root the vault records at
//! `knownRoot[root]` (storage slot `keccak(root ‖ 0)`). A claim on LEZ proves
//! that slot holds 1 under a header Sextant finalized, and that its leaf is in
//! that root; it spends `nullifier(pairing, secret, index)`. The destination
//! and the secret appear on neither chain; only the holder of the secret can
//! claim, and only to the destination it committed to.
//!
//! Outbound, a burn appends `burn_leaf(pairing, token, amount, eth_commitment)`
//! to the program's own tree, where `eth_commitment` hides the Ethereum
//! recipient the same way.
//!
//! Secrets are derived from the wallet seed (`derive_secret`), so a client
//! that lost every local file recovers its positions from the seed and public
//! events alone (F.9, R.5).

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};
use tiny_keccak::{Hasher, Keccak};

pub type H256 = [u8; 32];

/// Depth of both trees: 16,777,216 leaves.
pub const DEPTH: usize = 24;

/// A Solidity `bytes32` string literal: the bytes, left-aligned, zero-padded.
pub const fn tag(s: &[u8]) -> H256 {
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < s.len() {
        out[i] = s[i];
        i += 1;
    }
    out
}

pub const LEAF_TAG: H256 = tag(b"rfp022/wrap/leaf/v1");
pub const RELEASE_TAG: H256 = tag(b"rfp022/wrap/release/v1");
pub const PAIR_TAG: &[u8] = b"rfp022/pair/v1";
pub const COMMIT_TAG: &[u8] = b"rfp022/commit/v1";
pub const NULLIFIER_TAG: &[u8] = b"rfp022/nullifier/v1";
pub const BURN_TAG: &[u8] = b"rfp022/burn/v1";
pub const ETH_COMMIT_TAG: H256 = tag(b"rfp022/eth-commit/v1");
pub const BURN_NULLIFIER_TAG: &[u8] = b"rfp022/burn-nullifier/v1";
pub const SECRET_TAG: &[u8] = b"rfp022/secret/v1";

/// The vault's storage slot of `mapping(bytes32 => bool) knownRoot`.
pub const KNOWN_ROOTS_SLOT: u64 = 0;

pub fn sha256(parts: &[&[u8]]) -> H256 {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

pub fn keccak(parts: &[&[u8]]) -> H256 {
    let mut k = Keccak::v256();
    for p in parts {
        k.update(p);
    }
    let mut out = [0u8; 32];
    k.finalize(&mut out);
    out
}

/// A `uint256` ABI word, big-endian.
pub fn word_u128(v: u128) -> H256 {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&v.to_be_bytes());
    w
}

pub fn word_u64(v: u64) -> H256 {
    word_u128(u128::from(v))
}

/// An `address` ABI word.
pub fn word_addr(a: &[u8; 20]) -> H256 {
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(a);
    w
}

pub fn hash2(a: &H256, b: &H256) -> H256 {
    sha256(&[a, b])
}

/// `zeros[i]`: the root of an empty subtree of height `i`; `zeros[DEPTH]` is the empty root.
pub fn zeros() -> [H256; DEPTH + 1] {
    let mut z = [[0u8; 32]; DEPTH + 1];
    for i in 1..=DEPTH {
        z[i] = hash2(&z[i - 1], &z[i - 1]);
    }
    z
}

/// The root a leaf at `index` with sibling `path` (bottom up) leads to, or
/// `None` if the path is not exactly `DEPTH` long or the index is out of range.
pub fn root_from_path(leaf: &H256, index: u64, path: &[H256]) -> Option<H256> {
    if path.len() != DEPTH || index >= 1u64 << DEPTH {
        return None;
    }
    let mut node = *leaf;
    for (level, sib) in path.iter().enumerate() {
        node = if (index >> level) & 1 == 1 { hash2(sib, &node) } else { hash2(&node, sib) };
    }
    Some(node)
}

/// One LEZ bridge deployment, as the vault pins it (F.15): the LEZ chain and
/// zone, the program's account id, and the deployment's own identifier.
pub fn pairing(lez_chain_id: &H256, zone_id: &H256, program_id: &H256, deployment_id: &H256) -> H256 {
    sha256(&[PAIR_TAG, lez_chain_id, zone_id, program_id, deployment_id])
}

/// The depositor's hiding commitment to their LEZ destination.
pub fn commitment(pairing: &H256, destination: &H256, secret: &H256) -> H256 {
    sha256(&[COMMIT_TAG, pairing, destination, secret])
}

/// `WrapVault.leafOf` for `(token, amount, commitment)` at `vault` on `chain_id`.
pub fn leaf(chain_id: u64, vault: &[u8; 20], pairing: &H256, token: &[u8; 20], amount: u128, commitment: &H256) -> H256 {
    sha256(&[&LEAF_TAG, &word_u64(chain_id), &word_addr(vault), pairing, &word_addr(token), &word_u128(amount), commitment])
}

/// The one nullifier a deposit can be claimed with: unique per tree position,
/// and unlinkable to it without the secret.
pub fn nullifier(pairing: &H256, secret: &H256, index: u64) -> H256 {
    sha256(&[NULLIFIER_TAG, pairing, secret, &index.to_le_bytes()])
}

/// The storage slot of `knownRoot[root]`: `keccak(root ‖ uint256(0))`.
pub fn known_root_slot(root: &H256) -> H256 {
    keccak(&[root, &word_u64(KNOWN_ROOTS_SLOT)])
}

/// The burner's hiding commitment to their Ethereum recipient and to the fee
/// a relayer may take from the release. A release opens it on Ethereum, so
/// the fee is fixed here, at burn time: whoever submits the release can then
/// neither redirect it nor raise the fee (whoever relays earns that fee).
pub fn eth_commitment(recipient: &[u8; 20], fee: u128, secret: &H256) -> H256 {
    sha256(&[&ETH_COMMIT_TAG, &word_addr(recipient), &word_u128(fee), secret])
}

pub const ATTEST_TAG: H256 = tag(b"rfp022/burn-root-attestation/v1");

/// What each member of the release committee signs (EIP-191, over this
/// keccak digest): "the burn tree of the paired deployment had `root` in LEZ
/// state at block `lez_block`, and `lez_final_block` is the highest block I
/// consider final". `CommitteeBurnVerifier.attestationDigest` computes the same.
pub fn attestation_digest(chain_id: u64, vault: &[u8; 20], pairing: &H256, root: &H256, lez_block: u64, lez_final_block: u64) -> H256 {
    keccak(&[&ATTEST_TAG, &word_u64(chain_id), &word_addr(vault), pairing, root, &word_u64(lez_block), &word_u64(lez_final_block)])
}

pub fn burn_leaf(pairing: &H256, token: &[u8; 20], amount: u128, eth_commitment: &H256) -> H256 {
    sha256(&[BURN_TAG, pairing, token, &amount.to_le_bytes(), eth_commitment])
}

pub fn burn_nullifier(pairing: &H256, secret: &H256, index: u64) -> H256 {
    sha256(&[BURN_NULLIFIER_TAG, pairing, secret, &index.to_le_bytes()])
}

/// `WrapVault.releaseDigest`: the public inputs a release proof commits to.
#[allow(clippy::too_many_arguments)]
pub fn release_digest(
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
) -> H256 {
    sha256(&[
        &RELEASE_TAG,
        &word_u64(chain_id),
        &word_addr(vault),
        pairing,
        &word_addr(token),
        &word_u128(amount),
        &word_addr(recipient),
        &word_addr(relayer),
        &word_u128(fee),
        lez_burn_root,
        nullifier,
    ])
}

/// What a position's secret is derived from: the wallet seed, the direction
/// (`b'd'` deposit, `b'b'` burn) and a counter. No secret is ever generated at
/// random, so none needs a backup.
pub fn derive_secret(seed: &H256, kind: u8, n: u64) -> H256 {
    sha256(&[SECRET_TAG, seed, &[kind], &n.to_le_bytes()])
}

/// An append-only tree that keeps only its frontier: the vault's algorithm,
/// and the LEZ program's burn tree.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Frontier {
    pub next_index: u64,
    pub filled: Vec<H256>,
    pub root: H256,
}

impl Default for Frontier {
    fn default() -> Self {
        let z = zeros();
        Self { next_index: 0, filled: z[..DEPTH].to_vec(), root: z[DEPTH] }
    }
}

impl Frontier {
    /// Append `leaf`; `None` when the tree is full.
    pub fn insert(&mut self, leaf: &H256) -> Option<u64> {
        let index = self.next_index;
        if index >= 1u64 << DEPTH {
            return None;
        }
        let z = zeros();
        let mut node = *leaf;
        let mut i = index;
        for level in 0..DEPTH {
            if i & 1 == 0 {
                self.filled[level] = node;
                node = hash2(&node, &z[level]);
            } else {
                node = hash2(&self.filled[level], &node);
            }
            i >>= 1;
        }
        self.next_index = index + 1;
        self.root = node;
        Some(index)
    }
}

/// The sibling path of `index` in the tree holding `leaves` (client side).
pub fn path(leaves: &[H256], index: usize) -> Vec<H256> {
    let z = zeros();
    let mut layer = leaves.to_vec();
    let mut i = index;
    let mut out = Vec::with_capacity(DEPTH);
    for level in 0..DEPTH {
        let sib = i ^ 1;
        out.push(layer.get(sib).copied().unwrap_or(z[level]));
        layer = layer.chunks(2).map(|p| hash2(&p[0], p.get(1).unwrap_or(&z[level]))).collect();
        i >>= 1;
    }
    out
}

/// The root of the tree holding `leaves` (client side).
pub fn root(leaves: &[H256]) -> H256 {
    let mut f = Frontier::default();
    for l in leaves {
        f.insert(l);
    }
    f.root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> H256 {
        hex::decode(s).unwrap().try_into().unwrap()
    }

    #[test]
    fn tags_are_solidity_bytes32_literals() {
        assert_eq!(&LEAF_TAG[..19], b"rfp022/wrap/leaf/v1");
        assert!(LEAF_TAG[19..].iter().all(|b| *b == 0));
    }

    #[test]
    fn frontier_and_paths_agree_for_every_prefix() {
        let leaves: Vec<H256> = (0u8..9).map(|i| sha256(&[&[i]])).collect();
        let mut f = Frontier::default();
        assert_eq!(f.root, zeros()[DEPTH]);
        for n in 0..leaves.len() {
            assert_eq!(f.insert(&leaves[n]), Some(n as u64));
            let prefix = &leaves[..=n];
            for (i, l) in prefix.iter().enumerate() {
                assert_eq!(root_from_path(l, i as u64, &path(prefix, i)), Some(f.root));
            }
        }
        // A wrong index or a wrong leaf does not lead to the root.
        let p = path(&leaves, 3);
        assert_ne!(root_from_path(&leaves[3], 2, &p), Some(f.root));
        assert_ne!(root_from_path(&leaves[2], 3, &p), Some(f.root));
        assert_eq!(root_from_path(&leaves[3], 3, &p[1..]), None);
        assert_eq!(root_from_path(&leaves[3], 1 << DEPTH, &p), None);
    }

    /// The empty root of the depth-24 SHA-256 tree; the same constant is
    /// asserted against `WrapVault.root()` after deployment in
    /// vault/test/WrapVault.t.sol (`test_empty_root_constant_is_shared_with_rust`).
    #[test]
    fn empty_root_is_the_vaults() {
        assert_eq!(zeros()[DEPTH], h("31206fa80a50bb6abe29085058f16212212a60eec8f049fecb92d8c8e0a84bc0"));
    }

    /// The same leaf as vault/test/WrapVault.t.sol
    /// (`test_leaf_constant_is_shared_with_rust`): chain 31337, vault 0x…01,
    /// pairing 0x11…, token 0x…02, amount 100e6, commitment 0x22….
    #[test]
    fn leaf_is_the_vaults() {
        let mut vault = [0u8; 20];
        vault[19] = 1;
        let mut token = [0u8; 20];
        token[19] = 2;
        let got = leaf(31337, &vault, &[0x11; 32], &token, 100_000_000, &[0x22; 32]);
        assert_eq!(got, h(LEAF_VECTOR));
    }

    /// The same digest as vault/test/WrapVault.t.sol
    /// (`test_release_digest_constant_is_shared_with_rust`).
    #[test]
    fn release_digest_is_the_vaults() {
        let a = |b: u8| {
            let mut x = [0u8; 20];
            x[19] = b;
            x
        };
        let got = release_digest(31337, &a(1), &[0x11; 32], &a(2), 1_000_000, &a(3), &a(4), 5, &[0x33; 32], &[0x44; 32]);
        assert_eq!(got, h(RELEASE_VECTOR));
    }

    /// The same burn-side hashes as vault/test/CommitteeBurnVerifier.t.sol
    /// (`test_burn_hashes_are_shared_with_rust`): pairing 0x11…, token 0x…02,
    /// 1 ETH, recipient 0x…03, fee 5, secret 0x55…, index 7.
    #[test]
    fn burn_hashes_are_the_verifiers() {
        let a = |b: u8| {
            let mut x = [0u8; 20];
            x[19] = b;
            x
        };
        let ec = eth_commitment(&a(3), 5, &[0x55; 32]);
        let leaf = burn_leaf(&[0x11; 32], &a(2), 1_000_000_000_000_000_000, &ec);
        let nf = burn_nullifier(&[0x11; 32], &[0x55; 32], 7);
        let att = attestation_digest(31337, &a(1), &[0x11; 32], &[0x33; 32], 40, 43);
        let got = [ec, leaf, nf, att].map(hex::encode);
        assert_eq!(got, BURN_VECTORS, "{got:?}");
    }

    pub const BURN_VECTORS: [&str; 4] = [
        "bbc8856a71249b9f3254eb8b6e962896d2f85388b584979cb7bf38358baba7b4",
        "91a62580a02a2a83385299bd6ee8516d079cf61deee0bba79c6ace84a44a5ab8",
        "c191ff0e20e847734710bc669edc21fcb03d0fd5b4eea2c1b603a9269eefb9ab",
        "246310dd736d66ce3cb00c7d265f7d4556e56779f880e799f062f1c574b308c3",
    ];

    pub const RELEASE_VECTOR: &str = "f6c5b41d69d736ebe3c139a70d2172fdf781b356f13abe0308c7694f7dd2feec";

    pub const LEAF_VECTOR: &str = "bbd9c2571ff327c42320ed455c820d85534eb3f417852e6c8179bc9379221c43";

    #[test]
    fn nullifiers_and_commitments_separate_their_inputs() {
        let p = [1u8; 32];
        let s = [2u8; 32];
        assert_ne!(nullifier(&p, &s, 0), nullifier(&p, &s, 1));
        assert_ne!(nullifier(&p, &s, 0), nullifier(&[3; 32], &s, 0));
        assert_ne!(commitment(&p, &[4; 32], &s), commitment(&p, &[5; 32], &s));
        assert_ne!(derive_secret(&s, b'd', 0), derive_secret(&s, b'b', 0));
    }

    #[test]
    fn known_root_slot_is_the_solidity_mapping_slot() {
        // keccak256(abi.encode(bytes32(0), uint256(0))), a well-known value.
        assert_eq!(known_root_slot(&[0u8; 32]), h("ad3228b676f7d3cd4284a5443f17f1962b36e491b30a40b2405849e597ba5fb5"));
    }
}

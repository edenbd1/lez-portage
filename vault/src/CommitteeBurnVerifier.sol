// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {IBurnVerifier} from "./IBurnVerifier.sol";

/// The two views of the vault this verifier reads: the pairing it is deployed with, and the
/// digest it asks a proof to commit to.
interface IReleasingVault {
    function PAIRING() external view returns (bytes32);
    function releaseDigest(
        address token,
        uint256 amount,
        address recipient,
        address relayer,
        uint256 fee,
        bytes32 lezBurnRoot,
        bytes32 nullifier
    ) external view returns (bytes32);
}

/// @title CommitteeBurnVerifier: the vault's release check (F.7) as far as it can be built today.
///
/// A release must show that a LEZ burn of (token, amount) to this recipient, with this relayer fee,
/// is final on LEZ and was not released before. This verifier splits that in two:
///
/// 1. **The burn's membership is checked here, cryptographically.** The proof opens the burn leaf
///    the LEZ program appended (`wrap_core::burn_leaf`): the token, the amount, and the Ethereum
///    commitment `sha256(tag ‖ recipient ‖ fee ‖ secret)`, at `index` with its depth-24 SHA-256
///    path. The root it leads to and the nullifier `sha256(tag ‖ pairing ‖ secret ‖ index)` are
///    computed here, and must be the ones the vault's `releaseDigest` was called with: the vault
///    then spends exactly that nullifier, so each leaf releases once, and neither the recipient,
///    the amount nor the fee can differ from what the burner committed to on LEZ.
///
/// 2. **That the root is the LEZ burn tree's, in final LEZ state, is attested, not proven.** A
///    fixed committee of `members` signs (EIP-191) `attestationDigest(vault, pairing, root,
///    lezBlock, lezFinalBlock)`: "the paired deployment's burn tree had this root at LEZ block
///    `lezBlock`, and `lezFinalBlock` is the highest block I consider final". `THRESHOLD` distinct
///    members must sign, and `lezFinalBlock` must be at least `FINALITY_DEPTH` blocks above
///    `lezBlock` (F.12).
///
/// **Trust assumption, stated plainly.** Part 2 is a `THRESHOLD`-of-n committee: if that many
/// members sign a root the burn tree never had, they can release the vault's funds against burns
/// that never happened. Nothing on Ethereum can check LEZ state today: LEZ v0.3 block headers carry
/// no state root and, since v0.3.1, no producer signature, and no Cryptarchia (Bedrock) light
/// client or LEZ validity prover exists for Ethereum. The committee stands in for both: finality
/// on Bedrock, and the burn tree's root in the state that finality covers. What replaces it is a
/// verifier of the same interface (`IBurnVerifier`), fixed in a new vault at construction, never
/// swapped into this one (Security #8).
///
/// **Privacy cost, stated plainly.** The opening is public on Ethereum: the release names its
/// burn leaf, so it is linkable to the LEZ transaction that appended it. For a public burn that
/// transaction names the burner's LEZ account; for a private burn (`BurnFrom` in a
/// privacy-preserving transaction) it names no account, but its block, token and amount are
/// public. Hiding the leaf needs the same check as a zero-knowledge proof (a RISC Zero receipt
/// behind a Groth16 verifier), which is not built.
contract CommitteeBurnVerifier is IBurnVerifier {
    bytes32 public constant ATTEST_TAG = "rfp022/burn-root-attestation/v1";
    bytes32 public constant ETH_COMMIT_TAG = "rfp022/eth-commit/v1";
    uint256 public constant DEPTH = 24;

    /// What a release proof carries: the burn's opening, the LEZ blocks the attestation names, and
    /// the members' signatures over it.
    struct Release {
        address token;
        uint256 amount;
        address recipient;
        address relayer;
        uint256 fee;
        bytes32 secret;
        uint64 index;
        bytes32[DEPTH] path;
        uint64 lezBlock;
        uint64 lezFinalBlock;
        bytes[] signatures;
    }

    uint256 public immutable THRESHOLD;
    uint64 public immutable FINALITY_DEPTH;
    address[] private members_;
    mapping(address => uint256) public memberIndex; // 1-based; 0 = not a member

    error BadConfig();
    error NotThisRelease();
    error BadOpening();
    error NotFinal();
    error RootNotAttested();

    constructor(address[] memory members, uint256 threshold, uint64 finalityDepth) {
        if (threshold == 0 || threshold > members.length || members.length > 256) revert BadConfig();
        for (uint256 i; i < members.length; ++i) {
            if (members[i] == address(0) || memberIndex[members[i]] != 0) revert BadConfig();
            memberIndex[members[i]] = i + 1;
            members_.push(members[i]);
        }
        THRESHOLD = threshold;
        FINALITY_DEPTH = finalityDepth;
    }

    function members() external view returns (address[] memory) {
        return members_;
    }

    // ---- the hashes wrap-core computes (core/src/lib.rs), byte for byte ----

    function ethCommitment(address recipient, uint256 fee, bytes32 secret) public pure returns (bytes32) {
        return sha256(abi.encode(ETH_COMMIT_TAG, recipient, fee, secret));
    }

    /// `sha256("rfp022/burn/v1" ‖ pairing ‖ token (20 bytes) ‖ amount (u128, little-endian) ‖ ethCommitment)`.
    function burnLeaf(bytes32 pairing, address token, uint256 amount, bytes32 ethCommitment_) public pure returns (bytes32) {
        if (amount > type(uint128).max) revert BadOpening();
        return sha256(abi.encodePacked("rfp022/burn/v1", pairing, token, _le(amount, 16), ethCommitment_));
    }

    /// `sha256("rfp022/burn-nullifier/v1" ‖ pairing ‖ secret ‖ index (u64, little-endian))`.
    function burnNullifier(bytes32 pairing, bytes32 secret, uint64 index) public pure returns (bytes32) {
        return sha256(abi.encodePacked("rfp022/burn-nullifier/v1", pairing, secret, _le(index, 8)));
    }

    function rootFromPath(bytes32 leaf, uint64 index, bytes32[DEPTH] memory path) public pure returns (bytes32 node) {
        if (index >= 1 << DEPTH) revert BadOpening();
        node = leaf;
        for (uint256 level; level < DEPTH; ++level) {
            node = (index >> level) & 1 == 1 ? sha256(abi.encodePacked(path[level], node)) : sha256(abi.encodePacked(node, path[level]));
        }
    }

    /// What each member signs, under EIP-191: bound to this chain, the vault and its pairing.
    function attestationDigest(address vault, bytes32 pairing, bytes32 root, uint64 lezBlock, uint64 lezFinalBlock)
        public
        view
        returns (bytes32)
    {
        return keccak256(abi.encode(ATTEST_TAG, block.chainid, vault, pairing, root, uint256(lezBlock), uint256(lezFinalBlock)));
    }

    // ---- IBurnVerifier, called by the vault ----

    function verify(bytes calldata proof, bytes32 publicInputs) external view returns (bool) {
        Release memory r = abi.decode(proof, (Release));
        IReleasingVault vault = IReleasingVault(msg.sender);
        bytes32 pairing = vault.PAIRING();

        // 1. The opening: the leaf, its root, its nullifier, and the release the vault asks about.
        bytes32 leaf = burnLeaf(pairing, r.token, r.amount, ethCommitment(r.recipient, r.fee, r.secret));
        bytes32 root = rootFromPath(leaf, r.index, r.path);
        bytes32 nullifier = burnNullifier(pairing, r.secret, r.index);
        if (vault.releaseDigest(r.token, r.amount, r.recipient, r.relayer, r.fee, root, nullifier) != publicInputs) {
            revert NotThisRelease();
        }

        // 2. The root: final, and attested by THRESHOLD distinct members.
        if (uint256(r.lezFinalBlock) < uint256(r.lezBlock) + FINALITY_DEPTH) revert NotFinal();
        bytes32 signed = keccak256(
            abi.encodePacked("\x19Ethereum Signed Message:\n32", attestationDigest(msg.sender, pairing, root, r.lezBlock, r.lezFinalBlock))
        );
        uint256 seen; // bitmap of members counted (at most 256)
        uint256 count;
        for (uint256 i; i < r.signatures.length; ++i) {
            uint256 m = memberIndex[_recover(signed, r.signatures[i])];
            if (m == 0 || seen & (1 << (m - 1)) != 0) continue;
            seen |= 1 << (m - 1);
            ++count;
        }
        if (count < THRESHOLD) revert RootNotAttested();
        return true;
    }

    /// The signer of a 65-byte (r, s, v) signature, or address(0); high-s and other v are refused.
    function _recover(bytes32 h, bytes memory sig) internal pure returns (address) {
        if (sig.length != 65) return address(0);
        bytes32 r;
        bytes32 s;
        uint8 v;
        assembly {
            r := mload(add(sig, 32))
            s := mload(add(sig, 64))
            v := byte(0, mload(add(sig, 96)))
        }
        if (uint256(s) > 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0 || (v != 27 && v != 28)) {
            return address(0);
        }
        return ecrecover(h, v, r, s);
    }

    /// The low `n` bytes of `v`, least significant first.
    function _le(uint256 v, uint256 n) internal pure returns (bytes memory out) {
        out = new bytes(n);
        for (uint256 i; i < n; ++i) {
            out[i] = bytes1(uint8(v >> (8 * i)));
        }
    }
}

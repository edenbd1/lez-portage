// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

/// @title BurnRootCommittee: who says a LEZ burn-tree root is final, in both release verifiers.
///
/// A fixed committee of `members` signs (EIP-191) `attestationDigest(vault, pairing, root,
/// lezBlock, lezFinalBlock)`: "the paired deployment's burn tree had this root at LEZ block
/// `lezBlock`, and `lezFinalBlock` is the highest block I consider final". `THRESHOLD` distinct
/// members must sign, and `lezFinalBlock` must be at least `FINALITY_DEPTH` blocks above
/// `lezBlock` (F.12).
///
/// **Trust assumption, stated plainly.** This is a `THRESHOLD`-of-n committee: if that many members
/// sign a root the burn tree never had, they can release the vault's funds against burns that
/// never happened. Nothing on Ethereum can check LEZ state today: LEZ v0.3 block headers carry no
/// state root and, since v0.3.1, no producer signature, and no Cryptarchia (Bedrock) light client
/// or LEZ validity prover exists for Ethereum. The committee stands in for both: finality on
/// Bedrock, and the burn tree's root in the state that finality covers. What replaces it is a
/// verifier of the same interface (`IBurnVerifier`), fixed in a new vault at construction, never
/// swapped into this one (Security #8).
abstract contract BurnRootCommittee {
    bytes32 public constant ATTEST_TAG = "rfp022/burn-root-attestation/v1";

    uint256 public immutable THRESHOLD;
    uint64 public immutable FINALITY_DEPTH;
    address[] private members_;
    mapping(address => uint256) public memberIndex; // 1-based; 0 = not a member

    error BadConfig();
    error NotFinal();
    error RootNotAttested();

    constructor(address[] memory committee, uint256 threshold, uint64 finalityDepth) {
        if (threshold == 0 || threshold > committee.length || committee.length > 256) revert BadConfig();
        for (uint256 i; i < committee.length; ++i) {
            if (committee[i] == address(0) || memberIndex[committee[i]] != 0) revert BadConfig();
            memberIndex[committee[i]] = i + 1;
            members_.push(committee[i]);
        }
        THRESHOLD = threshold;
        FINALITY_DEPTH = finalityDepth;
    }

    function members() external view returns (address[] memory) {
        return members_;
    }

    /// What each member signs, under EIP-191: bound to this chain, the vault and its pairing.
    function attestationDigest(address vault, bytes32 pairing, bytes32 root, uint64 lezBlock, uint64 lezFinalBlock)
        public
        view
        returns (bytes32)
    {
        return keccak256(abi.encode(ATTEST_TAG, block.chainid, vault, pairing, root, uint256(lezBlock), uint256(lezFinalBlock)));
    }

    /// Reverts unless `root` is final by `FINALITY_DEPTH` and attested by `THRESHOLD` distinct members.
    function _requireAttested(
        address vault,
        bytes32 pairing,
        bytes32 root,
        uint64 lezBlock,
        uint64 lezFinalBlock,
        bytes[] memory signatures
    ) internal view {
        if (uint256(lezFinalBlock) < uint256(lezBlock) + FINALITY_DEPTH) revert NotFinal();
        bytes32 signed = keccak256(
            abi.encodePacked("\x19Ethereum Signed Message:\n32", attestationDigest(vault, pairing, root, lezBlock, lezFinalBlock))
        );
        uint256 seen; // bitmap of members counted (at most 256)
        uint256 count;
        for (uint256 i; i < signatures.length; ++i) {
            uint256 m = memberIndex[_recover(signed, signatures[i])];
            if (m == 0 || seen & (1 << (m - 1)) != 0) continue;
            seen |= 1 << (m - 1);
            ++count;
        }
        if (count < THRESHOLD) revert RootNotAttested();
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
}

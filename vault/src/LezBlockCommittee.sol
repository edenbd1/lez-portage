// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

/// @title LezBlockCommittee: who says a LEZ block is final, in the chain-derived release verifier.
///
/// A fixed committee of `members` signs (EIP-191) `blockAttestationDigest(vault, pairing, blockHash,
/// lezBlock, lezFinalBlock)`: "the LEZ block `lezBlock` of the paired deployment's chain has hash
/// `blockHash`, and `lezFinalBlock` is the highest LEZ block I consider final on Bedrock".
/// `THRESHOLD` distinct members must sign, and `lezFinalBlock` must be at least `FINALITY_DEPTH`
/// blocks above `lezBlock` (F.12).
///
/// **What this committee no longer vouches for.** `BurnRootCommittee` signs a burn-tree root: LEZ
/// state, which only the committee's word ties to anything. This one signs a block hash, and the
/// root is derived from the blocks under it inside the proof (`zk/lez_chain`), so the committee
/// cannot name a root. A block hash is also a public, checkable fact: the sequencer inscribes the
/// whole block on its Bedrock channel, and anyone running a Bedrock node can see whether a block
/// with this hash is at this height of the channel's finalized messages.
///
/// **Trust assumption, stated plainly.** `THRESHOLD` members signing the hash of a block that is
/// not on the final chain (one built by whoever holds the sequencer's channel key, or made up)
/// can still release against burns that never happened: the proof follows whatever hash chain
/// leads to the hash they sign. What replaces this is a proof of the block's inscription in
/// Bedrock's finalized chain (docs/BEDROCK-FINALITY.md), behind the same `IBurnVerifier`, in a
/// new vault.
abstract contract LezBlockCommittee {
    bytes32 public constant BLOCK_ATTEST_TAG = "rfp022/lez-block-final/v1";

    uint256 public immutable THRESHOLD;
    uint64 public immutable FINALITY_DEPTH;
    address[] private members_;
    mapping(address => uint256) public memberIndex; // 1-based; 0 = not a member

    error BadConfig();
    error NotFinal();
    error BlockNotAttested();

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
    function blockAttestationDigest(address vault, bytes32 pairing, bytes32 blockHash, uint64 lezBlock, uint64 lezFinalBlock)
        public
        view
        returns (bytes32)
    {
        return keccak256(
            abi.encode(BLOCK_ATTEST_TAG, block.chainid, vault, pairing, blockHash, uint256(lezBlock), uint256(lezFinalBlock))
        );
    }

    /// Reverts unless `blockHash` at `lezBlock` is final by `FINALITY_DEPTH` and attested by
    /// `THRESHOLD` distinct members.
    function _requireFinalBlock(
        address vault,
        bytes32 pairing,
        bytes32 blockHash,
        uint64 lezBlock,
        uint64 lezFinalBlock,
        bytes[] memory signatures
    ) internal view {
        if (uint256(lezFinalBlock) < uint256(lezBlock) + FINALITY_DEPTH) revert NotFinal();
        bytes32 signed = keccak256(
            abi.encodePacked(
                "\x19Ethereum Signed Message:\n32", blockAttestationDigest(vault, pairing, blockHash, lezBlock, lezFinalBlock)
            )
        );
        uint256 seen; // bitmap of members counted (at most 256)
        uint256 count;
        for (uint256 i; i < signatures.length; ++i) {
            uint256 m = memberIndex[_recover(signed, signatures[i])];
            if (m == 0 || seen & (1 << (m - 1)) != 0) continue;
            seen |= 1 << (m - 1);
            ++count;
        }
        if (count < THRESHOLD) revert BlockNotAttested();
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

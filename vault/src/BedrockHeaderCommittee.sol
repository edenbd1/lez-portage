// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

/// @title BedrockHeaderCommittee: who says a Bedrock header is canonical, in the Bedrock-anchored release verifier.
///
/// A fixed committee of `members` signs (EIP-191) `headerAttestationDigest(vault, pairing, headerId)`:
/// "the Bedrock block `headerId` is on Bedrock's canonical chain, as my node sees it". `THRESHOLD`
/// distinct members must sign.
///
/// **What this committee no longer vouches for.** `LezBlockCommittee` signs a LEZ block hash: that
/// the block is final is the committee's word. Here the release proof shows, inside, that the LEZ
/// block its tree is derived from is inscribed on the pinned channel by the pinned sequencer key,
/// in a Bedrock block with at least `MIN_DEPTH` headers built on it, the last of which is
/// `headerId`. So the committee no longer names a LEZ block, a root or a depth; it only says one
/// Bedrock header is canonical, which anyone running a Bedrock node can check
/// (`GET /cryptarchia/blocks/{headerId}` returns it, and it descends from the node's LIB or is
/// below it). If it is, the inscription's block is `MIN_DEPTH` deep under a canonical block, so
/// final by Bedrock's rule (the LIB is the k-th ancestor of the tip; forks deeper than k are
/// refused).
///
/// **Trust assumption, stated plainly.** `THRESHOLD` members signing a header that is not on
/// Bedrock's canonical chain (headers can be made up: nothing here checks a proof of leadership)
/// can still release against a LEZ block that was never inscribed, but only one signed by the
/// pinned sequencer key: the proof checks the inscription's signature. Forging a release needs
/// both the committee threshold and the sequencer's key. What replaces the committee is a proof
/// that the header is canonical: a Cryptarchia light client, which needs Bedrock's note-set
/// ledger (docs/BEDROCK-FINALITY.md).
abstract contract BedrockHeaderCommittee {
    bytes32 public constant HEADER_ATTEST_TAG = "rfp022/bedrock-header/v1";

    uint256 public immutable THRESHOLD;
    address[] private members_;
    mapping(address => uint256) public memberIndex; // 1-based; 0 = not a member

    error BadConfig();
    error HeaderNotAttested();

    constructor(address[] memory committee, uint256 threshold) {
        if (threshold == 0 || threshold > committee.length || committee.length > 256) revert BadConfig();
        for (uint256 i; i < committee.length; ++i) {
            if (committee[i] == address(0) || memberIndex[committee[i]] != 0) revert BadConfig();
            memberIndex[committee[i]] = i + 1;
            members_.push(committee[i]);
        }
        THRESHOLD = threshold;
    }

    function members() external view returns (address[] memory) {
        return members_;
    }

    /// What each member signs, under EIP-191: bound to this chain, the vault and its pairing.
    function headerAttestationDigest(address vault, bytes32 pairing, bytes32 headerId) public view returns (bytes32) {
        return keccak256(abi.encode(HEADER_ATTEST_TAG, block.chainid, vault, pairing, headerId));
    }

    /// Reverts unless `THRESHOLD` distinct members signed that `headerId` is canonical.
    function _requireCanonicalHeader(address vault, bytes32 pairing, bytes32 headerId, bytes[] memory signatures)
        internal
        view
    {
        bytes32 signed = keccak256(
            abi.encodePacked("\x19Ethereum Signed Message:\n32", headerAttestationDigest(vault, pairing, headerId))
        );
        uint256 seen; // bitmap of members counted (at most 256)
        uint256 count;
        for (uint256 i; i < signatures.length; ++i) {
            uint256 m = memberIndex[_recover(signed, signatures[i])];
            if (m == 0 || seen & (1 << (m - 1)) != 0) continue;
            seen |= 1 << (m - 1);
            ++count;
        }
        if (count < THRESHOLD) revert HeaderNotAttested();
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

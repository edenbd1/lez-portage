// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {IBurnVerifier} from "./IBurnVerifier.sol";
import {LezBlockCommittee} from "./LezBlockCommittee.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";

/// @title ZkChainBurnVerifier: the release check (F.7) with the burn-tree root derived from LEZ blocks.
///
/// `ZkBurnVerifier` proves a burn's membership in zero knowledge, but takes the root from a
/// committee. Here one Groth16 proof covers both the membership and where the root comes from:
///
/// 1. **The chain, proven.** The RISC Zero guest `zk/lez_chain` (image id `CHAIN_IMAGE_ID`) reads
///    LEZ v0.3.1 blocks as the sequencer inscribes them on Bedrock, from the block after
///    `ANCHOR_HASH` (height `ANCHOR_ID`, fixed at deployment) to a tip. It checks each block's
///    hash against its contents and its link to the previous one, and appends to a tree the burn
///    leaves of the bridge program (`PROGRAM_ID`) on its burn-tree account (`BURNS_ACCOUNT`) that
///    privacy-preserving transactions settled. On LEZ v0.3.1 a privacy-preserving transaction is
///    fee-exempt, and one whose settlement fails invalidates its block, so in a valid block each
///    of those leaves is in the bridge's burn tree. Public burns are left out: a public
///    transaction that reverted is still in its block. The guest can continue from an earlier
///    receipt of itself, verified inside.
/// 2. **The burn, proven in zero knowledge.** The guest `zk/burn_release` (image id `IMAGE_ID`)
///    verifies the chain receipt inside (under the image id the chain journal names for itself),
///    proves the burn's opening under the chain tree's root as `zk/burn_membership` does, and
///    commits the vault's 352-byte release preimage of that root, then the chain receipt's image
///    id, `PROGRAM_ID`, `BURNS_ACCOUNT`, the anchor and the tip. The chain receipt is resolved
///    before the Groth16 wrap, so `RISC0` checks one seal for both guests.
/// 3. **The tip, attested final** by the committee of `LezBlockCommittee`: a block hash, not a root.
///
/// **Trust assumption, stated plainly.** Parts 1 and 2 are cryptographic (RISC Zero's proof
/// system, Groth16 with RISC Zero's setup). They make the root a function of the LEZ blocks under
/// the tip. What is not proven: (a) that the tip is final on Bedrock: the committee says so;
/// (b) that the blocks are valid LEZ blocks (their private transactions' proofs, nullifiers and
/// commitment roots, their public effects): LEZ's sequencer checks them and LEZ nodes re-execute
/// them, and a block they would refuse can still be inscribed. A threshold of the committee
/// signing a block hash off the final chain, or a sequencer inscribing a block LEZ would refuse,
/// can release against burns that never happened. F.7 is met only with (a) proven
/// (docs/BEDROCK-FINALITY.md) and (b) accepted as LEZ's own security or proven by a validity
/// prover.
contract ZkChainBurnVerifier is LezBlockCommittee, IBurnVerifier {
    struct Release {
        bytes seal;
        bytes journal;
        uint64 lezFinalBlock;
        bytes[] signatures;
    }

    /// The release guest's journal: the 11-word release preimage, then 7 words.
    uint256 public constant JOURNAL_LEN = 18 * 32;

    IRiscZeroVerifier public immutable RISC0;
    bytes32 public immutable IMAGE_ID;
    bytes32 public immutable CHAIN_IMAGE_ID;
    bytes32 public immutable PROGRAM_ID;
    bytes32 public immutable BURNS_ACCOUNT;
    uint64 public immutable ANCHOR_ID;
    bytes32 public immutable ANCHOR_HASH;

    error NotThisRelease();
    error NotThisChain();

    constructor(
        IRiscZeroVerifier risc0,
        bytes32 imageId,
        bytes32 chainImageId,
        bytes32 programId,
        bytes32 burnsAccount,
        uint64 anchorId,
        bytes32 anchorHash,
        address[] memory committee,
        uint256 threshold,
        uint64 finalityDepth
    ) LezBlockCommittee(committee, threshold, finalityDepth) {
        if (
            address(risc0) == address(0) || imageId == bytes32(0) || chainImageId == bytes32(0) || programId == bytes32(0)
                || burnsAccount == bytes32(0) || anchorHash == bytes32(0)
        ) revert BadConfig();
        RISC0 = risc0;
        IMAGE_ID = imageId;
        CHAIN_IMAGE_ID = chainImageId;
        PROGRAM_ID = programId;
        BURNS_ACCOUNT = burnsAccount;
        ANCHOR_ID = anchorId;
        ANCHOR_HASH = anchorHash;
    }

    function verify(bytes calldata proof, bytes32 publicInputs) external view returns (bool) {
        Release memory r = abi.decode(proof, (Release));
        bytes memory j = r.journal;
        if (j.length != JOURNAL_LEN) revert NotThisRelease();

        // 1. Its first 352 bytes are the release the vault asks about.
        if (_sha256Prefix(j, 352) != publicInputs) revert NotThisRelease();

        // 2. This deployment's chain, from the pinned anchor, by the pinned chain guest.
        if (
            _word(j, 11) != CHAIN_IMAGE_ID || _word(j, 12) != PROGRAM_ID || _word(j, 13) != BURNS_ACCOUNT
                || _word(j, 14) != bytes32(uint256(ANCHOR_ID)) || _word(j, 15) != ANCHOR_HASH
        ) revert NotThisChain();

        // 3. The release guest proved the whole journal.
        RISC0.verify(r.seal, IMAGE_ID, sha256(j));

        // 4. The tip the root is as of: final, attested by THRESHOLD distinct members.
        uint256 tipId = uint256(_word(j, 16));
        if (tipId > type(uint64).max) revert NotThisChain();
        _requireFinalBlock(msg.sender, _word(j, 3), _word(j, 17), uint64(tipId), r.lezFinalBlock, r.signatures);
        return true;
    }

    /// sha256 of the first `n` bytes of `j`.
    function _sha256Prefix(bytes memory j, uint256 n) internal view returns (bytes32 d) {
        assembly {
            let ok := staticcall(gas(), 0x02, add(j, 32), n, 0x00, 0x20)
            if iszero(ok) { revert(0, 0) }
            d := mload(0x00)
        }
    }

    /// Word `i` of the journal.
    function _word(bytes memory j, uint256 i) internal pure returns (bytes32 w) {
        assembly {
            w := mload(add(j, add(32, mul(32, i))))
        }
    }
}

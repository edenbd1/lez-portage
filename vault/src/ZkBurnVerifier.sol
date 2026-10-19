// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {IBurnVerifier} from "./IBurnVerifier.sol";
import {BurnRootCommittee} from "./BurnRootCommittee.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";

/// @title ZkBurnVerifier: the vault's release check (F.7) with the burn's membership in zero knowledge.
///
/// A release must show that a LEZ burn of (token, amount) to this recipient, with this relayer fee,
/// is final on LEZ and was not released before. This verifier splits that in two, as
/// `CommitteeBurnVerifier` does, but the first part reveals nothing about which burn it is:
///
/// 1. **Membership, proven in zero knowledge.** The RISC Zero guest `zk/burn_membership` (image id
///    `IMAGE_ID`) reads the burn's opening (token, amount, recipient, fee, secret, leaf index and
///    depth-24 SHA-256 path) and the release's context (chain id, vault, pairing, relayer), and
///    commits as its journal the vault's release preimage: `abi.encode(RELEASE_TAG, chainid,
///    vault, pairing, token, amount, recipient, relayer, fee, root, nullifier)`, where `root` is
///    the root the path leads to and `nullifier = sha256(tag ‖ pairing ‖ secret ‖ index)`. So
///    `sha256(journal)` is exactly `WrapVault.releaseDigest(...)`, the `publicInputs` the vault
///    passes. Its receipt is compressed to Groth16 and checked by RISC Zero's own
///    `RiscZeroGroth16Verifier` (`RISC0`), against `IMAGE_ID` and that digest. The vault computes
///    the digest itself, from its chain id, its address, its pairing and the release's arguments:
///    a proof made for another vault, chain, pairing, recipient, relayer, amount, fee, root or
///    nullifier does not verify. The secret, the index, the path and the leaf stay with the
///    prover. The vault spends the nullifier, so each burn releases once.
///
/// 2. **The root, attested by the committee** of `BurnRootCommittee`, unchanged: the root is read
///    from the journal (word 9) and must be signed by `THRESHOLD` members as final by
///    `FINALITY_DEPTH` LEZ blocks.
///
/// **Trust assumption, stated plainly.** Part 1 is cryptographic (RISC Zero's STARK, its
/// recursion, and the Groth16 wrap with RISC Zero's trusted setup). Part 2 is still a committee:
/// `THRESHOLD` members signing a root the burn tree never had can release against burns that
/// never happened (they can prove membership in a tree of their own making). F.7 needs part 2
/// replaced by a proof of LEZ finality and state; nothing on Ethereum can check LEZ state today.
///
/// **What a release still reveals.** The journal is the release's arguments: the token, the
/// amount, the recipient, the relayer, the fee, the root and the nullifier. Not the leaf, its
/// index, or the LEZ transaction that appended it: the release is hidden among the burns of the
/// same token and amount under that root (the bridge only burns fixed denominations). The fee
/// and the recipient are public on Ethereum, as any payment is; the root dates the release to
/// after the burn.
contract ZkBurnVerifier is BurnRootCommittee, IBurnVerifier {
    /// What a release proof carries: the Groth16 seal (RISC Zero's 4-byte selector, then the
    /// proof), the guest's journal, the LEZ blocks the attestation names, and the members'
    /// signatures over it.
    struct Release {
        bytes seal;
        bytes journal;
        uint64 lezBlock;
        uint64 lezFinalBlock;
        bytes[] signatures;
    }

    IRiscZeroVerifier public immutable RISC0;
    bytes32 public immutable IMAGE_ID;

    error NotThisRelease();

    constructor(
        IRiscZeroVerifier risc0,
        bytes32 imageId,
        address[] memory committee,
        uint256 threshold,
        uint64 finalityDepth
    ) BurnRootCommittee(committee, threshold, finalityDepth) {
        if (address(risc0) == address(0) || imageId == bytes32(0)) revert BadConfig();
        RISC0 = risc0;
        IMAGE_ID = imageId;
    }

    function verify(bytes calldata proof, bytes32 publicInputs) external view returns (bool) {
        Release memory r = abi.decode(proof, (Release));

        // 1. The journal is the release the vault asks about, and the guest proved it.
        if (sha256(r.journal) != publicInputs) revert NotThisRelease();
        RISC0.verify(r.seal, IMAGE_ID, publicInputs); // reverts unless the seal proves IMAGE_ID wrote it

        // 2. Its root: final, and attested by THRESHOLD distinct members.
        _requireAttested(msg.sender, _word(r.journal, 3), _word(r.journal, 9), r.lezBlock, r.lezFinalBlock, r.signatures);
        return true;
    }

    /// Word `i` of the journal (a 352-byte release preimage: its digest matched the vault's).
    function _word(bytes memory j, uint256 i) internal pure returns (bytes32 w) {
        assembly {
            w := mload(add(j, add(32, mul(32, i))))
        }
    }
}

// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {WrapVault} from "../src/WrapVault.sol";
import {ZkChainBurnVerifier} from "../src/ZkChainBurnVerifier.sol";
import {LezBlockCommittee} from "../src/LezBlockCommittee.sol";
import {VerificationFailed} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroGroth16Verifier, SelectorMismatch} from "risc0/groth16/RiscZeroGroth16Verifier.sol";
import {ControlID} from "risc0/groth16/ControlID.sol";

/// F.7 with the root derived from LEZ blocks: a release carries one real Groth16 seal for two
/// guests, made by scripts/e2e-chain.sh on the local run (evidence/local-chain/): zk/lez_chain over
/// the local v0.3.1 sequencer's blocks from the anchor to a tip, in two recursive steps, and
/// zk/burn_release proving carol's private burn under that chain's tree, the chain receipt
/// resolved inside. The fixture (test/fixtures/zk_chain_release.json) is that proof with the
/// deployment it was made for: the vault sits at the address its journal names, with its pairing.
/// Every refusal a release can meet, one test each.
contract ZkChainBurnVerifierTest is Test {
    WrapVault v;
    ZkChainBurnVerifier zv;
    RiscZeroGroth16Verifier risc0;
    address constant ETH = address(0);
    uint64 constant DEPTH_BLOCKS = 3;
    uint256 constant ONE = 1 ether;
    uint256[3] pks = [uint256(0xC0FFEE01), 0xC0FFEE02, 0xC0FFEE03];
    uint256[2] outsiders = [uint256(0xBAD01), 0xBAD02];
    address admin = address(0xA11CE);
    address mallory = address(0xBAD);

    bytes seal;
    bytes journal;
    bytes32 imageId;
    bytes32 chainImageId;
    bytes32 programId;
    bytes32 burnsAccount;
    uint64 anchorId;
    bytes32 anchorHash;
    uint64 tipId;
    bytes32 tipHash;
    address vaultAddr;
    bytes32 pairing;
    address carol;
    address relayer;
    uint256 fee;
    bytes32 root;
    bytes32 nullifier;

    function setUp() public {
        string memory f = vm.readFile("test/fixtures/zk_chain_release.json");
        seal = vm.parseJsonBytes(f, ".seal");
        journal = vm.parseJsonBytes(f, ".journal");
        imageId = vm.parseJsonBytes32(f, ".image_id");
        assertEq(journal.length, 576, "the release preimage and the chain tail");
        chainImageId = _word(11);
        programId = _word(12);
        burnsAccount = _word(13);
        anchorId = uint64(uint256(_word(14)));
        anchorHash = _word(15);
        tipId = uint64(uint256(_word(16)));
        tipHash = _word(17);
        vaultAddr = address(uint160(uint256(_word(2))));
        pairing = _word(3);
        carol = address(uint160(uint256(_word(6))));
        relayer = address(uint160(uint256(_word(7))));
        fee = uint256(_word(8));
        root = _word(9);
        nullifier = _word(10);
        vm.chainId(uint256(_word(1)));

        risc0 = new RiscZeroGroth16Verifier(ControlID.CONTROL_ROOT, ControlID.BN254_CONTROL_ID);
        zv = _verifier(imageId, chainImageId, anchorHash);
        deployCodeTo("WrapVault.sol:WrapVault", abi.encode(pairing, zv, uint64(1 days), admin, admin), vaultAddr);
        v = WrapVault(payable(vaultAddr));
        uint256[] memory amounts = new uint256[](2);
        amounts[0] = 0.1 ether;
        amounts[1] = ONE;
        vm.prank(admin);
        v.registerToken(ETH, 18, amounts, 100 ether, 100 ether);
        vm.deal(admin, 10 ether);
        vm.prank(admin);
        v.deposit{value: ONE}(ETH, ONE, bytes32(uint256(1)));
    }

    function _verifier(bytes32 img, bytes32 chainImg, bytes32 anchor) internal returns (ZkChainBurnVerifier) {
        return new ZkChainBurnVerifier(risc0, img, chainImg, programId, burnsAccount, anchorId, anchor, _members(), 2, DEPTH_BLOCKS);
    }

    function _members() internal view returns (address[] memory m) {
        m = new address[](3);
        for (uint256 i; i < 3; ++i) m[i] = vm.addr(pks[i]);
    }

    function _word(uint256 i) internal view returns (bytes32 w) {
        bytes memory j = journal;
        assembly {
            w := mload(add(j, add(32, mul(32, i))))
        }
    }

    function _journalWith(uint256 i, bytes32 w) internal view returns (bytes memory j) {
        j = bytes.concat(journal);
        for (uint256 k; k < 32; ++k) j[32 * i + k] = w[k];
    }

    function _att(bytes32 h, uint64 b, uint64 fin) internal view returns (bytes32) {
        return keccak256(abi.encode(bytes32("rfp022/lez-block-final/v1"), block.chainid, vaultAddr, pairing, h, uint256(b), uint256(fin)));
    }

    function _sign(uint256 pk, bytes32 h, uint64 b, uint64 fin) internal view returns (bytes memory) {
        (uint8 vv, bytes32 rr, bytes32 s) = vm.sign(pk, keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", _att(h, b, fin))));
        return abi.encodePacked(rr, s, vv);
    }

    function _sigs(uint256[] memory who, bytes32 h, uint64 b, uint64 fin) internal view returns (bytes[] memory s) {
        s = new bytes[](who.length);
        for (uint256 i; i < who.length; ++i) s[i] = _sign(who[i], h, b, fin);
    }

    function _two() internal view returns (uint256[] memory w) {
        w = new uint256[](2);
        w[0] = pks[0];
        w[1] = pks[2];
    }

    /// Carol's release: the fixture, the tip attested final by members 1 and 3.
    function _carol() internal view returns (ZkChainBurnVerifier.Release memory r) {
        r.seal = seal;
        r.journal = journal;
        r.lezFinalBlock = tipId + DEPTH_BLOCKS;
        r.signatures = _sigs(_two(), tipHash, tipId, r.lezFinalBlock);
    }

    struct Args {
        uint256 amount;
        address recipient;
        address relayer;
        uint256 fee;
        bytes32 root;
        bytes32 nullifier;
    }

    function _args() internal view returns (Args memory) {
        return Args(ONE, carol, relayer, fee, root, nullifier);
    }

    function _release(Args memory a, ZkChainBurnVerifier.Release memory r) internal {
        v.release(ETH, a.amount, payable(a.recipient), payable(a.relayer), a.fee, a.root, a.nullifier, abi.encode(r));
    }

    function _refused(Args memory a, ZkChainBurnVerifier.Release memory r, bytes memory err) internal {
        bytes memory proof = abi.encode(r);
        vm.expectRevert(err);
        v.release(ETH, a.amount, payable(a.recipient), payable(a.relayer), a.fee, a.root, a.nullifier, proof);
    }

    function _sel(bytes4 s) internal pure returns (bytes memory) {
        return abi.encodePacked(s);
    }

    // ---- the fixture ----

    function test_the_fixture_is_a_release_of_this_vault_over_a_chain() public view {
        assertEq(_word(0), bytes32("rfp022/wrap/release/v1"));
        bytes memory pre = new bytes(352);
        for (uint256 i; i < 352; ++i) pre[i] = journal[i];
        assertEq(sha256(pre), v.releaseDigest(ETH, ONE, carol, relayer, fee, root, nullifier), "the vault's digest");
        assertEq(bytes4(seal), risc0.SELECTOR());
        assertEq(seal.length, 260);
        assertGt(tipId, anchorId, "the chain covers blocks after the anchor");
        assertEq(fee, 0.01 ether, "the fee carol's burn fixed");
    }

    // ---- the valid release ----

    function test_a_chain_derived_release_pays_once_to_the_committed_recipient() public {
        uint256 before = address(v).balance;
        (Args memory a, bytes memory proof) = (_args(), abi.encode(_carol()));
        vm.prank(relayer);
        uint256 g = gasleft();
        v.release(ETH, a.amount, payable(a.recipient), payable(a.relayer), a.fee, a.root, a.nullifier, proof);
        emit log_named_uint("WrapVault.release via ZkChainBurnVerifier, execution gas (no 21000, no calldata)", g - gasleft());
        assertEq(carol.balance, ONE - fee);
        assertEq(relayer.balance, fee);
        assertEq(address(v).balance, before - ONE);
        assertTrue(v.nullifierSpent(nullifier));
    }

    function test_the_groth16_check_alone() public {
        bytes32 digest = sha256(journal);
        uint256 g = gasleft();
        risc0.verify(seal, imageId, digest);
        emit log_named_uint("RiscZeroGroth16Verifier.verify, execution gas", g - gasleft());
    }

    function test_replay_is_refused() public {
        _release(_args(), _carol());
        _refused(_args(), _carol(), _sel(WrapVault.AlreadyReleased.selector));
    }

    // ---- another release than the one proven ----

    function test_another_recipient_amount_fee_relayer_root_or_nullifier_is_refused() public {
        Args memory a;
        for (uint256 k; k < 6; ++k) {
            a = _args();
            if (k == 0) a.recipient = mallory;
            if (k == 1) a.amount = 0.1 ether;
            if (k == 2) a.fee = 0.5 ether;
            if (k == 3) a.relayer = mallory;
            if (k == 4) a.root = keccak256("another root");
            if (k == 5) a.nullifier = keccak256("another nullifier");
            _refused(a, _carol(), _sel(ZkChainBurnVerifier.NotThisRelease.selector));
        }
    }

    function test_another_chain_id_is_refused() public {
        vm.chainId(1);
        ZkChainBurnVerifier.Release memory r = _carol();
        _refused(_args(), r, _sel(ZkChainBurnVerifier.NotThisRelease.selector));
    }

    /// A bare membership journal (zk/burn_membership's, 352 bytes) names no chain.
    function test_a_membership_journal_without_the_chain_is_refused() public {
        ZkChainBurnVerifier.Release memory r = _carol();
        bytes memory pre = new bytes(352);
        for (uint256 i; i < 352; ++i) pre[i] = journal[i];
        r.journal = pre;
        _refused(_args(), r, _sel(ZkChainBurnVerifier.NotThisRelease.selector));
    }

    // ---- the journal tampered ----

    function test_a_journal_tampered_to_match_other_arguments_is_refused() public {
        Args memory a = _args();
        a.recipient = mallory;
        ZkChainBurnVerifier.Release memory r = _carol();
        r.journal = _journalWith(6, bytes32(uint256(uint160(mallory))));
        _refused(a, r, _sel(VerificationFailed.selector));
        a = _args();
        a.root = keccak256("a root of no chain");
        r = _carol();
        r.journal = _journalWith(9, a.root);
        _refused(a, r, _sel(VerificationFailed.selector));
    }

    /// The chain tail must be this deployment's: the pinned chain guest, program, burn tree and anchor.
    function test_a_journal_naming_another_chain_is_refused() public {
        for (uint256 w = 11; w <= 15; ++w) {
            ZkChainBurnVerifier.Release memory r = _carol();
            r.journal = _journalWith(w, keccak256(abi.encode("another", w)));
            _refused(_args(), r, _sel(ZkChainBurnVerifier.NotThisChain.selector));
        }
    }

    /// Another tip in the journal, attested by the committee: the seal does not prove it.
    function test_a_journal_tampered_to_another_tip_is_refused() public {
        bytes32 other = keccak256("a block the committee signs");
        ZkChainBurnVerifier.Release memory r = _carol();
        r.journal = _journalWith(17, other);
        r.signatures = _sigs(_two(), other, tipId, r.lezFinalBlock);
        _refused(_args(), r, _sel(VerificationFailed.selector));
        r = _carol();
        r.journal = _journalWith(16, bytes32(uint256(tipId + 1)));
        r.signatures = _sigs(_two(), tipHash, tipId + 1, tipId + 1 + DEPTH_BLOCKS);
        r.lezFinalBlock = tipId + 1 + DEPTH_BLOCKS;
        _refused(_args(), r, _sel(VerificationFailed.selector));
    }

    // ---- the seal and the pinned guests ----

    function test_a_tampered_seal_is_refused() public {
        ZkChainBurnVerifier.Release memory r = _carol();
        r.seal = bytes.concat(seal);
        r.seal[100] = bytes1(uint8(r.seal[100]) ^ 1);
        _refused(_args(), r, _sel(VerificationFailed.selector));
        r.seal = bytes.concat(seal);
        r.seal[0] = bytes1(uint8(r.seal[0]) ^ 0xff);
        _refused(_args(), r, abi.encodeWithSelector(SelectorMismatch.selector, bytes4(r.seal), risc0.SELECTOR()));
    }

    function test_a_verifier_pinned_to_other_guests_or_another_anchor_refuses() public {
        bytes memory proof = abi.encode(_carol());
        bytes32 digest = v.releaseDigest(ETH, ONE, carol, relayer, fee, root, nullifier);
        ZkChainBurnVerifier other = _verifier(keccak256("another release guest"), chainImageId, anchorHash);
        vm.prank(vaultAddr);
        vm.expectRevert(VerificationFailed.selector);
        other.verify(proof, digest);
        other = _verifier(imageId, keccak256("another chain guest"), anchorHash);
        vm.prank(vaultAddr);
        vm.expectRevert(ZkChainBurnVerifier.NotThisChain.selector);
        other.verify(proof, digest);
        other = _verifier(imageId, chainImageId, keccak256("another anchor"));
        vm.prank(vaultAddr);
        vm.expectRevert(ZkChainBurnVerifier.NotThisChain.selector);
        other.verify(proof, digest);
        vm.prank(vaultAddr);
        assertTrue(zv.verify(proof, digest));
    }

    // ---- the tip: final, attested ----

    function test_a_tip_not_yet_final_is_refused() public {
        ZkChainBurnVerifier.Release memory r = _carol();
        r.lezFinalBlock = tipId + DEPTH_BLOCKS - 1;
        r.signatures = _sigs(_two(), tipHash, tipId, r.lezFinalBlock);
        _refused(_args(), r, _sel(LezBlockCommittee.NotFinal.selector));
    }

    function test_a_block_hash_the_members_did_not_read_is_refused() public {
        ZkChainBurnVerifier.Release memory r = _carol();
        r.signatures = _sigs(_two(), keccak256("the block the members read"), tipId, r.lezFinalBlock);
        _refused(_args(), r, _sel(LezBlockCommittee.BlockNotAttested.selector));
    }

    function test_one_member_or_outsiders_are_refused() public {
        ZkChainBurnVerifier.Release memory r = _carol();
        uint256[] memory one = new uint256[](2);
        one[0] = pks[1];
        one[1] = pks[1];
        r.signatures = _sigs(one, tipHash, tipId, r.lezFinalBlock);
        _refused(_args(), r, _sel(LezBlockCommittee.BlockNotAttested.selector));
        uint256[] memory out = new uint256[](2);
        out[0] = outsiders[0];
        out[1] = outsiders[1];
        r.signatures = _sigs(out, tipHash, tipId, r.lezFinalBlock);
        _refused(_args(), r, _sel(LezBlockCommittee.BlockNotAttested.selector));
    }

    /// A member's signature in its malleated (high-s) form does not count: member 1's, malleated,
    /// with member 3's, is one signature, not two.
    function test_a_malleated_signature_does_not_count() public {
        ZkChainBurnVerifier.Release memory r = _carol();
        bytes memory sig = _sign(pks[0], tipHash, tipId, r.lezFinalBlock);
        bytes32 rr;
        bytes32 s;
        uint8 vv;
        assembly {
            rr := mload(add(sig, 32))
            s := mload(add(sig, 64))
            vv := byte(0, mload(add(sig, 96)))
        }
        uint256 n = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;
        bytes32 hs = bytes32(n - uint256(s));
        uint8 hv = vv == 27 ? 28 : 27;
        bytes32 digest = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", _att(tipHash, tipId, r.lezFinalBlock)));
        assertEq(ecrecover(digest, hv, rr, hs), vm.addr(pks[0]), "the malleated form recovers to member 1");
        r.signatures[0] = abi.encodePacked(rr, hs, hv);
        r.signatures[1] = _sign(pks[2], tipHash, tipId, r.lezFinalBlock);
        _refused(_args(), r, _sel(LezBlockCommittee.BlockNotAttested.selector));
    }

    /// One member and one outsider are one signature.
    function test_a_member_and_an_outsider_are_refused() public {
        ZkChainBurnVerifier.Release memory r = _carol();
        r.signatures[0] = _sign(pks[2], tipHash, tipId, r.lezFinalBlock);
        r.signatures[1] = _sign(outsiders[0], tipHash, tipId, r.lezFinalBlock);
        _refused(_args(), r, _sel(LezBlockCommittee.BlockNotAttested.selector));
    }

    /// An attestation for another vault of the same pairing does not apply here.
    function test_an_attestation_for_another_vault_is_refused() public {
        ZkChainBurnVerifier.Release memory r = _carol();
        bytes32 d = keccak256(abi.encode(bytes32("rfp022/lez-block-final/v1"), block.chainid, address(0xdead), pairing, tipHash, uint256(tipId), uint256(r.lezFinalBlock)));
        for (uint256 i; i < 2; ++i) {
            (uint8 vv, bytes32 rr, bytes32 s) = vm.sign(_two()[i], keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", d)));
            r.signatures[i] = abi.encodePacked(rr, s, vv);
        }
        _refused(_args(), r, _sel(LezBlockCommittee.BlockNotAttested.selector));
    }

    function test_the_attestation_digest_is_the_scripts() public view {
        assertEq(zv.blockAttestationDigest(vaultAddr, pairing, tipHash, tipId, tipId + 3), _att(tipHash, tipId, tipId + 3));
    }

    function test_a_misconfigured_verifier_is_refused() public {
        vm.expectRevert(LezBlockCommittee.BadConfig.selector);
        new ZkChainBurnVerifier(risc0, imageId, chainImageId, programId, bytes32(0), anchorId, anchorHash, _members(), 2, DEPTH_BLOCKS);
        vm.expectRevert(LezBlockCommittee.BadConfig.selector);
        new ZkChainBurnVerifier(risc0, imageId, chainImageId, programId, burnsAccount, anchorId, anchorHash, _members(), 4, DEPTH_BLOCKS);
    }
}

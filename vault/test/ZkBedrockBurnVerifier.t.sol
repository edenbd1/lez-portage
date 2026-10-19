// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {WrapVault} from "../src/WrapVault.sol";
import {ZkBedrockBurnVerifier} from "../src/ZkBedrockBurnVerifier.sol";
import {BedrockHeaderCommittee} from "../src/BedrockHeaderCommittee.sol";
import {VerificationFailed} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroGroth16Verifier, SelectorMismatch} from "risc0/groth16/RiscZeroGroth16Verifier.sol";
import {ControlID} from "risc0/groth16/ControlID.sol";

/// F.7 with the root derived from LEZ blocks and the chain's tip anchored in Bedrock: a release
/// carries one real Groth16 seal for three guests, made by scripts/e2e-bedrock.sh on the local run
/// (evidence/local-bedrock/): zk/lez_chain over the local v0.3.1 sequencer's blocks, zk/bedrock_anchor
/// proving the chain's tip inscribed on the sequencer's channel of a local Bedrock devnet, signed by
/// its key, MIN_DEPTH headers deep, and zk/bedrock_release proving carol's private burn under the
/// chain's tree. The fixture (test/fixtures/zk_bedrock_release.json) is that proof with the
/// deployment it was made for. Every refusal a release can meet, one test each.
contract ZkBedrockBurnVerifierTest is Test {
    WrapVault v;
    ZkBedrockBurnVerifier zv;
    RiscZeroGroth16Verifier risc0;
    address constant ETH = address(0);
    uint256 constant ONE = 1 ether;
    uint256[3] pks = [uint256(0xC0FFEE01), 0xC0FFEE02, 0xC0FFEE03];
    uint256[2] outsiders = [uint256(0xBAD01), 0xBAD02];
    address admin = address(0xA11CE);
    address mallory = address(0xBAD);

    bytes seal;
    bytes journal;
    ZkBedrockBurnVerifier.Config cfg;
    bytes32 top;
    uint32 depth;
    address vaultAddr;
    bytes32 pairing;
    address carol;
    address relayer;
    uint256 fee;
    bytes32 root;
    bytes32 nullifier;

    function setUp() public {
        string memory f = vm.readFile("test/fixtures/zk_bedrock_release.json");
        seal = vm.parseJsonBytes(f, ".seal");
        journal = vm.parseJsonBytes(f, ".journal");
        assertEq(journal.length, 800, "the release preimage, the chain tail, the Bedrock tail");
        risc0 = new RiscZeroGroth16Verifier(ControlID.CONTROL_ROOT, ControlID.BN254_CONTROL_ID);
        depth = uint32(uint256(_word(24)));
        cfg = ZkBedrockBurnVerifier.Config({
            risc0: risc0,
            imageId: vm.parseJsonBytes32(f, ".image_id"),
            chainImageId: _word(11),
            anchorImageId: _word(18),
            programId: _word(12),
            burnsAccount: _word(13),
            anchorId: uint64(uint256(_word(14))),
            anchorHash: _word(15),
            channelId: _word(19),
            sequencerKey: _word(20),
            minDepth: depth
        });
        top = _word(22);
        vaultAddr = address(uint160(uint256(_word(2))));
        pairing = _word(3);
        carol = address(uint160(uint256(_word(6))));
        relayer = address(uint160(uint256(_word(7))));
        fee = uint256(_word(8));
        root = _word(9);
        nullifier = _word(10);
        vm.chainId(uint256(_word(1)));

        zv = new ZkBedrockBurnVerifier(cfg, _members(), 2);
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

    function _att(address vault, bytes32 h) internal view returns (bytes32) {
        return keccak256(abi.encode(bytes32("rfp022/bedrock-header/v1"), block.chainid, vault, pairing, h));
    }

    function _sign(uint256 pk, bytes32 h) internal view returns (bytes memory) {
        (uint8 vv, bytes32 rr, bytes32 s) = vm.sign(pk, keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", _att(vaultAddr, h))));
        return abi.encodePacked(rr, s, vv);
    }

    function _sigs(uint256 a, uint256 b, bytes32 h) internal view returns (bytes[] memory s) {
        s = new bytes[](2);
        s[0] = _sign(a, h);
        s[1] = _sign(b, h);
    }

    /// Carol's release: the fixture, the top header attested canonical by members 1 and 3.
    function _carol() internal view returns (ZkBedrockBurnVerifier.Release memory r) {
        r.seal = seal;
        r.journal = journal;
        r.signatures = _sigs(pks[0], pks[2], top);
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

    function _release(Args memory a, ZkBedrockBurnVerifier.Release memory r) internal {
        v.release(ETH, a.amount, payable(a.recipient), payable(a.relayer), a.fee, a.root, a.nullifier, abi.encode(r));
    }

    function _refused(Args memory a, ZkBedrockBurnVerifier.Release memory r, bytes memory err) internal {
        bytes memory proof = abi.encode(r);
        vm.expectRevert(err);
        v.release(ETH, a.amount, payable(a.recipient), payable(a.relayer), a.fee, a.root, a.nullifier, proof);
    }

    function _sel(bytes4 s) internal pure returns (bytes memory) {
        return abi.encodePacked(s);
    }

    // ---- the fixture ----

    function test_the_fixture_is_a_release_of_this_vault_anchored_in_bedrock() public view {
        assertEq(_word(0), bytes32("rfp022/wrap/release/v1"));
        bytes memory pre = new bytes(352);
        for (uint256 i; i < 352; ++i) pre[i] = journal[i];
        assertEq(sha256(pre), v.releaseDigest(ETH, ONE, carol, relayer, fee, root, nullifier), "the vault's digest");
        assertEq(bytes4(seal), risc0.SELECTOR());
        assertEq(seal.length, 260);
        assertGt(uint256(_word(16)), uint256(_word(14)), "the chain covers blocks after the anchor");
        assertGe(depth, 5, "at least Bedrock's k of the devnet");
        assertTrue(_word(21) != top, "the top header is above the inscription's block");
        assertEq(fee, 0.01 ether, "the fee carol's burn fixed");
    }

    // ---- the valid release ----

    function test_a_bedrock_anchored_release_pays_once_to_the_committed_recipient() public {
        uint256 before = address(v).balance;
        (Args memory a, bytes memory proof) = (_args(), abi.encode(_carol()));
        vm.prank(relayer);
        uint256 g = gasleft();
        v.release(ETH, a.amount, payable(a.recipient), payable(a.relayer), a.fee, a.root, a.nullifier, proof);
        emit log_named_uint("WrapVault.release via ZkBedrockBurnVerifier, execution gas (no 21000, no calldata)", g - gasleft());
        assertEq(carol.balance, ONE - fee);
        assertEq(relayer.balance, fee);
        assertEq(address(v).balance, before - ONE);
        assertTrue(v.nullifierSpent(nullifier));
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
            _refused(a, _carol(), _sel(ZkBedrockBurnVerifier.NotThisRelease.selector));
        }
    }

    function test_another_chain_id_is_refused() public {
        vm.chainId(1);
        _refused(_args(), _carol(), _sel(ZkBedrockBurnVerifier.NotThisRelease.selector));
    }

    /// A chain-derived journal (zk/burn_release's, 576 bytes) carries no Bedrock anchor.
    function test_a_chain_journal_without_the_anchor_is_refused() public {
        ZkBedrockBurnVerifier.Release memory r = _carol();
        bytes memory j = new bytes(576);
        for (uint256 i; i < 576; ++i) j[i] = journal[i];
        r.journal = j;
        _refused(_args(), r, _sel(ZkBedrockBurnVerifier.NotThisRelease.selector));
    }

    // ---- the journal tampered ----

    function test_a_journal_tampered_to_match_other_arguments_is_refused() public {
        Args memory a = _args();
        a.recipient = mallory;
        ZkBedrockBurnVerifier.Release memory r = _carol();
        r.journal = _journalWith(6, bytes32(uint256(uint160(mallory))));
        _refused(a, r, _sel(VerificationFailed.selector));
    }

    function test_a_journal_naming_another_chain_is_refused() public {
        for (uint256 w = 11; w <= 15; ++w) {
            ZkBedrockBurnVerifier.Release memory r = _carol();
            r.journal = _journalWith(w, keccak256(abi.encode("another", w)));
            _refused(_args(), r, _sel(ZkBedrockBurnVerifier.NotThisChain.selector));
        }
    }

    /// Another anchor guest, channel or sequencer key in the journal.
    function test_a_journal_naming_another_anchor_guest_channel_or_key_is_refused() public {
        for (uint256 w = 18; w <= 20; ++w) {
            ZkBedrockBurnVerifier.Release memory r = _carol();
            r.journal = _journalWith(w, keccak256(abi.encode("another", w)));
            _refused(_args(), r, _sel(ZkBedrockBurnVerifier.NotThisChannel.selector));
        }
    }

    function test_an_anchor_shallower_than_min_depth_is_refused() public {
        ZkBedrockBurnVerifier.Release memory r = _carol();
        r.journal = _journalWith(24, bytes32(uint256(depth - 1)));
        _refused(_args(), r, _sel(ZkBedrockBurnVerifier.NotDeepEnough.selector));
        // Claiming more depth than proven: the seal does not prove it.
        r.journal = _journalWith(24, bytes32(uint256(depth + 25)));
        _refused(_args(), r, _sel(VerificationFailed.selector));
    }

    /// Another top header in the journal, attested by the committee: the seal does not prove it.
    /// Another inscription block or slot: the same.
    function test_a_journal_tampered_to_another_bedrock_header_is_refused() public {
        bytes32 other = keccak256("a header the committee signs");
        ZkBedrockBurnVerifier.Release memory r = _carol();
        r.journal = _journalWith(22, other);
        r.signatures = _sigs(pks[0], pks[2], other);
        _refused(_args(), r, _sel(VerificationFailed.selector));
        for (uint256 w = 21; w <= 23; w += 2) {
            r = _carol();
            r.journal = _journalWith(w, keccak256(abi.encode("another", w)));
            _refused(_args(), r, _sel(VerificationFailed.selector));
        }
    }

    // ---- the seal and the pinned guests ----

    function test_a_tampered_seal_is_refused() public {
        ZkBedrockBurnVerifier.Release memory r = _carol();
        r.seal = bytes.concat(seal);
        r.seal[100] = bytes1(uint8(r.seal[100]) ^ 1);
        _refused(_args(), r, _sel(VerificationFailed.selector));
        r.seal = bytes.concat(seal);
        r.seal[0] = bytes1(uint8(r.seal[0]) ^ 0xff);
        _refused(_args(), r, abi.encodeWithSelector(SelectorMismatch.selector, bytes4(r.seal), risc0.SELECTOR()));
    }

    function test_a_verifier_pinned_otherwise_refuses() public {
        bytes memory proof = abi.encode(_carol());
        bytes32 digest = v.releaseDigest(ETH, ONE, carol, relayer, fee, root, nullifier);
        for (uint256 k; k < 7; ++k) {
            ZkBedrockBurnVerifier.Config memory c = cfg;
            bytes4 err = ZkBedrockBurnVerifier.NotThisChannel.selector;
            if (k == 0) (c.imageId, err) = (keccak256("another release guest"), VerificationFailed.selector);
            if (k == 1) (c.chainImageId, err) = (keccak256("another chain guest"), ZkBedrockBurnVerifier.NotThisChain.selector);
            if (k == 2) (c.anchorHash, err) = (keccak256("another anchor"), ZkBedrockBurnVerifier.NotThisChain.selector);
            if (k == 3) c.anchorImageId = keccak256("another anchor guest");
            if (k == 4) c.channelId = keccak256("another channel");
            if (k == 5) c.sequencerKey = keccak256("another key");
            if (k == 6) (c.minDepth, err) = (depth + 1, ZkBedrockBurnVerifier.NotDeepEnough.selector);
            ZkBedrockBurnVerifier other = new ZkBedrockBurnVerifier(c, _members(), 2);
            vm.prank(vaultAddr);
            vm.expectRevert(err);
            other.verify(proof, digest);
        }
        vm.prank(vaultAddr);
        assertTrue(zv.verify(proof, digest));
    }

    // ---- the top header: canonical, attested ----

    function test_a_header_the_members_did_not_see_is_refused() public {
        ZkBedrockBurnVerifier.Release memory r = _carol();
        r.signatures = _sigs(pks[0], pks[2], keccak256("the header the members see"));
        _refused(_args(), r, _sel(BedrockHeaderCommittee.HeaderNotAttested.selector));
        // The inscription's own block, not the top: not what the proof names as top.
        r.signatures = _sigs(pks[0], pks[2], _word(21));
        _refused(_args(), r, _sel(BedrockHeaderCommittee.HeaderNotAttested.selector));
    }

    function test_one_member_twice_outsiders_or_a_member_and_an_outsider_are_refused() public {
        ZkBedrockBurnVerifier.Release memory r = _carol();
        r.signatures = _sigs(pks[1], pks[1], top);
        _refused(_args(), r, _sel(BedrockHeaderCommittee.HeaderNotAttested.selector));
        r.signatures = _sigs(outsiders[0], outsiders[1], top);
        _refused(_args(), r, _sel(BedrockHeaderCommittee.HeaderNotAttested.selector));
        r.signatures = _sigs(pks[2], outsiders[0], top);
        _refused(_args(), r, _sel(BedrockHeaderCommittee.HeaderNotAttested.selector));
        r.signatures = new bytes[](0);
        _refused(_args(), r, _sel(BedrockHeaderCommittee.HeaderNotAttested.selector));
    }

    /// A member's signature in its malleated (high-s) form does not count.
    function test_a_malleated_signature_does_not_count() public {
        ZkBedrockBurnVerifier.Release memory r = _carol();
        bytes memory sig = _sign(pks[0], top);
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
        bytes32 digest = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", _att(vaultAddr, top)));
        assertEq(ecrecover(digest, hv, rr, hs), vm.addr(pks[0]), "the malleated form recovers to member 1");
        r.signatures[0] = abi.encodePacked(rr, hs, hv);
        r.signatures[1] = _sign(pks[2], top);
        _refused(_args(), r, _sel(BedrockHeaderCommittee.HeaderNotAttested.selector));
    }

    /// An attestation for another vault of the same pairing does not apply here.
    function test_an_attestation_for_another_vault_is_refused() public {
        ZkBedrockBurnVerifier.Release memory r = _carol();
        for (uint256 i; i < 2; ++i) {
            (uint8 vv, bytes32 rr, bytes32 s) =
                vm.sign(i == 0 ? pks[0] : pks[2], keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", _att(address(0xdead), top))));
            r.signatures[i] = abi.encodePacked(rr, s, vv);
        }
        _refused(_args(), r, _sel(BedrockHeaderCommittee.HeaderNotAttested.selector));
    }

    function test_the_attestation_digest_is_the_scripts() public view {
        assertEq(zv.headerAttestationDigest(vaultAddr, pairing, top), _att(vaultAddr, top));
    }

    function test_a_misconfigured_verifier_is_refused() public {
        ZkBedrockBurnVerifier.Config memory c = cfg;
        c.sequencerKey = bytes32(0);
        vm.expectRevert(BedrockHeaderCommittee.BadConfig.selector);
        new ZkBedrockBurnVerifier(c, _members(), 2);
        c = cfg;
        c.minDepth = 0;
        vm.expectRevert(BedrockHeaderCommittee.BadConfig.selector);
        new ZkBedrockBurnVerifier(c, _members(), 2);
        vm.expectRevert(BedrockHeaderCommittee.BadConfig.selector);
        new ZkBedrockBurnVerifier(cfg, _members(), 4);
    }
}

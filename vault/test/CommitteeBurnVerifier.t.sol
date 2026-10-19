// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {WrapVault} from "../src/WrapVault.sol";
import {CommitteeBurnVerifier} from "../src/CommitteeBurnVerifier.sol";
import {BurnRootCommittee} from "../src/BurnRootCommittee.sol";
import {IBurnVerifier} from "../src/IBurnVerifier.sol";

/// F.7 as built: a release opens a LEZ burn leaf, checked here against a burn-tree root that a
/// 2-of-3 committee attests final. Every refusal a release can meet, one test each.
contract CommitteeBurnVerifierTest is Test {
    WrapVault v;
    CommitteeBurnVerifier cv;
    address constant ETH = address(0);
    bytes32 constant PAIRING = keccak256("test pairing");
    uint64 constant DEPTH_BLOCKS = 3;
    uint256 constant ONE = 1 ether;
    uint256[3] pks = [uint256(0xC0FFEE01), 0xC0FFEE02, 0xC0FFEE03];
    address admin = address(0xA11CE);
    address alice = address(0xA1);
    address carol = address(0xCA);
    address relayer = address(0x7E1A);
    address mallory = address(0xBAD);

    // The LEZ burn tree of the test: three burns; leaf 1 is carol's 1 ETH, fee 0.01 ETH.
    bytes32[] leaves;
    bytes32 constant CAROL_SECRET = bytes32(uint256(0xCA5EC2E7));
    uint256 constant FEE = 0.01 ether;
    uint64 constant LEZ_BLOCK = 40;

    function setUp() public {
        address[] memory m = new address[](3);
        for (uint256 i; i < 3; ++i) m[i] = vm.addr(pks[i]);
        cv = new CommitteeBurnVerifier(m, 2, DEPTH_BLOCKS);
        v = new WrapVault(PAIRING, cv, 1 days, admin, admin);
        uint256[] memory amounts = new uint256[](2);
        amounts[0] = 0.1 ether;
        amounts[1] = ONE;
        vm.prank(admin);
        v.registerToken(ETH, 18, amounts, 100 ether, 100 ether);
        vm.deal(alice, 10 ether);
        for (uint256 i; i < 3; ++i) {
            vm.prank(alice);
            v.deposit{value: ONE}(ETH, ONE, bytes32(i));
        }
        leaves.push(_leaf(ETH, ONE, _ec(alice, 0, bytes32(uint256(1)))));
        leaves.push(_leaf(ETH, ONE, _ec(carol, FEE, CAROL_SECRET)));
        leaves.push(_leaf(ETH, 0.1 ether, _ec(alice, 0, bytes32(uint256(2)))));
    }

    // ---- the hashes, written again here (an independent copy of wrap-core's), so that a test
    // makes no external call but the one it means ----

    function _le(uint256 x, uint256 n) internal pure returns (bytes memory out) {
        out = new bytes(n);
        for (uint256 i; i < n; ++i) out[i] = bytes1(uint8(x >> (8 * i)));
    }

    function _ec(address recipient, uint256 fee, bytes32 secret) internal pure returns (bytes32) {
        return sha256(abi.encode(bytes32("rfp022/eth-commit/v1"), recipient, fee, secret));
    }

    function _leaf(address token, uint256 amount, bytes32 ec) internal pure returns (bytes32) {
        return sha256(abi.encodePacked(bytes("rfp022/burn/v1"), PAIRING, token, _le(amount, 16), ec));
    }

    function _nf(bytes32 secret, uint64 index) internal pure returns (bytes32) {
        return sha256(abi.encodePacked(bytes("rfp022/burn-nullifier/v1"), PAIRING, secret, _le(index, 8)));
    }

    function _rootOf(bytes32 leaf, uint64 index, bytes32[24] memory path) internal pure returns (bytes32 node) {
        node = leaf;
        for (uint256 l; l < 24; ++l) node = (index >> l) & 1 == 1 ? _h(path[l], node) : _h(node, path[l]);
    }

    function _att(address vault, bytes32 root, uint64 b, uint64 f) internal view returns (bytes32) {
        return keccak256(
            abi.encode(bytes32("rfp022/burn-root-attestation/v1"), block.chainid, vault, PAIRING, root, uint256(b), uint256(f))
        );
    }

    // ---- a client's view of the burn tree ----

    function _h(bytes32 a, bytes32 b) internal pure returns (bytes32) {
        return sha256(abi.encodePacked(a, b));
    }

    function _path(bytes32[] memory ls, uint256 index) internal pure returns (bytes32[24] memory p) {
        bytes32[] memory layer = ls;
        bytes32 z;
        for (uint256 d; d < 24; ++d) {
            uint256 sib = index ^ 1;
            p[d] = sib < layer.length ? layer[sib] : z;
            uint256 n = (layer.length + 1) / 2;
            bytes32[] memory next = new bytes32[](n);
            for (uint256 i; i < n; ++i) {
                next[i] = _h(layer[2 * i], 2 * i + 1 < layer.length ? layer[2 * i + 1] : z);
            }
            layer = next;
            index >>= 1;
            z = _h(z, z);
        }
    }

    function _root() internal view returns (bytes32) {
        return _rootOf(leaves[1], 1, _path(leaves, 1));
    }

    function _sign(uint256 pk, bytes32 root, uint64 b, uint64 f, address vault) internal view returns (bytes memory) {
        bytes32 d = _att(vault, root, b, f);
        (uint8 vv, bytes32 r, bytes32 s) = vm.sign(pk, keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", d)));
        return abi.encodePacked(r, s, vv);
    }

    function _sigs(uint256[] memory who, bytes32 root, uint64 b, uint64 f) internal view returns (bytes[] memory s) {
        s = new bytes[](who.length);
        for (uint256 i; i < who.length; ++i) s[i] = _sign(who[i], root, b, f, address(v));
    }

    function _two() internal view returns (uint256[] memory w) {
        w = new uint256[](2);
        w[0] = pks[0];
        w[1] = pks[2];
    }

    /// Carol's honest release, attested by members 1 and 3 at a final block.
    function _carol() internal view returns (CommitteeBurnVerifier.Release memory r) {
        r.token = ETH;
        r.amount = ONE;
        r.recipient = carol;
        r.relayer = relayer;
        r.fee = FEE;
        r.secret = CAROL_SECRET;
        r.index = 1;
        r.path = _path(leaves, 1);
        r.lezBlock = LEZ_BLOCK;
        r.lezFinalBlock = LEZ_BLOCK + DEPTH_BLOCKS;
        r.signatures = _sigs(_two(), _root(), r.lezBlock, r.lezFinalBlock);
    }

    function _nullifier(CommitteeBurnVerifier.Release memory r) internal view returns (bytes32) {
        return _nf(r.secret, r.index);
    }

    /// Send `r` as a release with the root and nullifier its opening leads to.
    function _release(CommitteeBurnVerifier.Release memory r) internal {
        bytes32 root = _rootOf(_leaf(r.token, r.amount, _ec(r.recipient, r.fee, r.secret)), r.index, r.path);
        v.release(ETH, r.amount, payable(r.recipient), payable(r.relayer), r.fee, root, _nullifier(r), abi.encode(r));
    }

    /// The same, expected to revert with `err` (the arguments are computed first: the sha256
    /// precompile is a call, and `expectRevert` binds to the next one).
    function _refused(CommitteeBurnVerifier.Release memory r, bytes4 err) internal {
        bytes32 root = _rootOf(_leaf(r.token, r.amount, _ec(r.recipient, r.fee, r.secret)), r.index, r.path);
        (bytes32 nf, bytes memory proof) = (_nullifier(r), abi.encode(r));
        vm.expectRevert(err);
        v.release(ETH, r.amount, payable(r.recipient), payable(r.relayer), r.fee, root, nf, proof);
    }

    // ---- the valid release ----

    function test_a_final_attested_burn_releases_once_to_its_committed_recipient() public {
        CommitteeBurnVerifier.Release memory r = _carol();
        uint256 before = address(v).balance;
        vm.prank(relayer);
        _release(r);
        assertEq(carol.balance, ONE - FEE, "the recipient the burn committed to");
        assertEq(relayer.balance, FEE, "the fee the burn committed to");
        assertEq(address(v).balance, before - ONE);
        assertTrue(v.nullifierSpent(_nullifier(r)));
    }

    function test_replay_is_refused() public {
        CommitteeBurnVerifier.Release memory r = _carol();
        _release(r);
        _refused(r, WrapVault.AlreadyReleased.selector);
        assertEq(carol.balance, ONE - FEE);
    }

    /// The nullifier is derived from the opening, not chosen: a fresh one for the same leaf is refused.
    function test_a_second_nullifier_for_the_same_burn_is_refused() public {
        CommitteeBurnVerifier.Release memory r = _carol();
        _release(r);
        (bytes32 root, bytes memory proof) = (_root(), abi.encode(r));
        vm.expectRevert(CommitteeBurnVerifier.NotThisRelease.selector);
        v.release(ETH, ONE, payable(carol), payable(relayer), FEE, root, keccak256("another nullifier"), proof);
    }

    // ---- what the burner committed to cannot change ----

    function test_another_recipient_is_refused() public {
        CommitteeBurnVerifier.Release memory r = _carol();
        r.recipient = mallory;
        // Its opening leads to another root, which no member signed.
        _refused(r, BurnRootCommittee.RootNotAttested.selector);
        // Or the true root named with mallory as the recipient: not this release.
        r = _carol();
        (bytes32 root, bytes32 nf, bytes memory proof) = (_root(), _nullifier(r), abi.encode(r));
        vm.expectRevert(CommitteeBurnVerifier.NotThisRelease.selector);
        v.release(ETH, ONE, payable(mallory), payable(relayer), FEE, root, nf, proof);
    }

    function test_another_amount_is_refused() public {
        CommitteeBurnVerifier.Release memory r = _carol();
        r.amount = 0.1 ether;
        _refused(r, BurnRootCommittee.RootNotAttested.selector);
        r = _carol();
        (bytes32 root, bytes32 nf, bytes memory proof) = (_root(), _nullifier(r), abi.encode(r));
        vm.expectRevert(CommitteeBurnVerifier.NotThisRelease.selector);
        v.release(ETH, 0.1 ether, payable(carol), payable(relayer), FEE, root, nf, proof);
    }

    function test_a_higher_fee_is_refused_and_a_front_runner_earns_only_the_committed_fee() public {
        CommitteeBurnVerifier.Release memory r = _carol();
        r.fee = 0.5 ether;
        _refused(r, BurnRootCommittee.RootNotAttested.selector);
        // Another relayer may submit the same opening: it earns the fee carol fixed, no more.
        r = _carol();
        r.relayer = mallory;
        vm.prank(mallory);
        _release(r);
        assertEq(mallory.balance, FEE);
        assertEq(carol.balance, ONE - FEE);
    }

    function test_a_relayer_other_than_the_proofs_is_refused() public {
        CommitteeBurnVerifier.Release memory r = _carol();
        (bytes32 root, bytes32 nf, bytes memory proof) = (_root(), _nullifier(r), abi.encode(r));
        vm.expectRevert(CommitteeBurnVerifier.NotThisRelease.selector);
        v.release(ETH, ONE, payable(carol), payable(mallory), FEE, root, nf, proof);
    }

    function test_another_index_or_path_is_refused() public {
        CommitteeBurnVerifier.Release memory r = _carol();
        r.index = 0;
        _refused(r, BurnRootCommittee.RootNotAttested.selector);
        r = _carol();
        r.path[3] = keccak256("not a node");
        _refused(r, BurnRootCommittee.RootNotAttested.selector);
        r = _carol();
        r.index = uint64(1 << 24);
        _refused(r, CommitteeBurnVerifier.BadOpening.selector);
    }

    // ---- finality ----

    function test_a_burn_not_yet_final_by_the_configured_depth_is_refused() public {
        CommitteeBurnVerifier.Release memory r = _carol();
        r.lezFinalBlock = r.lezBlock + DEPTH_BLOCKS - 1;
        r.signatures = _sigs(_two(), _root(), r.lezBlock, r.lezFinalBlock);
        _refused(r, BurnRootCommittee.NotFinal.selector);
        // Signed as final, then presented with a later final block: the signatures no longer match.
        r = _carol();
        r.lezFinalBlock += 10;
        _refused(r, BurnRootCommittee.RootNotAttested.selector);
    }

    // ---- forged burns and forged attestations ----

    function test_a_burn_that_never_happened_is_refused() public {
        // Mallory makes up a leaf paying herself, and a tree holding only it; non-members sign.
        bytes32[] memory fake = new bytes32[](1);
        fake[0] = _leaf(ETH, ONE, _ec(mallory, 0, bytes32(uint256(666))));
        CommitteeBurnVerifier.Release memory r;
        r.token = ETH;
        r.amount = ONE;
        r.recipient = mallory;
        r.relayer = mallory;
        r.secret = bytes32(uint256(666));
        r.path = _path(fake, 0);
        r.lezBlock = LEZ_BLOCK;
        r.lezFinalBlock = LEZ_BLOCK + DEPTH_BLOCKS;
        bytes32 root = _rootOf(fake[0], 0, r.path);
        r.signatures = new bytes[](3);
        for (uint256 i; i < 3; ++i) r.signatures[i] = _sign(0xBADBAD + i, root, r.lezBlock, r.lezFinalBlock, address(v));
        _refused(r, BurnRootCommittee.RootNotAttested.selector);
        // One member alone is below the threshold.
        uint256[] memory one = new uint256[](1);
        one[0] = pks[1];
        r.signatures = _sigs(one, root, r.lezBlock, r.lezFinalBlock);
        _refused(r, BurnRootCommittee.RootNotAttested.selector);
        // One member and one outsider: still one member.
        r.signatures = new bytes[](2);
        r.signatures[0] = _sign(pks[1], root, r.lezBlock, r.lezFinalBlock, address(v));
        r.signatures[1] = _sign(0xBADBAD, root, r.lezBlock, r.lezFinalBlock, address(v));
        _refused(r, BurnRootCommittee.RootNotAttested.selector);
        assertEq(mallory.balance, 0);
    }

    function test_one_member_signing_twice_counts_once() public {
        CommitteeBurnVerifier.Release memory r = _carol();
        bytes memory s = _sign(pks[0], _root(), r.lezBlock, r.lezFinalBlock, address(v));
        r.signatures = new bytes[](3);
        r.signatures[0] = s;
        r.signatures[1] = s;
        r.signatures[2] = s;
        _refused(r, BurnRootCommittee.RootNotAttested.selector);
    }

    function test_a_malleated_signature_does_not_count() public {
        CommitteeBurnVerifier.Release memory r = _carol();
        bytes memory s = r.signatures[1];
        bytes32 rr;
        bytes32 ss;
        uint8 vv;
        assembly {
            rr := mload(add(s, 32))
            ss := mload(add(s, 64))
            vv := byte(0, mload(add(s, 96)))
        }
        // (r, n - s, v ^ 1) recovers the same signer; it must be refused, and alone the
        // remaining member is below the threshold.
        uint256 n = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;
        r.signatures[1] = abi.encodePacked(rr, bytes32(n - uint256(ss)), vv == 27 ? uint8(28) : uint8(27));
        _refused(r, BurnRootCommittee.RootNotAttested.selector);
    }

    function test_an_attestation_for_another_vault_or_chain_is_refused() public {
        CommitteeBurnVerifier.Release memory r = _carol();
        r.signatures[0] = _sign(pks[0], _root(), r.lezBlock, r.lezFinalBlock, address(0xD1FF));
        r.signatures[1] = _sign(pks[2], _root(), r.lezBlock, r.lezFinalBlock, address(0xD1FF));
        _refused(r, BurnRootCommittee.RootNotAttested.selector);
        r = _carol();
        vm.chainId(1);
        _refused(r, BurnRootCommittee.RootNotAttested.selector);
    }

    function test_another_pairing_is_refused() public {
        // A vault paired with another LEZ deployment, same committee: carol's leaf is not its.
        WrapVault other = new WrapVault(keccak256("another pairing"), cv, 1 days, admin, admin);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = ONE;
        vm.prank(admin);
        other.registerToken(ETH, 18, amounts, 100 ether, 100 ether);
        vm.prank(alice);
        other.deposit{value: ONE}(ETH, ONE, bytes32(0));
        CommitteeBurnVerifier.Release memory r = _carol();
        r.signatures[0] = _sign(pks[0], _root(), r.lezBlock, r.lezFinalBlock, address(other));
        r.signatures[1] = _sign(pks[2], _root(), r.lezBlock, r.lezFinalBlock, address(other));
        (bytes32 root, bytes32 nf, bytes memory proof) = (_root(), _nullifier(r), abi.encode(r));
        vm.expectRevert(CommitteeBurnVerifier.NotThisRelease.selector);
        other.release(ETH, ONE, payable(carol), payable(relayer), FEE, root, nf, proof);
    }

    function test_only_the_vault_asks() public {
        // Called directly, `msg.sender` is not a vault: there is no pairing to read.
        CommitteeBurnVerifier.Release memory r = _carol();
        vm.expectRevert();
        cv.verify(abi.encode(r), bytes32(0));
    }

    function test_the_committee_is_fixed_and_well_formed() public {
        address[] memory m = new address[](2);
        m[0] = address(1);
        m[1] = address(1);
        vm.expectRevert(BurnRootCommittee.BadConfig.selector);
        new CommitteeBurnVerifier(m, 1, 3);
        m[1] = address(0);
        vm.expectRevert(BurnRootCommittee.BadConfig.selector);
        new CommitteeBurnVerifier(m, 1, 3);
        m[1] = address(2);
        vm.expectRevert(BurnRootCommittee.BadConfig.selector);
        new CommitteeBurnVerifier(m, 0, 3);
        vm.expectRevert(BurnRootCommittee.BadConfig.selector);
        new CommitteeBurnVerifier(m, 3, 3);
        assertEq(cv.members().length, 3);
        assertEq(cv.THRESHOLD(), 2);
    }

    /// The vectors of core/src/lib.rs (`burn_hashes_are_the_verifiers`).
    function test_burn_hashes_are_shared_with_rust() public {
        address token = address(2);
        bytes32 ec = cv.ethCommitment(address(3), 5, bytes32(uint256(0x5555555555555555555555555555555555555555555555555555555555555555)));
        assertEq(ec, 0xbbc8856a71249b9f3254eb8b6e962896d2f85388b584979cb7bf38358baba7b4);
        bytes32 p = 0x1111111111111111111111111111111111111111111111111111111111111111;
        assertEq(cv.burnLeaf(p, token, 1 ether, ec), 0x91a62580a02a2a83385299bd6ee8516d079cf61deee0bba79c6ace84a44a5ab8);
        assertEq(
            cv.burnNullifier(p, bytes32(uint256(0x5555555555555555555555555555555555555555555555555555555555555555)), 7),
            0xc191ff0e20e847734710bc669edc21fcb03d0fd5b4eea2c1b603a9269eefb9ab
        );
        vm.chainId(31337);
        assertEq(
            cv.attestationDigest(address(1), p, 0x3333333333333333333333333333333333333333333333333333333333333333, 40, 43),
            0x246310dd736d66ce3cb00c7d265f7d4556e56779f880e799f062f1c574b308c3
        );
    }

    function test_release_gas() public {
        CommitteeBurnVerifier.Release memory r = _carol();
        uint256 g = gasleft();
        _release(r);
        emit log_named_uint("release through the committee verifier, 2 signatures (gas, test-measured)", g - gasleft());
    }
}

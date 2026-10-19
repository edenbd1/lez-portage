// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {WrapVault} from "../src/WrapVault.sol";
import {ZkBurnVerifier} from "../src/ZkBurnVerifier.sol";
import {BurnRootCommittee} from "../src/BurnRootCommittee.sol";
import {IRiscZeroVerifier, VerificationFailed} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroGroth16Verifier, SelectorMismatch} from "risc0/groth16/RiscZeroGroth16Verifier.sol";
import {ControlID} from "risc0/groth16/ControlID.sol";

/// F.7 part 1 in zero knowledge: a release carries a real Groth16 seal, made by the prover of this
/// repository (scripts/zk-fixture.sh: the guest zk/burn_membership, proven by r0vm 3.0.5 and
/// wrapped by RISC Zero's Groth16 prover), checked by RISC Zero's own RiscZeroGroth16Verifier
/// (risc0-ethereum v3.0.1). The burn tree is CommitteeBurnVerifier.t.sol's: three burns, carol's
/// 1 ETH at index 1 with a 0.01 ETH relayer fee. The vault sits where the fixture's journal says
/// (VAULT, chain 31337). Every refusal a release can meet, one test each.
contract ZkBurnVerifierTest is Test {
    WrapVault v;
    ZkBurnVerifier zv;
    RiscZeroGroth16Verifier risc0;
    address constant ETH = address(0);
    address constant VAULT = address(0xfa22);
    bytes32 constant PAIRING = keccak256("test pairing");
    bytes32 constant RELEASE_TAG = "rfp022/wrap/release/v1";
    uint64 constant DEPTH_BLOCKS = 3;
    uint256 constant ONE = 1 ether;
    uint256 constant FEE = 0.01 ether;
    uint64 constant LEZ_BLOCK = 40;
    uint256[3] pks = [uint256(0xC0FFEE01), 0xC0FFEE02, 0xC0FFEE03];
    uint256[2] outsiders = [uint256(0xBAD01), 0xBAD02];
    address admin = address(0xA11CE);
    address alice = address(0xA1);
    address carol = address(0xCA);
    address relayer = address(0x7E1A);
    address mallory = address(0xBAD);

    // The fixture: what the prover published, and the burn it proves (known to the test, never
    // to the vault).
    bytes seal;
    bytes journal;
    bytes32 imageId;
    bytes32 root;
    bytes32 nullifier;
    bytes32 constant CAROL_SECRET = bytes32(uint256(0xCA5EC2E7));
    bytes32[] leaves;

    function setUp() public {
        string memory f = vm.readFile("test/fixtures/zk_release.json");
        seal = vm.parseJsonBytes(f, ".seal");
        journal = vm.parseJsonBytes(f, ".journal");
        imageId = vm.parseJsonBytes32(f, ".image_id");
        root = vm.parseJsonBytes32(f, ".fields.root");
        nullifier = vm.parseJsonBytes32(f, ".fields.nullifier");

        risc0 = new RiscZeroGroth16Verifier(ControlID.CONTROL_ROOT, ControlID.BN254_CONTROL_ID);
        zv = new ZkBurnVerifier(risc0, imageId, _members(), 2, DEPTH_BLOCKS);
        deployCodeTo("WrapVault.sol:WrapVault", abi.encode(PAIRING, zv, uint64(1 days), admin, admin), VAULT);
        v = WrapVault(payable(VAULT));
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

    function _members() internal view returns (address[] memory m) {
        m = new address[](3);
        for (uint256 i; i < 3; ++i) m[i] = vm.addr(pks[i]);
    }

    // ---- the hashes, an independent copy of wrap-core's ----

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

    function _att(address vault, bytes32 r, uint64 b, uint64 f) internal view returns (bytes32) {
        return keccak256(abi.encode(bytes32("rfp022/burn-root-attestation/v1"), block.chainid, vault, PAIRING, r, uint256(b), uint256(f)));
    }

    function _sign(uint256 pk, bytes32 r, uint64 b, uint64 f) internal view returns (bytes memory) {
        (uint8 vv, bytes32 rr, bytes32 s) = vm.sign(pk, keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", _att(VAULT, r, b, f))));
        return abi.encodePacked(rr, s, vv);
    }

    function _sigs(uint256[] memory who, bytes32 r, uint64 b, uint64 f) internal view returns (bytes[] memory s) {
        s = new bytes[](who.length);
        for (uint256 i; i < who.length; ++i) s[i] = _sign(who[i], r, b, f);
    }

    function _two() internal view returns (uint256[] memory w) {
        w = new uint256[](2);
        w[0] = pks[0];
        w[1] = pks[2];
    }

    /// Carol's honest release: the fixture's seal and journal, its root attested by members 1 and 3
    /// at a final block.
    function _carol() internal view returns (ZkBurnVerifier.Release memory r) {
        r.seal = seal;
        r.journal = journal;
        r.lezBlock = LEZ_BLOCK;
        r.lezFinalBlock = LEZ_BLOCK + DEPTH_BLOCKS;
        r.signatures = _sigs(_two(), root, r.lezBlock, r.lezFinalBlock);
    }

    /// The release's arguments; each test changes one.
    struct Args {
        uint256 amount;
        address recipient;
        address relayer;
        uint256 fee;
        bytes32 root;
        bytes32 nullifier;
    }

    function _args() internal view returns (Args memory) {
        return Args(ONE, carol, relayer, FEE, root, nullifier);
    }

    function _release(Args memory a, ZkBurnVerifier.Release memory r) internal {
        v.release(ETH, a.amount, payable(a.recipient), payable(a.relayer), a.fee, a.root, a.nullifier, abi.encode(r));
    }

    function _refused(Args memory a, ZkBurnVerifier.Release memory r, bytes memory err) internal {
        bytes memory proof = abi.encode(r);
        vm.expectRevert(err);
        v.release(ETH, a.amount, payable(a.recipient), payable(a.relayer), a.fee, a.root, a.nullifier, proof);
    }

    function _sel(bytes4 s) internal pure returns (bytes memory) {
        return abi.encodePacked(s);
    }

    /// The journal with word `i` replaced.
    function _journalWith(uint256 i, bytes32 w) internal view returns (bytes memory j) {
        j = bytes.concat(journal);
        for (uint256 k; k < 32; ++k) j[32 * i + k] = w[k];
    }

    // ---- the fixture is what it claims to be ----

    /// The prover's journal is the vault's release preimage of the test tree's root and of carol's
    /// nullifier, computed here independently; the seal carries this verifier's selector; the
    /// verifier parameters are risc0-ethereum v3.0.1's (the prover's control root and BN254
    /// control id, which r0vm 3.0.5 printed).
    function test_the_fixture_is_the_test_trees_release() public view {
        bytes32 r = leaves[1];
        bytes32[24] memory p = _path(leaves, 1);
        for (uint256 l; l < 24; ++l) r = (1 >> l) & 1 == 1 ? _h(p[l], r) : _h(r, p[l]);
        assertEq(r, root, "the root of the test tree");
        bytes32 nf = sha256(abi.encodePacked(bytes("rfp022/burn-nullifier/v1"), PAIRING, CAROL_SECRET, _le(1, 8)));
        assertEq(nf, nullifier, "carol's burn nullifier");
        assertEq(
            journal,
            abi.encode(RELEASE_TAG, uint256(31337), VAULT, PAIRING, ETH, ONE, carol, relayer, FEE, root, nullifier),
            "the release preimage"
        );
        assertEq(sha256(journal), v.releaseDigest(ETH, ONE, carol, relayer, FEE, root, nullifier));
        assertEq(bytes4(seal), risc0.SELECTOR(), "the seal's selector");
        assertEq(seal.length, 260, "selector and a Groth16 proof (a, b, c)");
        assertEq(address(v.VERIFIER()), address(zv));
    }

    /// Nothing the release sends names the leaf, the secret or a node of the path.
    function test_the_release_calldata_omits_the_opening() public view {
        bytes memory data = abi.encodeCall(
            WrapVault.release, (ETH, ONE, payable(carol), payable(relayer), FEE, root, nullifier, abi.encode(_carol()))
        );
        bytes32[24] memory p = _path(leaves, 1);
        assertFalse(_contains(data, leaves[1]), "the leaf");
        assertFalse(_contains(data, CAROL_SECRET), "the secret");
        assertFalse(_contains(data, _ec(carol, FEE, CAROL_SECRET)), "the Ethereum commitment");
        for (uint256 l; l < 24; ++l) assertFalse(_contains(data, p[l]) && p[l] != bytes32(0), "a path node");
    }

    function _contains(bytes memory hay, bytes32 w) internal pure returns (bool) {
        if (hay.length < 32) return false;
        for (uint256 i; i + 32 <= hay.length; ++i) {
            bytes32 x;
            assembly {
                x := mload(add(add(hay, 32), i))
            }
            if (x == w) return true;
        }
        return false;
    }

    // ---- the valid release ----

    function test_a_proven_attested_burn_releases_once_to_its_committed_recipient() public {
        uint256 before = address(v).balance;
        vm.prank(relayer);
        uint256 g = gasleft();
        _release(_args(), _carol());
        emit log_named_uint("release gas (call, excl. 21000 and calldata)", g - gasleft());
        assertEq(carol.balance, ONE - FEE, "the recipient the burn committed to");
        assertEq(relayer.balance, FEE, "the fee the burn committed to");
        assertEq(address(v).balance, before - ONE);
        assertTrue(v.nullifierSpent(nullifier));
    }

    function test_replay_is_refused() public {
        _release(_args(), _carol());
        _refused(_args(), _carol(), _sel(WrapVault.AlreadyReleased.selector));
        assertEq(carol.balance, ONE - FEE);
    }

    // ---- a release other than the one proven: the vault's digest is not the journal's ----

    function test_another_recipient_is_refused() public {
        Args memory a = _args();
        a.recipient = mallory;
        _refused(a, _carol(), _sel(ZkBurnVerifier.NotThisRelease.selector));
    }

    function test_another_amount_is_refused() public {
        Args memory a = _args();
        a.amount = 0.1 ether;
        _refused(a, _carol(), _sel(ZkBurnVerifier.NotThisRelease.selector));
    }

    function test_a_higher_fee_is_refused() public {
        Args memory a = _args();
        a.fee = 0.5 ether;
        _refused(a, _carol(), _sel(ZkBurnVerifier.NotThisRelease.selector));
    }

    /// The relayer is in the journal: a copied proof cannot be front-run for its fee.
    function test_another_relayer_is_refused() public {
        Args memory a = _args();
        a.relayer = mallory;
        _refused(a, _carol(), _sel(ZkBurnVerifier.NotThisRelease.selector));
    }

    function test_another_root_is_refused() public {
        Args memory a = _args();
        a.root = keccak256("another root");
        ZkBurnVerifier.Release memory r = _carol();
        r.signatures = _sigs(_two(), a.root, r.lezBlock, r.lezFinalBlock);
        _refused(a, r, _sel(ZkBurnVerifier.NotThisRelease.selector));
    }

    function test_another_nullifier_is_refused() public {
        Args memory a = _args();
        a.nullifier = keccak256("another nullifier");
        _refused(a, _carol(), _sel(ZkBurnVerifier.NotThisRelease.selector));
    }

    function test_another_chain_is_refused() public {
        vm.chainId(1);
        ZkBurnVerifier.Release memory r = _carol();
        _refused(_args(), r, _sel(ZkBurnVerifier.NotThisRelease.selector));
    }

    /// The same proof sent to another vault of the same pairing and verifier.
    function test_another_vault_is_refused() public {
        WrapVault other = new WrapVault(PAIRING, zv, 1 days, admin, admin);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = ONE;
        vm.prank(admin);
        other.registerToken(ETH, 18, amounts, 100 ether, 100 ether);
        vm.deal(address(other), ONE);
        bytes memory proof = abi.encode(_carol());
        vm.expectRevert(ZkBurnVerifier.NotThisRelease.selector);
        other.release(ETH, ONE, payable(carol), payable(relayer), FEE, root, nullifier, proof);
    }

    // ---- a journal changed to match changed arguments: the seal does not prove it ----

    function test_a_journal_tampered_to_another_recipient_is_refused() public {
        Args memory a = _args();
        a.recipient = mallory;
        ZkBurnVerifier.Release memory r = _carol();
        r.journal = _journalWith(6, bytes32(uint256(uint160(mallory))));
        assertEq(sha256(r.journal), v.releaseDigest(ETH, a.amount, a.recipient, a.relayer, a.fee, a.root, a.nullifier));
        _refused(a, r, _sel(VerificationFailed.selector));
    }

    function test_a_journal_tampered_to_another_amount_is_refused() public {
        Args memory a = _args();
        a.amount = 0.1 ether;
        ZkBurnVerifier.Release memory r = _carol();
        r.journal = _journalWith(5, bytes32(a.amount));
        _refused(a, r, _sel(VerificationFailed.selector));
    }

    function test_a_journal_tampered_to_another_fee_is_refused() public {
        Args memory a = _args();
        a.fee = 0.5 ether;
        ZkBurnVerifier.Release memory r = _carol();
        r.journal = _journalWith(8, bytes32(a.fee));
        _refused(a, r, _sel(VerificationFailed.selector));
    }

    /// A root the committee does sign, with a journal claiming it: no proof of membership under it.
    function test_a_journal_tampered_to_another_root_is_refused() public {
        Args memory a = _args();
        a.root = keccak256("a root the committee signs");
        ZkBurnVerifier.Release memory r = _carol();
        r.journal = _journalWith(9, a.root);
        r.signatures = _sigs(_two(), a.root, r.lezBlock, r.lezFinalBlock);
        _refused(a, r, _sel(VerificationFailed.selector));
    }

    function test_a_journal_tampered_to_another_nullifier_is_refused() public {
        Args memory a = _args();
        a.nullifier = keccak256("a fresh nullifier");
        ZkBurnVerifier.Release memory r = _carol();
        r.journal = _journalWith(10, a.nullifier);
        _refused(a, r, _sel(VerificationFailed.selector));
    }

    // ---- the seal ----

    function test_a_tampered_seal_is_refused() public {
        ZkBurnVerifier.Release memory r = _carol();
        r.seal = bytes.concat(seal);
        r.seal[100] = bytes1(uint8(r.seal[100]) ^ 1);
        _refused(_args(), r, _sel(VerificationFailed.selector));
    }

    function test_a_seal_for_another_verifier_version_is_refused() public {
        ZkBurnVerifier.Release memory r = _carol();
        r.seal = bytes.concat(seal);
        r.seal[0] = bytes1(uint8(r.seal[0]) ^ 0xff);
        bytes4 got = bytes4(r.seal);
        _refused(_args(), r, abi.encodeWithSelector(SelectorMismatch.selector, got, risc0.SELECTOR()));
    }

    /// A verifier pinned to another guest refuses this guest's receipt.
    function test_another_image_id_is_refused() public {
        ZkBurnVerifier other = new ZkBurnVerifier(risc0, keccak256("another guest"), _members(), 2, DEPTH_BLOCKS);
        bytes memory proof = abi.encode(_carol());
        bytes32 digest = v.releaseDigest(ETH, ONE, carol, relayer, FEE, root, nullifier);
        vm.prank(VAULT);
        vm.expectRevert(VerificationFailed.selector);
        other.verify(proof, digest);
        // The same call to the verifier pinned to the fixture's guest passes.
        vm.prank(VAULT);
        assertTrue(zv.verify(proof, digest));
    }

    // ---- the root: still the committee's ----

    function test_a_root_the_committee_did_not_sign_is_refused() public {
        ZkBurnVerifier.Release memory r = _carol();
        r.signatures = _sigs(_two(), keccak256("the root the members read"), r.lezBlock, r.lezFinalBlock);
        _refused(_args(), r, _sel(BurnRootCommittee.RootNotAttested.selector));
    }

    function test_a_root_not_yet_final_is_refused() public {
        ZkBurnVerifier.Release memory r = _carol();
        r.lezFinalBlock = LEZ_BLOCK + DEPTH_BLOCKS - 1;
        r.signatures = _sigs(_two(), root, r.lezBlock, r.lezFinalBlock);
        _refused(_args(), r, _sel(BurnRootCommittee.NotFinal.selector));
    }

    function test_one_member_alone_is_refused() public {
        ZkBurnVerifier.Release memory r = _carol();
        uint256[] memory one = new uint256[](2);
        one[0] = pks[1];
        one[1] = pks[1];
        r.signatures = _sigs(one, root, r.lezBlock, r.lezFinalBlock);
        _refused(_args(), r, _sel(BurnRootCommittee.RootNotAttested.selector));
    }

    function test_outsiders_are_refused() public {
        ZkBurnVerifier.Release memory r = _carol();
        uint256[] memory o = new uint256[](2);
        o[0] = outsiders[0];
        o[1] = outsiders[1];
        r.signatures = _sigs(o, root, r.lezBlock, r.lezFinalBlock);
        _refused(_args(), r, _sel(BurnRootCommittee.RootNotAttested.selector));
    }

    /// The attestation binds the pairing the journal (and so the vault) names.
    function test_an_attestation_for_another_pairing_is_refused() public {
        ZkBurnVerifier.Release memory r = _carol();
        bytes32 d = keccak256(
            abi.encode(bytes32("rfp022/burn-root-attestation/v1"), block.chainid, VAULT, keccak256("other pairing"), root, uint256(r.lezBlock), uint256(r.lezFinalBlock))
        );
        for (uint256 i; i < 2; ++i) {
            (uint8 vv, bytes32 rr, bytes32 s) = vm.sign(_two()[i], keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", d)));
            r.signatures[i] = abi.encodePacked(rr, s, vv);
        }
        _refused(_args(), r, _sel(BurnRootCommittee.RootNotAttested.selector));
    }

    // ---- configuration ----

    function test_a_verifier_needs_a_risc0_verifier_and_an_image_id() public {
        vm.expectRevert(BurnRootCommittee.BadConfig.selector);
        new ZkBurnVerifier(IRiscZeroVerifier(address(0)), imageId, _members(), 2, DEPTH_BLOCKS);
        vm.expectRevert(BurnRootCommittee.BadConfig.selector);
        new ZkBurnVerifier(risc0, bytes32(0), _members(), 2, DEPTH_BLOCKS);
        vm.expectRevert(BurnRootCommittee.BadConfig.selector);
        new ZkBurnVerifier(risc0, imageId, _members(), 4, DEPTH_BLOCKS);
    }
}

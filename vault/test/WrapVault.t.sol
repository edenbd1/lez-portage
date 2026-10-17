// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {Test, Vm} from "forge-std/Test.sol";
import {WrapVault} from "../src/WrapVault.sol";
import {MockERC20, MockUSDT, MockFalseToken, MockFeeToken, MockVerifier} from "./Mocks.sol";

contract WrapVaultTest is Test {
    WrapVault v;
    MockVerifier ver;
    MockERC20 usdc;
    MockERC20 dai;
    MockUSDT usdt;
    address constant ETH = address(0);
    address admin = address(0xA11CE);
    address freezer = address(0xF2EE);
    address alice = address(0xA1);
    address bob = address(0xB0B);
    address relayer = address(0x7E1A);
    bytes32 constant PAIRING = keccak256("test pairing");
    uint64 constant WINDOW = 1 days;

    function setUp() public {
        ver = new MockVerifier();
        v = new WrapVault(PAIRING, ver, WINDOW, admin, freezer);
        usdc = new MockERC20("USDC", 6);
        dai = new MockERC20("DAI", 18);
        usdt = new MockUSDT();
        vm.startPrank(admin);
        v.registerToken(ETH, 18, _amounts(0.1 ether, 1 ether), 100 ether, 100 ether);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 10_000e6, 10_000e6);
        v.registerToken(address(dai), 18, _amounts(100e18, 1000e18), 10_000e18, 10_000e18);
        v.registerToken(address(usdt), 6, _amounts(100e6, 1000e6), 10_000e6, 10_000e6);
        vm.stopPrank();
        vm.deal(alice, 1000 ether);
        usdc.mint(alice, 1e12);
        dai.mint(alice, 1e24);
        usdt.mint(alice, 1e12);
        vm.startPrank(alice);
        usdc.approve(address(v), type(uint256).max);
        dai.approve(address(v), type(uint256).max);
        usdt.approve(address(v), type(uint256).max);
        vm.stopPrank();
    }

    function _amounts(uint256 a, uint256 b) internal pure returns (uint256[] memory r) {
        r = new uint256[](2);
        r[0] = a;
        r[1] = b;
    }

    function _h(bytes32 a, bytes32 b) internal pure returns (bytes32) {
        return sha256(abi.encodePacked(a, b));
    }

    /// The root of the depth-24 tree holding `leaves`, computed naively.
    function _naiveRoot(bytes32[] memory leaves) internal pure returns (bytes32) {
        bytes32[] memory layer = leaves;
        bytes32 z;
        for (uint256 d; d < 24; ++d) {
            uint256 n = (layer.length + 1) / 2;
            bytes32[] memory next = new bytes32[](n);
            for (uint256 i; i < n; ++i) {
                bytes32 l = layer[2 * i];
                bytes32 r = 2 * i + 1 < layer.length ? layer[2 * i + 1] : z;
                next[i] = _h(l, r);
            }
            if (layer.length == 0) next = new bytes32[](0);
            layer = next;
            z = _h(z, z);
        }
        return layer.length == 0 ? z : layer[0];
    }

    // ---- the tree and the slot the LEZ program proves ----

    function test_knownRoot_is_mapping_at_slot_0() public {
        bytes32 r = v.root();
        assertEq(vm.load(address(v), keccak256(abi.encode(r, uint256(0)))), bytes32(uint256(1)));
        vm.prank(alice);
        v.deposit{value: 1 ether}(ETH, 1 ether, bytes32(uint256(7)));
        bytes32 r2 = v.root();
        assertTrue(r2 != r);
        assertEq(vm.load(address(v), keccak256(abi.encode(r2, uint256(0)))), bytes32(uint256(1)));
        // An old root stays known: a claim built against it never expires (R.4).
        assertEq(vm.load(address(v), keccak256(abi.encode(r, uint256(0)))), bytes32(uint256(1)));
        assertEq(vm.load(address(v), keccak256(abi.encode(bytes32(uint256(123)), uint256(0)))), bytes32(0));
    }

    function test_empty_root_and_incremental_root_match_naive_tree() public {
        assertEq(v.root(), _naiveRoot(new bytes32[](0)));
        bytes32[] memory leaves = new bytes32[](5);
        for (uint256 i; i < 5; ++i) {
            bytes32 c = bytes32(i + 1);
            leaves[i] = v.leafOf(ETH, 0.1 ether, c);
            vm.prank(alice);
            assertEq(v.deposit{value: 0.1 ether}(ETH, 0.1 ether, c), i);
            bytes32[] memory prefix = new bytes32[](i + 1);
            for (uint256 j; j <= i; ++j) prefix[j] = leaves[j];
            assertEq(v.root(), _naiveRoot(prefix));
        }
    }

    function test_leaf_binds_chain_vault_pairing_token_amount_commitment() public view {
        bytes32 c = bytes32(uint256(42));
        bytes32 expect = sha256(abi.encode(v.LEAF_TAG(), block.chainid, address(v), PAIRING, address(usdc), uint256(100e6), c));
        assertEq(v.leafOf(address(usdc), 100e6, c), expect);
        assertTrue(v.leafOf(address(usdc), 1000e6, c) != expect);
        assertTrue(v.leafOf(address(usdt), 100e6, c) != expect);
    }

    // ---- deposits (F.1, F.2, F.8, F.17) ----

    function test_deposit_eth_directly_without_wrapping() public {
        bytes32 c = keccak256("commitment");
        vm.recordLogs();
        vm.prank(alice);
        v.deposit{value: 1 ether}(ETH, 1 ether, c);
        assertEq(address(v).balance, 1 ether);
        Vm.Log[] memory logs = vm.getRecordedLogs();
        assertEq(logs.length, 1);
        // The event carries the index, leaf, commitment, token, amount and root: nothing names
        // a LEZ account (F.2).
        (bytes32 leaf, bytes32 cc, uint256 amount, bytes32 r) = abi.decode(logs[0].data, (bytes32, bytes32, uint256, bytes32));
        assertEq(leaf, v.leafOf(ETH, 1 ether, c));
        assertEq(cc, c);
        assertEq(amount, 1 ether);
        assertEq(r, v.root());
    }

    function test_deposit_6_and_18_decimal_tokens() public {
        vm.startPrank(alice);
        v.deposit(address(usdc), 100e6, bytes32(uint256(1)));
        v.deposit(address(dai), 1000e18, bytes32(uint256(2)));
        vm.stopPrank();
        assertEq(usdc.balanceOf(address(v)), 100e6);
        assertEq(dai.balanceOf(address(v)), 1000e18);
        assertEq(v.nextIndex(), 2);
    }

    function test_deposit_and_release_usdt_which_returns_nothing() public {
        vm.prank(alice);
        v.deposit(address(usdt), 1000e6, bytes32(uint256(1)));
        assertEq(usdt.balanceOf(address(v)), 1000e6);
        bytes32 nf = keccak256("nf");
        ver.approve(v.releaseDigest(address(usdt), 1000e6, bob, relayer, 1e6, bytes32(uint256(9)), nf));
        v.release(address(usdt), 1000e6, payable(bob), payable(relayer), 1e6, bytes32(uint256(9)), nf, hex"01");
        assertEq(usdt.balanceOf(bob), 999e6);
        assertEq(usdt.balanceOf(relayer), 1e6);
    }

    function test_a_token_returning_false_is_refused() public {
        MockFalseToken f = new MockFalseToken();
        vm.prank(admin);
        v.registerToken(address(f), 18, _amounts(1e18, 2e18), 1e20, 1e20);
        f.mint(alice, 1e18);
        vm.prank(alice);
        vm.expectRevert(WrapVault.TransferFailed.selector);
        v.deposit(address(f), 1e18, bytes32(0));
    }

    function test_a_fee_on_transfer_token_is_refused() public {
        MockFeeToken f = new MockFeeToken();
        vm.prank(admin);
        v.registerToken(address(f), 18, _amounts(1e18, 2e18), 1e20, 1e20);
        f.mint(alice, 1e18);
        vm.startPrank(alice);
        f.approve(address(v), 1e18);
        vm.expectRevert(WrapVault.TransferFailed.selector);
        v.deposit(address(f), 1e18, bytes32(0));
        vm.stopPrank();
    }

    function test_only_permitted_amounts_and_listed_tokens() public {
        vm.startPrank(alice);
        vm.expectRevert(WrapVault.InvalidAmount.selector);
        v.deposit(address(usdc), 123e6, bytes32(0));
        vm.expectRevert(WrapVault.InvalidAmount.selector);
        v.deposit{value: 0.5 ether}(ETH, 0.5 ether, bytes32(0));
        vm.expectRevert(WrapVault.InvalidAmount.selector);
        v.deposit{value: 0.1 ether}(ETH, 1 ether, bytes32(0)); // value mismatch
        vm.expectRevert(WrapVault.InvalidAmount.selector);
        v.deposit{value: 1}(address(usdc), 100e6, bytes32(0)); // ETH sent with an ERC-20
        vm.expectRevert(WrapVault.UnsupportedToken.selector);
        v.deposit(address(0xDEAD), 100e6, bytes32(0));
        vm.stopPrank();
    }

    // ---- caps (F.11) ----

    function test_deposit_cap_per_window() public {
        vm.prank(admin);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 2000e6, 2000e6);
        vm.startPrank(alice);
        v.deposit(address(usdc), 1000e6, bytes32(0));
        v.deposit(address(usdc), 1000e6, bytes32(0));
        vm.expectRevert(WrapVault.CapExceeded.selector);
        v.deposit(address(usdc), 100e6, bytes32(0));
        vm.warp(block.timestamp + WINDOW);
        v.deposit(address(usdc), 100e6, bytes32(0));
        vm.stopPrank();
    }

    function test_release_cap_per_window() public {
        vm.prank(admin);
        v.registerToken(ETH, 18, _amounts(0.1 ether, 1 ether), 100 ether, 1 ether);
        vm.deal(address(v), 10 ether);
        bytes32 r = bytes32(uint256(5));
        ver.approve(v.releaseDigest(ETH, 1 ether, bob, relayer, 0, r, bytes32(uint256(1))));
        ver.approve(v.releaseDigest(ETH, 1 ether, bob, relayer, 0, r, bytes32(uint256(2))));
        v.release(ETH, 1 ether, payable(bob), payable(relayer), 0, r, bytes32(uint256(1)), hex"01");
        vm.expectRevert(WrapVault.CapExceeded.selector);
        v.release(ETH, 1 ether, payable(bob), payable(relayer), 0, r, bytes32(uint256(2)), hex"01");
        vm.warp(block.timestamp + WINDOW);
        v.release(ETH, 1 ether, payable(bob), payable(relayer), 0, r, bytes32(uint256(2)), hex"01");
        assertEq(bob.balance, 2 ether);
    }

    // ---- freeze (F.14, Security #5) ----

    function test_freeze_per_token_and_global_each_direction_independently() public {
        vm.deal(address(v), 10 ether);
        bytes32 r = bytes32(uint256(5));
        ver.approve(v.releaseDigest(ETH, 1 ether, bob, relayer, 0, r, bytes32(uint256(1))));

        vm.prank(freezer);
        v.freeze(ETH, true, false);
        vm.prank(alice);
        vm.expectRevert(WrapVault.IsFrozen.selector);
        v.deposit{value: 1 ether}(ETH, 1 ether, bytes32(0));
        vm.prank(alice);
        v.deposit(address(usdc), 100e6, bytes32(0)); // another token is unaffected
        v.release(ETH, 1 ether, payable(bob), payable(relayer), 0, r, bytes32(uint256(1)), hex"01"); // releases still run

        vm.prank(freezer);
        v.freezeAll(false, true);
        vm.prank(freezer);
        v.freeze(ETH, false, false);
        ver.approve(v.releaseDigest(ETH, 1 ether, bob, relayer, 0, r, bytes32(uint256(2))));
        vm.expectRevert(WrapVault.IsFrozen.selector);
        v.release(ETH, 1 ether, payable(bob), payable(relayer), 0, r, bytes32(uint256(2)), hex"01");
        vm.prank(alice);
        v.deposit{value: 1 ether}(ETH, 1 ether, bytes32(0)); // deposits run again

        vm.prank(alice);
        vm.expectRevert(WrapVault.NotFreezer.selector);
        v.freezeAll(true, true);
    }

    // ---- release (F.7, R.3, Security #2, #3, #6) ----

    function test_release_once_and_only_as_proven() public {
        vm.prank(alice);
        v.deposit{value: 1 ether}(ETH, 1 ether, bytes32(0));
        bytes32 r = bytes32(uint256(5));
        bytes32 nf = keccak256("burn 1");
        ver.approve(v.releaseDigest(ETH, 1 ether, bob, relayer, 0.01 ether, r, nf));

        // Anyone can submit, but a submitter cannot redirect, overcharge, or change the root.
        vm.expectRevert(WrapVault.VerificationFailed.selector);
        v.release(ETH, 1 ether, payable(alice), payable(relayer), 0.01 ether, r, nf, hex"01");
        vm.expectRevert(WrapVault.VerificationFailed.selector);
        v.release(ETH, 1 ether, payable(bob), payable(alice), 0.01 ether, r, nf, hex"01");
        vm.expectRevert(WrapVault.VerificationFailed.selector);
        v.release(ETH, 1 ether, payable(bob), payable(relayer), 0.02 ether, r, nf, hex"01");
        vm.expectRevert(WrapVault.VerificationFailed.selector);
        v.release(ETH, 1 ether, payable(bob), payable(relayer), 0.01 ether, bytes32(uint256(6)), nf, hex"01");
        vm.expectRevert(WrapVault.VerificationFailed.selector);
        v.release(ETH, 1 ether, payable(bob), payable(relayer), 0.01 ether, r, nf, hex"");

        vm.prank(address(0xCAFE));
        v.release(ETH, 1 ether, payable(bob), payable(relayer), 0.01 ether, r, nf, hex"01");
        assertEq(bob.balance, 0.99 ether);
        assertEq(relayer.balance, 0.01 ether);
        assertTrue(v.nullifierSpent(nf));

        vm.expectRevert(WrapVault.AlreadyReleased.selector);
        v.release(ETH, 1 ether, payable(bob), payable(relayer), 0.01 ether, r, nf, hex"01");
    }

    function test_release_refuses_bad_amounts_and_unregistered_tokens() public {
        bytes32 nf = bytes32(uint256(1));
        vm.expectRevert(WrapVault.InvalidAmount.selector);
        v.release(ETH, 0.5 ether, payable(bob), payable(relayer), 0, bytes32(0), nf, hex"01");
        vm.expectRevert(WrapVault.InvalidAmount.selector);
        v.release(ETH, 1 ether, payable(bob), payable(relayer), 2 ether, bytes32(0), nf, hex"01");
        vm.expectRevert(WrapVault.UnsupportedToken.selector);
        v.release(address(0xDEAD), 1 ether, payable(bob), payable(relayer), 0, bytes32(0), nf, hex"01");
    }

    function test_the_digest_binds_this_vault_and_chain() public {
        bytes32 d = v.releaseDigest(ETH, 1 ether, bob, relayer, 0, bytes32(0), bytes32(0));
        WrapVault other = new WrapVault(PAIRING, ver, WINDOW, admin, freezer);
        assertTrue(other.releaseDigest(ETH, 1 ether, bob, relayer, 0, bytes32(0), bytes32(0)) != d);
        vm.chainId(11155111);
        assertTrue(v.releaseDigest(ETH, 1 ether, bob, relayer, 0, bytes32(0), bytes32(0)) != d);
    }

    // ---- registry (F.10) ----

    function test_only_admin_registers_and_delisting_keeps_releases_open() public {
        vm.prank(alice);
        vm.expectRevert(WrapVault.NotAdmin.selector);
        v.registerToken(address(0xBEEF), 18, _amounts(1, 2), 1, 1);

        vm.prank(alice);
        v.deposit(address(usdc), 100e6, bytes32(0));
        vm.prank(admin);
        v.delistToken(address(usdc));
        vm.prank(alice);
        vm.expectRevert(WrapVault.UnsupportedToken.selector);
        v.deposit(address(usdc), 100e6, bytes32(0));

        bytes32 nf = bytes32(uint256(77));
        ver.approve(v.releaseDigest(address(usdc), 100e6, bob, relayer, 0, bytes32(0), nf));
        v.release(address(usdc), 100e6, payable(bob), payable(relayer), 0, bytes32(0), nf, hex"01");
        assertEq(usdc.balanceOf(bob), 100e6);
    }

    function test_registration_rejects_amounts_lez_cannot_represent() public {
        uint256[] memory a = new uint256[](1);
        a[0] = uint256(type(uint128).max) + 1;
        vm.prank(admin);
        vm.expectRevert(WrapVault.InvalidAmount.selector);
        v.registerToken(address(0xBEEF), 18, a, 1, 1);
        vm.prank(admin);
        vm.expectRevert(WrapVault.InvalidAmount.selector);
        v.registerToken(address(0xBEEF), 18, new uint256[](0), 1, 1);
    }

    function test_constructor_refuses_a_missing_verifier() public {
        vm.expectRevert(WrapVault.BadConfig.selector);
        new WrapVault(PAIRING, MockVerifier(address(0)), WINDOW, admin, freezer);
    }

    // ---- constants shared with core/src/lib.rs (the Rust side must hash identically) ----

    function test_empty_root_constant_is_shared_with_rust() public {
        assertEq(new WrapVault(PAIRING, ver, WINDOW, admin, freezer).root(), 0x31206fa80a50bb6abe29085058f16212212a60eec8f049fecb92d8c8e0a84bc0);
    }

    function test_leaf_constant_is_shared_with_rust() public view {
        bytes32 l = sha256(
            abi.encode(v.LEAF_TAG(), uint256(31337), address(1), bytes32(0x1111111111111111111111111111111111111111111111111111111111111111), address(2), uint256(100e6), bytes32(0x2222222222222222222222222222222222222222222222222222222222222222))
        );
        assertEq(l, 0xbbd9c2571ff327c42320ed455c820d85534eb3f417852e6c8179bc9379221c43);
    }

    function test_release_digest_constant_is_shared_with_rust() public view {
        bytes32 d = sha256(
            abi.encode(v.RELEASE_TAG(), uint256(31337), address(1), bytes32(0x1111111111111111111111111111111111111111111111111111111111111111), address(2), uint256(1_000_000), address(3), address(4), uint256(5), bytes32(0x3333333333333333333333333333333333333333333333333333333333333333), bytes32(0x4444444444444444444444444444444444444444444444444444444444444444))
        );
        assertEq(d, 0xf6c5b41d69d736ebe3c139a70d2172fdf781b356f13abe0308c7694f7dd2feec);
        // And the vault's own function is that formula, for its chain, address and pairing.
        assertEq(
            v.releaseDigest(address(2), 1_000_000, address(3), address(4), 5, bytes32(uint256(0x33)), bytes32(uint256(0x44))),
            sha256(abi.encode(v.RELEASE_TAG(), block.chainid, address(v), PAIRING, address(2), uint256(1_000_000), address(3), address(4), uint256(5), bytes32(uint256(0x33)), bytes32(uint256(0x44))))
        );
    }
}

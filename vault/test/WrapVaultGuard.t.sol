// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {Test, stdStorage, StdStorage} from "forge-std/Test.sol";
import {WrapVault} from "../src/WrapVault.sol";
import {MockERC20, MockVerifier} from "./Mocks.sol";

/// The rolling-window rate limits, the global caps, the timelock and the freezer (guardian),
/// each tested against the admin or the freezer acting adversarially.
contract WrapVaultGuardTest is Test {
    using stdStorage for StdStorage;

    WrapVault v;
    MockVerifier ver;
    MockERC20 usdc;
    MockERC20 dai;
    address constant ETH = address(0);
    address admin = address(0xA11CE);
    address freezer = address(0xF2EE);
    address alice = address(0xA1);
    address bob = address(0xB0B);
    address relayer = address(0x7E1A);
    bytes32 constant PAIRING = keccak256("test pairing");
    uint64 constant WINDOW = 1 days;
    uint256 nonce;

    function setUp() public {
        vm.warp(1_700_000_000);
        ver = new MockVerifier();
        v = new WrapVault(PAIRING, ver, WINDOW, admin, freezer);
        usdc = new MockERC20("USDC", 6);
        dai = new MockERC20("DAI", 18);
        vm.startPrank(admin);
        v.registerToken(ETH, 18, _amounts(0.1 ether, 1 ether), 100 ether, 100 ether);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 2000e6, 2000e6);
        v.registerToken(address(dai), 18, _amounts(100e18, 1000e18), 10_000e18, 10_000e18);
        vm.stopPrank();
        vm.deal(alice, 1000 ether);
        vm.deal(address(v), 1000 ether);
        usdc.mint(alice, 1e15);
        usdc.mint(address(v), 1e15);
        dai.mint(alice, 1e27);
        dai.mint(address(v), 1e27);
        // The funds above stand for earlier deposits: release only pays what deposits brought in.
        stdstore.target(address(v)).sig("escrowed(address)").with_key(ETH).checked_write(uint256(1000 ether));
        stdstore.target(address(v)).sig("escrowed(address)").with_key(address(usdc)).checked_write(uint256(1e15));
        stdstore.target(address(v)).sig("escrowed(address)").with_key(address(dai)).checked_write(uint256(1e27));
        vm.startPrank(alice);
        usdc.approve(address(v), type(uint256).max);
        dai.approve(address(v), type(uint256).max);
        vm.stopPrank();
    }

    function _amounts(uint256 a, uint256 b) internal pure returns (uint256[] memory r) {
        r = new uint256[](2);
        r[0] = a;
        r[1] = b;
    }

    function _deposit(address token, uint256 amount) internal {
        vm.prank(alice);
        v.deposit{value: token == ETH ? amount : 0}(token, amount, bytes32(++nonce));
    }

    function _release(address token, uint256 amount) internal {
        bytes32 nf = bytes32(++nonce);
        ver.approve(v.releaseDigest(token, amount, bob, relayer, 0, bytes32(0), nf));
        v.release(token, amount, payable(bob), payable(relayer), 0, bytes32(0), nf, hex"01");
    }

    function _releaseCall(address token, uint256 amount) internal returns (bool ok, bytes memory ret) {
        bytes32 nf = bytes32(++nonce);
        ver.approve(v.releaseDigest(token, amount, bob, relayer, 0, bytes32(0), nf));
        (ok, ret) = address(v).call(
            abi.encodeCall(WrapVault.release, (token, amount, payable(bob), payable(relayer), 0, bytes32(0), nf, hex"01"))
        );
    }

    /// Schedule `data` as the admin, wait the delay, and run it as the admin.
    function _timelocked(bytes memory data) internal {
        vm.prank(admin);
        v.schedule(data);
        vm.warp(block.timestamp + v.delay());
        _exec(data);
    }

    function _exec(bytes memory data) internal {
        vm.prank(admin);
        (bool ok, bytes memory ret) = address(v).call(data);
        if (!ok) {
            assembly {
                revert(add(ret, 32), mload(ret))
            }
        }
    }

    function _depositAvailable(address token) internal view returns (uint256 units) {
        (units,) = v.available(token, false);
    }

    // ---- rolling window (F.11, Security #4) ----

    /// The fixed window let a full cap through just before its reset and another just after: two
    /// caps in one second. The bucket refills at cap / WINDOW per second, so one second after a
    /// full cap only that second's refill is available.
    function test_the_boundary_burst_of_a_fixed_window_is_closed() public {
        _deposit(address(usdc), 100e6); // what used to open a fixed window
        vm.warp(block.timestamp + WINDOW - 1); // the last second of that window
        _deposit(address(usdc), 1000e6);
        _deposit(address(usdc), 1000e6);
        vm.warp(block.timestamp + 1); // the first second of the next fixed window
        vm.prank(alice);
        vm.expectRevert(WrapVault.CapExceeded.selector);
        v.deposit(address(usdc), 100e6, bytes32(0));
        assertEq(_depositAvailable(address(usdc)), uint256(2000e6) / WINDOW); // one second of refill
    }

    function test_the_bucket_refills_linearly_over_the_window() public {
        _deposit(address(usdc), 1000e6);
        _deposit(address(usdc), 1000e6);
        assertEq(_depositAvailable(address(usdc)), 0);
        vm.warp(block.timestamp + WINDOW / 4);
        assertEq(_depositAvailable(address(usdc)), 500e6);
        vm.prank(alice);
        vm.expectRevert(WrapVault.CapExceeded.selector);
        v.deposit(address(usdc), 1000e6, bytes32(0));
        vm.warp(block.timestamp + WINDOW / 4);
        _deposit(address(usdc), 1000e6);
        assertEq(_depositAvailable(address(usdc)), 0);
        vm.warp(block.timestamp + 10 * WINDOW); // a long pause refills to the cap, not beyond
        assertEq(_depositAvailable(address(usdc)), 2000e6);
    }

    function test_releases_are_rate_limited_on_the_same_rolling_window() public {
        _release(address(usdc), 1000e6);
        _release(address(usdc), 1000e6);
        (bool ok, bytes memory ret) = _releaseCall(address(usdc), 100e6);
        assertFalse(ok);
        assertEq(bytes4(ret), WrapVault.CapExceeded.selector);
        vm.warp(block.timestamp + WINDOW / 2);
        _release(address(usdc), 1000e6);
        (ok, ret) = _releaseCall(address(usdc), 100e6);
        assertFalse(ok);
        // Deposits are limited separately: releases did not consume them.
        assertEq(_depositAvailable(address(usdc)), 2000e6);
    }

    /// Over any interval of length T, at most cap * (1 + T / WINDOW) is admitted, and nothing is
    /// refused that the bucket had room for.
    function testFuzz_rolling_bound_holds_over_every_interval(uint256 seed) public {
        uint256 cap = 2000e6;
        uint256 n = 40;
        uint256[] memory at = new uint256[](n);
        uint256[] memory amt = new uint256[](n);
        uint256 k;
        for (uint256 i; i < n; ++i) {
            seed = uint256(keccak256(abi.encode(seed, i)));
            vm.warp(block.timestamp + (seed % (WINDOW / 3)) * ((seed >> 64) % 4 == 0 ? 0 : 1));
            uint256 a = (seed >> 128) % 2 == 0 ? 100e6 : 1000e6;
            uint256 room = _depositAvailable(address(usdc));
            vm.prank(alice);
            try v.deposit(address(usdc), a, bytes32(i)) {
                assertLe(a, room);
                at[k] = block.timestamp;
                amt[k] = a;
                ++k;
            } catch (bytes memory err) {
                assertEq(bytes4(err), WrapVault.CapExceeded.selector);
                assertGt(a, room);
            }
        }
        for (uint256 i; i < k; ++i) {
            uint256 sum;
            for (uint256 j = i; j < k; ++j) {
                sum += amt[j];
                assertLe(sum, cap + (at[j] - at[i]) * cap / WINDOW + 1);
            }
        }
    }

    /// Lowering a cap below what the bucket holds blocks transfers until it drains, at the old
    /// cap's rate up to the change.
    function test_lowering_a_cap_takes_effect_at_once_and_drains_at_the_old_rate_until_then() public {
        _deposit(address(usdc), 1000e6);
        _deposit(address(usdc), 1000e6);
        vm.warp(block.timestamp + WINDOW / 2); // level 1000e6 at the old rate
        vm.prank(admin);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 500e6, 2000e6); // immediate: tightens
        assertEq(_depositAvailable(address(usdc)), 0);
        vm.prank(alice);
        vm.expectRevert(WrapVault.CapExceeded.selector);
        v.deposit(address(usdc), 100e6, bytes32(0));
        vm.warp(block.timestamp + WINDOW); // 500e6 more drains at the new rate: level 500e6, at cap
        assertEq(_depositAvailable(address(usdc)), 0);
        vm.warp(block.timestamp + WINDOW / 5); // 100e6 more
        _deposit(address(usdc), 100e6);
    }

    function test_a_raised_cap_is_settled_at_the_old_rate_first() public {
        WrapVault slow = new WrapVault(PAIRING, ver, 30 days, admin, freezer);
        vm.prank(admin);
        slow.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 3000e6, 3000e6);
        vm.prank(alice);
        usdc.approve(address(slow), type(uint256).max);
        for (uint256 i; i < 3; ++i) {
            vm.prank(alice);
            slow.deposit(address(usdc), 1000e6, bytes32(i));
        }
        bytes memory raise = abi.encodeCall(WrapVault.registerToken, (address(usdc), 6, _amounts(100e6, 1000e6), 30_000e6, 3000e6));
        vm.prank(admin);
        slow.schedule(raise);
        vm.warp(block.timestamp + slow.delay());
        vm.prank(admin);
        (bool ok,) = address(slow).call(raise);
        assertTrue(ok);
        // Two days at 3000e6 per 30 days drained 200e6: level 2800e6 under the new cap of 30,000e6.
        (uint256 room,) = slow.available(address(usdc), false);
        assertEq(room, 30_000e6 - 2800e6);
    }

    // ---- global caps per direction ----

    function _priceAndCap(uint96 depositCap, uint96 releaseCap) internal {
        _timelocked(abi.encodeCall(WrapVault.setTokenValue, (ETH, 3000e18)));
        _timelocked(abi.encodeCall(WrapVault.setTokenValue, (address(usdc), 1e18)));
        _timelocked(abi.encodeCall(WrapVault.setTokenValue, (address(dai), 1e18)));
        vm.prank(admin);
        v.setGlobalCaps(depositCap, releaseCap); // from UNCAPPED: tightens, immediate
    }

    /// Each token is under its own cap, and together they exceed the global one.
    function test_the_global_deposit_cap_spans_tokens() public {
        _priceAndCap(5000e18, type(uint96).max);
        _deposit(address(usdc), 1000e6); // 1000
        _deposit(address(dai), 1000e18); // 2000
        _deposit(ETH, 1 ether); // 5000
        (, uint256 g) = v.available(address(dai), false);
        assertEq(g, 0);
        (uint256 own,) = v.available(address(dai), false);
        assertEq(own, 9000e18);
        vm.prank(alice);
        vm.expectRevert(WrapVault.CapExceeded.selector);
        v.deposit(address(dai), 100e18, bytes32(0));
        vm.prank(alice);
        vm.expectRevert(WrapVault.CapExceeded.selector);
        v.deposit{value: 0.1 ether}(ETH, 0.1 ether, bytes32(0));
        vm.warp(block.timestamp + WINDOW / 50); // 100 of value
        _deposit(address(dai), 100e18);
        // Releases are not under the global deposit cap.
        _release(ETH, 1 ether);
    }

    function test_the_global_release_cap_spans_tokens() public {
        _priceAndCap(type(uint96).max, 4000e18);
        _release(ETH, 1 ether);
        _release(address(dai), 1000e18);
        (bool ok, bytes memory ret) = _releaseCall(address(usdc), 100e6);
        assertFalse(ok);
        assertEq(bytes4(ret), WrapVault.CapExceeded.selector);
        vm.warp(block.timestamp + WINDOW / 40); // 100 of value
        _release(address(usdc), 100e6);
    }

    /// While a global cap is on, a token without a value cannot move: leaving a token unpriced is
    /// no way around the cap.
    function test_an_unpriced_token_is_refused_while_a_global_cap_is_on() public {
        MockERC20 x = new MockERC20("X", 18);
        x.mint(alice, 1e24);
        vm.prank(alice);
        x.approve(address(v), type(uint256).max);
        vm.startPrank(admin);
        v.registerToken(address(x), 18, _amounts(1e18, 2e18), 1e21, 1e21);
        v.setGlobalCaps(1e27, 1e27);
        vm.stopPrank();
        vm.prank(alice);
        vm.expectRevert(WrapVault.Unpriced.selector);
        v.deposit(address(x), 1e18, bytes32(0));
        (bool ok, bytes memory ret) = _releaseCall(address(x), 1e18);
        assertFalse(ok);
        assertEq(bytes4(ret), WrapVault.Unpriced.selector);
        // Pricing it is a loosening, so it waits for the delay.
        vm.prank(admin);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.setTokenValue(address(x), 1e18);
        _timelocked(abi.encodeCall(WrapVault.setTokenValue, (address(x), 1e18)));
        _deposit(address(x), 1e18);
    }

    function test_value_changes_raise_at_once_and_lower_only_through_the_timelock() public {
        _timelocked(abi.encodeCall(WrapVault.setTokenValue, (ETH, 3000e18)));
        vm.startPrank(admin);
        v.setTokenValue(ETH, 4000e18); // more value per unit: tighter
        v.setTokenValue(ETH, 0); // unpriced: refused under a global cap, the tightest
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.setTokenValue(ETH, 1);
        vm.expectRevert(WrapVault.UnsupportedToken.selector);
        v.setTokenValue(address(0xDEAD), 1e18);
        vm.stopPrank();
        vm.prank(alice);
        vm.expectRevert(WrapVault.NotAdmin.selector);
        v.setTokenValue(ETH, 1e36);
    }

    function test_global_caps_lower_at_once_and_rise_only_through_the_timelock() public {
        vm.startPrank(admin);
        v.setGlobalCaps(1000e18, 1000e18);
        v.setGlobalCaps(500e18, 1000e18);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.setGlobalCaps(501e18, 1000e18);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.setGlobalCaps(type(uint96).max, type(uint96).max); // turning a cap off is a raise
        vm.stopPrank();
        _timelocked(abi.encodeCall(WrapVault.setGlobalCaps, (2000e18, 1000e18)));
        assertEq(v.globalDepositCap(), 2000e18);
    }

    // ---- the timelock ----

    function test_raising_a_cap_needs_the_exact_call_scheduled_a_delay_earlier() public {
        bytes memory raise = abi.encodeCall(WrapVault.registerToken, (address(usdc), 6, _amounts(100e6, 1000e6), 4000e6, 2000e6));
        vm.prank(admin);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 4000e6, 2000e6);

        vm.prank(alice);
        vm.expectRevert(WrapVault.NotAdmin.selector);
        v.schedule(raise);
        vm.prank(freezer);
        vm.expectRevert(WrapVault.NotAdmin.selector);
        v.schedule(raise);

        vm.prank(admin);
        (bytes32 id, uint256 at) = v.schedule(raise);
        assertEq(at, block.timestamp + 2 days);
        vm.prank(admin);
        vm.expectRevert(WrapVault.AlreadyScheduled.selector);
        v.schedule(raise);

        vm.warp(at - 1);
        vm.prank(admin);
        vm.expectRevert(WrapVault.TooEarly.selector);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 4000e6, 2000e6);
        vm.warp(at);
        // Another value than the one scheduled is not authorized by it.
        vm.prank(admin);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 40_000e6, 2000e6);
        // Nor can anyone but the admin run it.
        vm.prank(alice);
        vm.expectRevert(WrapVault.NotAdmin.selector);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 4000e6, 2000e6);

        _exec(raise);
        (,,,,, uint128 depositCap,,) = v.tokens(address(usdc));
        assertEq(depositCap, 4000e6);
        assertEq(v.readyAt(id), 0);
        // Consumed: it runs once.
        vm.prank(admin);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 2000e6, 2000e6);
        vm.prank(admin);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 4000e6, 2000e6);
    }

    /// An operation cannot be stockpiled: past GRACE after it matured it is void.
    function test_a_matured_operation_expires_after_the_grace_period() public {
        bytes memory raise = abi.encodeCall(WrapVault.registerToken, (address(usdc), 6, _amounts(100e6, 1000e6), 4000e6, 2000e6));
        vm.prank(admin);
        (, uint256 at) = v.schedule(raise);
        vm.warp(at + v.GRACE() + 1);
        vm.prank(admin);
        vm.expectRevert(WrapVault.Expired.selector);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 4000e6, 2000e6);
    }

    function test_a_cancelled_operation_cannot_run() public {
        bytes memory raise = abi.encodeCall(WrapVault.setGlobalCaps, (type(uint96).max, type(uint96).max));
        vm.prank(admin);
        v.setGlobalCaps(1e18, 1e18);
        vm.prank(admin);
        (bytes32 id, uint256 at) = v.schedule(raise);
        vm.prank(freezer);
        vm.expectRevert(WrapVault.NotAdmin.selector);
        v.cancel(id);
        vm.prank(admin);
        v.cancel(id);
        vm.warp(at);
        vm.prank(admin);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.setGlobalCaps(type(uint96).max, type(uint96).max);
    }

    function test_relisting_adding_a_denomination_or_changing_decimals_is_not_immediate() public {
        vm.startPrank(admin);
        v.delistToken(address(usdc)); // immediate
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 2000e6, 2000e6); // relist
        vm.stopPrank();
        _timelocked(abi.encodeCall(WrapVault.registerToken, (address(usdc), 6, _amounts(100e6, 1000e6), 2000e6, 2000e6)));
        _deposit(address(usdc), 100e6);

        vm.startPrank(admin);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.registerToken(address(usdc), 6, _amounts(100e6, 5000e6), 2000e6, 2000e6); // a new denomination
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 2000e6, 2001e6); // a higher release cap
        vm.expectRevert(WrapVault.BadConfig.selector);
        v.registerToken(address(usdc), 18, _amounts(100e6, 1000e6), 2000e6, 2000e6);
        vm.expectRevert(WrapVault.BadConfig.selector);
        v.registerToken(address(0xBEEF), 37, _amounts(1, 2), 1, 1);
        vm.stopPrank();
    }

    function test_the_delay_lengthens_at_once_and_shortens_through_itself_within_bounds() public {
        vm.startPrank(admin);
        v.setDelay(7 days);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.setDelay(1 days);
        vm.expectRevert(WrapVault.BadConfig.selector);
        v.setDelay(1 days - 1);
        vm.expectRevert(WrapVault.BadConfig.selector);
        v.setDelay(30 days + 1);
        (, uint256 at) = v.schedule(abi.encodeCall(WrapVault.setDelay, (1 days)));
        vm.stopPrank();
        assertEq(at, block.timestamp + 7 days); // the shortening waits the delay it shortens
        vm.warp(at);
        _exec(abi.encodeCall(WrapVault.setDelay, (1 days)));
        assertEq(v.delay(), 1 days);
    }

    function test_the_constructor_enforces_the_window_bounds_and_both_roles() public {
        vm.expectRevert(WrapVault.BadConfig.selector);
        new WrapVault(PAIRING, ver, 1 hours - 1, admin, freezer);
        vm.expectRevert(WrapVault.BadConfig.selector);
        new WrapVault(PAIRING, ver, 30 days + 1, admin, freezer);
        vm.expectRevert(WrapVault.BadConfig.selector);
        new WrapVault(PAIRING, ver, WINDOW, address(0), freezer);
        vm.expectRevert(WrapVault.BadConfig.selector);
        new WrapVault(PAIRING, ver, WINDOW, admin, address(0));
        WrapVault w = new WrapVault(PAIRING, ver, 1 hours, admin, freezer);
        assertEq(w.delay(), 2 days);
        assertEq(w.globalDepositCap(), type(uint96).max);
    }

    // ---- the freezer (guardian) ----

    function test_the_freezer_pauses_at_once_and_cannot_unpause() public {
        vm.prank(freezer);
        v.freezeAll(true, true);
        vm.prank(alice);
        vm.expectRevert(WrapVault.IsFrozen.selector);
        v.deposit(address(usdc), 100e6, bytes32(0));
        (bool ok, bytes memory ret) = _releaseCall(address(usdc), 100e6);
        assertFalse(ok);
        assertEq(bytes4(ret), WrapVault.IsFrozen.selector);

        // `false` leaves a direction as it is.
        uint64 count = v.pauseCount();
        vm.startPrank(freezer);
        v.freezeAll(false, false);
        assertTrue(v.depositsFrozen() && v.releasesFrozen());
        vm.expectRevert(WrapVault.NotAdmin.selector);
        v.unfreezeAll(true, true, count);
        vm.expectRevert(WrapVault.NotAdmin.selector);
        v.schedule(abi.encodeCall(WrapVault.unfreezeAll, (true, true, count)));
        vm.expectRevert(WrapVault.NotAdmin.selector);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 1, 1);
        vm.expectRevert(WrapVault.UnsupportedToken.selector);
        v.freeze(address(0xDEAD), true, true);
        v.freeze(address(usdc), true, true);
        v.freeze(address(usdc), false, false);
        vm.stopPrank();
        (,, bool df, bool rf,,,,) = v.tokens(address(usdc));
        assertTrue(df && rf);
    }

    function test_a_token_unpause_cannot_lift_a_later_token_pause() public {
        vm.prank(freezer);
        v.freeze(address(usdc), true, false);
        uint64 count = v.pauseCount();
        bytes memory lift = abi.encodeCall(WrapVault.unfreeze, (address(usdc), true, true, count));
        vm.prank(admin);
        (, uint256 at) = v.schedule(lift);
        vm.prank(freezer);
        v.freeze(address(usdc), true, true); // releases too, now
        vm.warp(at);
        vm.prank(admin);
        vm.expectRevert(WrapVault.StalePause.selector);
        v.unfreeze(address(usdc), true, true, count);
    }

    /// The admin is not a freezer, and cannot lift the freezer's pause at once.
    function test_the_admin_cannot_bypass_a_pause_at_once() public {
        vm.prank(admin);
        vm.expectRevert(WrapVault.NotFreezer.selector);
        v.freezeAll(true, true);

        vm.prank(freezer);
        v.freeze(address(usdc), true, true);
        uint64 count = v.pauseCount();
        vm.prank(admin);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.unfreeze(address(usdc), true, true, count);
        vm.prank(admin);
        (, uint256 at) = v.schedule(abi.encodeCall(WrapVault.unfreeze, (address(usdc), true, true, count)));
        vm.warp(at - 1);
        vm.prank(admin);
        vm.expectRevert(WrapVault.TooEarly.selector);
        v.unfreeze(address(usdc), true, true, count);
        // Nor by delisting and relisting, or by registering it again.
        vm.prank(admin);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 2000e6, 2000e6);
        (,, bool df, bool rf,,,,) = v.tokens(address(usdc));
        assertTrue(df && rf);

        vm.warp(at);
        _exec(abi.encodeCall(WrapVault.unfreeze, (address(usdc), true, true, count)));
        _deposit(address(usdc), 100e6);
        _release(address(usdc), 100e6);
    }

    /// An unpause scheduled ahead of time, and kept ready, cannot lift a pause that came after it.
    function test_an_unpause_kept_ready_cannot_lift_a_later_pause() public {
        uint64 count = v.pauseCount();
        bytes memory lift = abi.encodeCall(WrapVault.unfreezeAll, (true, true, count));
        vm.prank(admin);
        (, uint256 at) = v.schedule(lift);
        vm.warp(at);
        vm.prank(freezer);
        v.freezeAll(true, true); // the incident
        vm.prank(admin);
        vm.expectRevert(WrapVault.StalePause.selector);
        v.unfreezeAll(true, true, count);
        vm.prank(admin);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.unfreezeAll(true, true, count + 1);
        assertTrue(v.depositsFrozen() && v.releasesFrozen());
    }

    /// Pausing what is already paused does not count, so a freezer cannot keep voiding unpauses
    /// without end: there are only so many things to pause.
    function test_repausing_what_is_paused_does_not_void_a_scheduled_unpause() public {
        vm.prank(freezer);
        v.freezeAll(true, false);
        uint64 count = v.pauseCount();
        bytes memory lift = abi.encodeCall(WrapVault.unfreezeAll, (true, false, count));
        vm.prank(admin);
        (, uint256 at) = v.schedule(lift);
        vm.startPrank(freezer);
        v.freezeAll(true, false);
        v.freezeAll(false, false);
        vm.stopPrank();
        assertEq(v.pauseCount(), count);
        vm.warp(at);
        _exec(lift);
        assertFalse(v.depositsFrozen());
    }

    // ---- role rotation ----

    function test_the_freezer_rotates_only_through_the_timelock_and_never_to_the_admin() public {
        address g2 = address(0x6A2D);
        vm.prank(freezer);
        vm.expectRevert(WrapVault.NotAdmin.selector);
        v.setFreezer(g2);
        vm.prank(admin);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.setFreezer(g2);
        vm.prank(admin);
        vm.expectRevert(WrapVault.BadConfig.selector);
        v.setFreezer(admin);
        vm.prank(admin);
        vm.expectRevert(WrapVault.BadConfig.selector);
        v.setFreezer(address(0));

        // The current freezer can still pause while its replacement waits out the delay.
        vm.prank(admin);
        (, uint256 at) = v.schedule(abi.encodeCall(WrapVault.setFreezer, (g2)));
        vm.prank(freezer);
        v.freeze(ETH, true, false);
        vm.warp(at);
        _exec(abi.encodeCall(WrapVault.setFreezer, (g2)));
        assertEq(v.freezer(), g2);
        vm.prank(freezer);
        vm.expectRevert(WrapVault.NotFreezer.selector);
        v.freeze(ETH, true, true);
        vm.prank(g2);
        v.freeze(ETH, true, true);
    }

    /// A new admin accepts; every operation the old admin had pending is void from then on.
    function test_admin_rotation_is_two_step_timelocked_and_voids_the_old_admins_operations() public {
        address a2 = address(0xA22);
        bytes memory raise = abi.encodeCall(WrapVault.setGlobalCaps, (type(uint96).max, type(uint96).max));
        vm.prank(admin);
        v.setGlobalCaps(1e18, 1e18);
        vm.prank(admin);
        v.schedule(raise); // pending when the admin is replaced

        vm.prank(admin);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.transferAdmin(a2);
        vm.prank(admin);
        vm.expectRevert(WrapVault.BadConfig.selector);
        v.transferAdmin(freezer);
        _timelocked(abi.encodeCall(WrapVault.transferAdmin, (a2)));
        assertEq(v.pendingAdmin(), a2);
        assertEq(v.admin(), admin);

        vm.prank(alice);
        vm.expectRevert(WrapVault.NotAdmin.selector);
        v.acceptAdmin();
        vm.prank(a2);
        v.acceptAdmin();
        assertEq(v.admin(), a2);
        assertEq(v.pendingAdmin(), address(0));
        assertEq(v.adminEpoch(), 1);

        vm.prank(admin);
        vm.expectRevert(WrapVault.NotAdmin.selector);
        v.setGlobalCaps(0, 0);
        vm.prank(a2);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.setGlobalCaps(type(uint96).max, type(uint96).max);
    }

    // ---- gas: the rate limits' cost on deposit and release, each call on cold storage ----

    function _coldDeposit() internal returns (uint256 used) {
        vm.cool(address(v));
        vm.cool(address(usdc));
        uint256 g = gasleft();
        _deposit(address(usdc), 100e6);
        used = g - gasleft();
    }

    function _coldRelease() internal returns (uint256 used) {
        bytes32 nf = bytes32(++nonce);
        ver.approve(v.releaseDigest(address(usdc), 100e6, bob, relayer, 0, bytes32(0), nf));
        vm.cool(address(v));
        vm.cool(address(usdc));
        vm.cool(address(ver));
        uint256 g = gasleft();
        v.release(address(usdc), 100e6, payable(bob), payable(relayer), 0, bytes32(0), nf, hex"01");
        used = g - gasleft();
    }

    function test_gas_of_the_rate_limits() public {
        _deposit(address(usdc), 100e6); // a second deposit and release: buckets and tree non-empty
        _release(address(usdc), 100e6);
        uint256 depositTokenOnly = _coldDeposit();
        uint256 releaseTokenOnly = _coldRelease();
        _priceAndCap(1e27, 1e27);
        _deposit(address(usdc), 100e6);
        _release(address(usdc), 100e6);
        uint256 depositGlobal = _coldDeposit();
        uint256 releaseGlobal = _coldRelease();
        emit log_named_uint("deposit, token limit only", depositTokenOnly);
        emit log_named_uint("deposit, token and global limits", depositGlobal);
        emit log_named_uint("release, token limit only", releaseTokenOnly);
        emit log_named_uint("release, token and global limits", releaseGlobal);
        assertLt(depositGlobal - depositTokenOnly, 10_000);
        assertLt(releaseGlobal - releaseTokenOnly, 10_000);
    }

    /// A proposed admin who has since become the freezer cannot take the admin role too.
    function test_the_freezer_cannot_become_the_admin() public {
        address a2 = address(0xA22);
        _timelocked(abi.encodeCall(WrapVault.transferAdmin, (a2)));
        _timelocked(abi.encodeCall(WrapVault.setFreezer, (a2)));
        vm.prank(a2);
        vm.expectRevert(WrapVault.BadConfig.selector);
        v.acceptAdmin();
    }
}

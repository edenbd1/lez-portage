// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {WrapVault} from "../src/WrapVault.sol";
import {MockERC20, MockVerifier} from "./Mocks.sol";

/// A verifier with a flaw: it accepts every proof. What the vault still guarantees then is what
/// docs/FAILSAFE.md relies on.
contract FlawedVerifier {
    function verify(bytes calldata, bytes32) external pure returns (bool) {
        return true;
    }
}

/// Supply soundness (Security #6) and the verification-flaw failsafe (Security #9).
contract WrapVaultFailsafeTest is Test {
    WrapVault v;
    MockVerifier ver;
    MockERC20 usdc;
    address constant ETH = address(0);
    address admin = address(0xA11CE);
    address freezer = address(0xF2EE);
    address alice = address(0xA1);
    address bob = address(0xB0B);
    address relayer = address(0x7E1A);
    address thief = address(0xBAD);
    bytes32 constant PAIRING = keccak256("test pairing");
    uint64 constant WINDOW = 1 days;
    uint256 nonce;

    function setUp() public {
        vm.warp(1_700_000_000);
        ver = new MockVerifier();
        v = _vault(address(ver));
        usdc = new MockERC20("USDC", 6);
        vm.prank(admin);
        v.registerToken(address(usdc), 6, _amounts(100e6, 1000e6), 1e12, 1e12);
        vm.deal(alice, 100 ether);
        usdc.mint(alice, 1e12);
        vm.prank(alice);
        usdc.approve(address(v), type(uint256).max);
    }

    function _vault(address verifier) internal returns (WrapVault w) {
        w = new WrapVault(PAIRING, MockVerifier(verifier), WINDOW, admin, freezer);
        vm.prank(admin);
        w.registerToken(ETH, 18, _amounts(0.1 ether, 1 ether), 100 ether, 100 ether);
    }

    function _amounts(uint256 a, uint256 b) internal pure returns (uint256[] memory r) {
        r = new uint256[](2);
        r[0] = a;
        r[1] = b;
    }

    function _deposit(WrapVault w, address token, uint256 amount) internal {
        vm.prank(alice);
        w.deposit{value: token == ETH ? amount : 0}(token, amount, bytes32(++nonce));
    }

    function _release(WrapVault w, address token, uint256 amount, address to) internal returns (bytes32 nf) {
        nf = bytes32(++nonce);
        if (address(w.VERIFIER()) == address(ver)) ver.approve(w.releaseDigest(token, amount, to, relayer, 0, bytes32(0), nf));
        w.release(token, amount, payable(to), payable(relayer), 0, bytes32(0), nf, hex"01");
    }

    // ---- Security #6: releases are bounded by what deposits of the same token brought in ----

    function test_escrow_follows_deposits_and_releases() public {
        _deposit(v, ETH, 1 ether);
        _deposit(v, ETH, 0.1 ether);
        assertEq(v.escrowed(ETH), 1.1 ether);
        _release(v, ETH, 1 ether, bob);
        assertEq(v.escrowed(ETH), 0.1 ether);
        assertEq(bob.balance, 1 ether);
    }

    function test_a_flawed_verifier_cannot_release_more_than_the_tokens_deposits() public {
        WrapVault w = _vault(address(new FlawedVerifier()));
        _deposit(w, ETH, 1 ether);
        vm.deal(address(w), 50 ether); // funds that came in outside `deposit` are never paid out
        _release(w, ETH, 1 ether, thief);
        vm.expectRevert(WrapVault.Insolvent.selector);
        this.releaseExt(w, ETH, 0.1 ether, thief);
        assertEq(thief.balance, 1 ether);
    }

    function test_one_tokens_escrow_never_pays_another_token() public {
        _deposit(v, address(usdc), 1000e6);
        vm.deal(address(v), 10 ether);
        bytes32 nf = bytes32(++nonce);
        ver.approve(v.releaseDigest(ETH, 1 ether, bob, relayer, 0, bytes32(0), nf));
        vm.expectRevert(WrapVault.Insolvent.selector);
        v.release(ETH, 1 ether, payable(bob), payable(relayer), 0, bytes32(0), nf, hex"01");
        _release(v, address(usdc), 1000e6, bob);
        assertEq(usdc.balanceOf(bob), 1000e6);
    }

    function releaseExt(WrapVault w, address token, uint256 amount, address to) external {
        _release(w, token, amount, to);
    }

    // ---- Security #9: the release delay, the window in which a pause stops a payout ----

    function test_with_a_delay_a_release_is_booked_and_paid_after_it() public {
        vm.prank(admin);
        v.setReleaseDelay(6 hours); // lengthening is immediate
        _deposit(v, ETH, 1 ether);
        bytes32 nf = _release(v, ETH, 1 ether, bob);
        assertEq(bob.balance, 0);
        assertTrue(v.nullifierSpent(nf));
        assertEq(v.escrowed(ETH), 0);
        vm.expectRevert(WrapVault.TooEarly.selector);
        v.finalize(nf);
        vm.warp(block.timestamp + 6 hours);
        vm.prank(thief); // anyone may finalize; the payout still goes where the proof said
        v.finalize(nf);
        assertEq(bob.balance, 1 ether);
        assertEq(thief.balance, 0);
        vm.expectRevert(WrapVault.NotPending.selector);
        v.finalize(nf);
    }

    function test_a_pause_inside_the_delay_stops_the_payout() public {
        vm.prank(admin);
        v.setReleaseDelay(6 hours);
        _deposit(v, ETH, 1 ether);
        bytes32 nf = _release(v, ETH, 1 ether, thief);
        vm.prank(freezer); // the flaw is found within the delay
        v.freeze(ETH, false, true);
        vm.warp(block.timestamp + 7 days);
        vm.expectRevert(WrapVault.IsFrozen.selector);
        v.finalize(nf);
        vm.prank(freezer);
        v.freezeAll(false, true);
        vm.expectRevert(WrapVault.IsFrozen.selector);
        v.finalize(nf);
        assertEq(thief.balance, 0);
        assertEq(address(v).balance, 1 ether);
    }

    function test_a_booked_release_cannot_be_replayed() public {
        vm.prank(admin);
        v.setReleaseDelay(1 hours);
        _deposit(v, ETH, 1 ether);
        _deposit(v, ETH, 1 ether);
        bytes32 nf = _release(v, ETH, 1 ether, bob);
        vm.expectRevert(WrapVault.AlreadyReleased.selector);
        v.release(ETH, 1 ether, payable(bob), payable(relayer), 0, bytes32(0), nf, hex"01");
    }

    function test_shortening_the_delay_is_timelocked_and_bounded() public {
        vm.startPrank(admin);
        v.setReleaseDelay(2 days);
        vm.expectRevert(WrapVault.NotScheduled.selector);
        v.setReleaseDelay(1 hours);
        vm.expectRevert(WrapVault.BadConfig.selector);
        v.setReleaseDelay(8 days);
        vm.stopPrank();
        vm.prank(freezer);
        vm.expectRevert(WrapVault.NotAdmin.selector);
        v.setReleaseDelay(3 days);
        bytes memory d = abi.encodeCall(WrapVault.setReleaseDelay, (uint64(1 hours)));
        vm.prank(admin);
        v.schedule(d);
        vm.warp(block.timestamp + v.delay());
        vm.prank(admin);
        (bool ok,) = address(v).call(d);
        assertTrue(ok);
        assertEq(v.releaseDelay(), 1 hours);
    }

    function test_a_booked_release_keeps_its_time_when_the_delay_changes() public {
        vm.prank(admin);
        v.setReleaseDelay(1 hours);
        _deposit(v, ETH, 1 ether);
        bytes32 nf = _release(v, ETH, 1 ether, bob);
        vm.prank(admin);
        v.setReleaseDelay(7 days);
        vm.warp(block.timestamp + 1 hours);
        v.finalize(nf);
        assertEq(bob.balance, 1 ether);
    }
}

/// Drives random deposits, releases (through a verifier that accepts everything), bookings and
/// finalizations against one vault, and keeps the totals.
contract FailsafeHandler is Test {
    WrapVault public v;
    address constant ETH = address(0);
    uint256 public deposited;
    uint256 public released;
    uint256 public paid;
    bytes32[] public booked;
    uint256 n;

    constructor(WrapVault v_) {
        v = v_;
        vm.deal(address(this), 1e30);
    }

    function deposit(bool big) external {
        uint256 a = big ? 1 ether : 0.1 ether;
        try v.deposit{value: a}(ETH, a, bytes32(++n)) {
            deposited += a;
        } catch {}
    }

    function release(bool big, uint256 to) external {
        uint256 a = big ? 1 ether : 0.1 ether;
        address r = address(uint160(0x1000 + to % 5));
        bytes32 nf = bytes32(++n);
        uint256 before = r.balance;
        try v.release(ETH, a, payable(r), payable(r), 0, bytes32(0), nf, hex"") {
            released += a;
            if (v.releaseDelay() != 0) booked.push(nf);
            else paid += r.balance - before;
        } catch {}
    }

    function finalize(uint256 i) external {
        if (booked.length == 0) return;
        bytes32 nf = booked[i % booked.length];
        (, , address payable r,,,) = v.pending(nf);
        uint256 before = r.balance;
        try v.finalize(nf) {
            paid += r.balance - before;
        } catch {}
    }

    function warp(uint256 s) external {
        vm.warp(block.timestamp + s % 2 days);
    }

    function stray(uint256 a) external {
        vm.deal(address(v), address(v).balance + a % 10 ether); // funds outside `deposit`
    }

    receive() external payable {}
}

contract WrapVaultSupplyInvariantTest is Test {
    WrapVault v;
    FailsafeHandler h;

    function setUp() public {
        vm.warp(1_700_000_000);
        v = new WrapVault(keccak256("p"), MockVerifier(address(new FlawedVerifier())), 1 days, address(0xA11CE), address(0xF2EE));
        uint256[] memory a = new uint256[](2);
        a[0] = 0.1 ether;
        a[1] = 1 ether;
        vm.startPrank(address(0xA11CE));
        v.registerToken(address(0), 18, a, 1000 ether, 1000 ether);
        v.setReleaseDelay(3 hours);
        vm.stopPrank();
        h = new FailsafeHandler(v);
        targetContract(address(h));
    }

    /// Even with a verifier that accepts every proof, nothing beyond the deposits is ever released.
    function invariant_released_never_exceeds_deposited() public view {
        assertLe(h.released(), h.deposited());
        assertLe(h.paid(), h.released());
        assertEq(v.escrowed(address(0)), h.deposited() - h.released());
    }

    /// The vault always holds at least what is escrowed plus what is booked and not yet paid.
    function invariant_the_vault_backs_escrow_and_booked_payouts() public view {
        assertGe(address(v).balance, v.escrowed(address(0)) + (h.released() - h.paid()));
    }
}

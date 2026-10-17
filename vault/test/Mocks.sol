// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {IBurnVerifier} from "../src/IBurnVerifier.sol";

/// A standard ERC-20 (returns true), with configurable decimals: USDC (6), DAI and WETH (18).
contract MockERC20 {
    string public name;
    uint8 public decimals;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    constructor(string memory n, uint8 d) {
        name = n;
        decimals = d;
    }

    function mint(address to, uint256 a) external {
        balanceOf[to] += a;
    }

    function approve(address s, uint256 a) external returns (bool) {
        allowance[msg.sender][s] = a;
        return true;
    }

    function transfer(address to, uint256 a) external virtual returns (bool) {
        balanceOf[msg.sender] -= a;
        balanceOf[to] += a;
        return true;
    }

    function transferFrom(address f, address to, uint256 a) external virtual returns (bool) {
        allowance[f][msg.sender] -= a;
        balanceOf[f] -= a;
        balanceOf[to] += a;
        return true;
    }
}

/// USDT's shape: `transfer` and `transferFrom` return nothing (F.17).
contract MockUSDT {
    uint8 public constant decimals = 6;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 a) external {
        balanceOf[to] += a;
    }

    function approve(address s, uint256 a) external {
        allowance[msg.sender][s] = a;
    }

    function transfer(address to, uint256 a) external {
        require(balanceOf[msg.sender] >= a);
        balanceOf[msg.sender] -= a;
        balanceOf[to] += a;
    }

    function transferFrom(address f, address to, uint256 a) external {
        require(allowance[f][msg.sender] >= a && balanceOf[f] >= a);
        allowance[f][msg.sender] -= a;
        balanceOf[f] -= a;
        balanceOf[to] += a;
    }
}

/// Returns false instead of reverting: must be treated as a failed transfer.
contract MockFalseToken is MockERC20("false", 18) {
    function transferFrom(address, address, uint256) external pure override returns (bool) {
        return false;
    }
}

/// Takes a 1% fee on transfer: the vault would receive less than it mints against.
contract MockFeeToken is MockERC20("fee", 18) {
    function transferFrom(address f, address to, uint256 a) external override returns (bool) {
        allowance[f][msg.sender] -= a;
        balanceOf[f] -= a;
        balanceOf[to] += a - a / 100;
        return true;
    }
}

/// Accepts exactly the public-input digests a test has approved, once set.
contract MockVerifier is IBurnVerifier {
    mapping(bytes32 => bool) public ok;

    function approve(bytes32 d) external {
        ok[d] = true;
    }

    function verify(bytes calldata proof, bytes32 d) external view returns (bool) {
        return ok[d] && proof.length > 0;
    }
}

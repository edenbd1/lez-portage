// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {IBurnVerifier} from "./IBurnVerifier.sol";

/// @title WrapVault: the Ethereum half of a lock-and-mint bridge to LEZ (RFP-022 prototype).
///
/// Deposits escrow a registered ERC-20, or native ETH (token = address(0)), in one of the
/// token's permitted denominations, and append a leaf to an append-only SHA-256 Merkle tree:
///
///     leaf = sha256(abi.encode(LEAF_TAG, chainid, vault, PAIRING, token, amount, commitment))
///
/// `commitment` is the depositor's hiding commitment to their LEZ destination and a secret
/// derived from their seed; the destination itself never appears in a transaction argument, an
/// event or this contract's state (F.2). Every root the tree has ever had is recorded in
/// `knownRoot`, at storage slot 0, so a LEZ claim can prove "this root was a vault root" with a
/// storage proof against any later finalized header, and prove membership of its leaf privately:
/// the header it reads says nothing about which deposit it is, and a root, once known, is known
/// forever (R.4).
///
/// Releases are paid on a proof checked by `VERIFIER`, fixed at construction (Security #8), and
/// spend a nullifier once (R.3). Admin and freeze roles are minimal stand-ins for the RFP-001 and
/// RFP-002 libraries.
contract WrapVault {
    // ---- storage: `knownRoot` must stay the first variable (slot 0); the LEZ program proves it ----
    mapping(bytes32 => bool) public knownRoot;

    struct TokenConfig {
        bool listed; // accepts deposits; a delisted token still releases (no funds trapped)
        bool registered; // has ever been listed: releases stay possible
        bool depositsFrozen;
        bool releasesFrozen;
        uint8 decimals;
        uint128 depositCap; // per window, in token units; 0 = no deposits
        uint128 releaseCap;
        uint128 depositUsed;
        uint128 releaseUsed;
        uint64 depositWindowStart;
        uint64 releaseWindowStart;
    }

    mapping(address => TokenConfig) public tokens;
    mapping(address => mapping(uint256 => bool)) public permitted;
    mapping(bytes32 => bool) public nullifierSpent;

    uint256 public constant DEPTH = 24;
    bytes32 public constant LEAF_TAG = "rfp022/wrap/leaf/v1";
    bytes32 public constant RELEASE_TAG = "rfp022/wrap/release/v1";
    address public constant ETH = address(0);

    bytes32[DEPTH] public zeros;
    bytes32[DEPTH] public filledSubtrees;
    uint256 public nextIndex;
    bytes32 public root;

    /// sha256("rfp022/pair/v1" ‖ LEZ chain ‖ zone ‖ program ‖ deployment): the one LEZ deployment
    /// whose claims and burns this vault honours (F.15).
    bytes32 public immutable PAIRING;
    IBurnVerifier public immutable VERIFIER;
    uint64 public immutable WINDOW; // cap window, seconds

    address public admin;
    address public freezer;
    bool public depositsFrozen;
    bool public releasesFrozen;
    uint256 private locked = 1;

    event Deposit(uint256 indexed index, bytes32 leaf, bytes32 commitment, address indexed token, uint256 amount, bytes32 root);
    event Release(bytes32 indexed nullifier, address indexed token, uint256 amount, address recipient, address relayer, uint256 fee);
    event TokenRegistered(address indexed token, uint8 decimals, uint256[] amounts, uint128 depositCap, uint128 releaseCap);
    event TokenDelisted(address indexed token);
    event Frozen(address indexed token, bool deposits, bool releases);

    error NotAdmin();
    error NotFreezer();
    error UnsupportedToken();
    error InvalidAmount();
    error CapExceeded();
    error IsFrozen();
    error AlreadyReleased();
    error VerificationFailed();
    error TransferFailed();
    error TreeFull();
    error Reentrancy();
    error BadConfig();

    modifier onlyAdmin() {
        if (msg.sender != admin) revert NotAdmin();
        _;
    }

    modifier nonReentrant() {
        if (locked != 1) revert Reentrancy();
        locked = 2;
        _;
        locked = 1;
    }

    constructor(bytes32 pairing, IBurnVerifier verifier, uint64 window, address admin_, address freezer_) {
        if (address(verifier) == address(0) || window == 0) revert BadConfig();
        PAIRING = pairing;
        VERIFIER = verifier;
        WINDOW = window;
        admin = admin_;
        freezer = freezer_;
        bytes32 z;
        for (uint256 i; i < DEPTH; ++i) {
            zeros[i] = z;
            filledSubtrees[i] = z;
            z = sha256(abi.encodePacked(z, z));
        }
        root = z;
        knownRoot[z] = true;
    }

    // ---- admin (RFP-001 stand-in) ----

    /// Register or update a token. Every permitted amount must be a whole number of base units
    /// that fits LEZ's u128 balances, which is what the LEZ program mints one to one.
    function registerToken(address token, uint8 decimals, uint256[] calldata amounts, uint128 depositCap, uint128 releaseCap)
        external
        onlyAdmin
    {
        if (amounts.length == 0) revert InvalidAmount();
        TokenConfig storage t = tokens[token];
        for (uint256 i; i < amounts.length; ++i) {
            if (amounts[i] == 0 || amounts[i] > type(uint128).max) revert InvalidAmount();
            permitted[token][amounts[i]] = true;
        }
        t.listed = true;
        t.registered = true;
        t.decimals = decimals;
        t.depositCap = depositCap;
        t.releaseCap = releaseCap;
        emit TokenRegistered(token, decimals, amounts, depositCap, releaseCap);
    }

    /// Stops new deposits of `token`. Releases continue, so no holder is trapped.
    function delistToken(address token) external onlyAdmin {
        tokens[token].listed = false;
        emit TokenDelisted(token);
    }

    function setFreezer(address f) external onlyAdmin {
        freezer = f;
    }

    // ---- freeze (RFP-002 stand-in): independent of the LEZ half (Security #5) ----

    modifier onlyFreezer() {
        if (msg.sender != freezer && msg.sender != admin) revert NotFreezer();
        _;
    }

    /// Pause or resume deposits and/or releases of one token.
    function freeze(address token, bool deposits, bool releases) external onlyFreezer {
        tokens[token].depositsFrozen = deposits;
        tokens[token].releasesFrozen = releases;
        emit Frozen(token, deposits, releases);
    }

    /// Pause or resume deposits and/or releases of every token.
    function freezeAll(bool deposits, bool releases) external onlyFreezer {
        depositsFrozen = deposits;
        releasesFrozen = releases;
        emit Frozen(address(type(uint160).max), deposits, releases);
    }

    // ---- deposit ----

    function leafOf(address token, uint256 amount, bytes32 commitment) public view returns (bytes32) {
        return sha256(abi.encode(LEAF_TAG, block.chainid, address(this), PAIRING, token, amount, commitment));
    }

    /// Escrow `amount` of `token` (address(0) for ETH, sent as msg.value) against `commitment`.
    function deposit(address token, uint256 amount, bytes32 commitment) external payable nonReentrant returns (uint256 index) {
        TokenConfig storage t = tokens[token];
        if (!t.listed) revert UnsupportedToken();
        if (depositsFrozen || t.depositsFrozen) revert IsFrozen();
        if (!permitted[token][amount]) revert InvalidAmount();
        if (block.timestamp >= t.depositWindowStart + WINDOW) {
            t.depositWindowStart = uint64(block.timestamp);
            t.depositUsed = 0;
        }
        if (uint256(t.depositUsed) + amount > t.depositCap) revert CapExceeded();
        t.depositUsed += uint128(amount);

        if (token == ETH) {
            if (msg.value != amount) revert InvalidAmount();
        } else {
            if (msg.value != 0) revert InvalidAmount();
            uint256 before = _balanceOf(token);
            _call(token, abi.encodeWithSelector(0x23b872dd, msg.sender, address(this), amount)); // transferFrom
            // A fee-on-transfer or rebasing token would back fewer units than it mints.
            if (_balanceOf(token) - before != amount) revert TransferFailed();
        }

        bytes32 leaf = leafOf(token, amount, commitment);
        index = _insert(leaf);
        emit Deposit(index, leaf, commitment, token, amount, root);
    }

    function _insert(bytes32 leaf) internal returns (uint256 index) {
        index = nextIndex;
        if (index >= 1 << DEPTH) revert TreeFull();
        nextIndex = index + 1;
        bytes32 node = leaf;
        uint256 i = index;
        for (uint256 level; level < DEPTH; ++level) {
            if (i & 1 == 0) {
                filledSubtrees[level] = node;
                node = sha256(abi.encodePacked(node, zeros[level]));
            } else {
                node = sha256(abi.encodePacked(filledSubtrees[level], node));
            }
            i >>= 1;
        }
        root = node;
        knownRoot[node] = true;
    }

    // ---- release ----

    /// The single public-input word a release proof commits to: the pairing, this vault and
    /// chain, the asset and amount, the Ethereum destination and the relayer's fixed fee (so a
    /// relayer can neither redirect nor overcharge, Security #3), the LEZ burn-tree root and the
    /// burn's nullifier.
    function releaseDigest(
        address token,
        uint256 amount,
        address recipient,
        address relayer,
        uint256 fee,
        bytes32 lezBurnRoot,
        bytes32 nullifier
    ) public view returns (bytes32) {
        return sha256(
            abi.encode(RELEASE_TAG, block.chainid, address(this), PAIRING, token, amount, recipient, relayer, fee, lezBurnRoot, nullifier)
        );
    }

    function release(
        address token,
        uint256 amount,
        address payable recipient,
        address payable relayer,
        uint256 fee,
        bytes32 lezBurnRoot,
        bytes32 nullifier,
        bytes calldata proof
    ) external nonReentrant {
        TokenConfig storage t = tokens[token];
        if (!t.registered) revert UnsupportedToken();
        if (releasesFrozen || t.releasesFrozen) revert IsFrozen();
        if (!permitted[token][amount] || fee > amount) revert InvalidAmount();
        if (nullifierSpent[nullifier]) revert AlreadyReleased();
        if (!VERIFIER.verify(proof, releaseDigest(token, amount, recipient, relayer, fee, lezBurnRoot, nullifier))) {
            revert VerificationFailed();
        }
        if (block.timestamp >= t.releaseWindowStart + WINDOW) {
            t.releaseWindowStart = uint64(block.timestamp);
            t.releaseUsed = 0;
        }
        if (uint256(t.releaseUsed) + amount > t.releaseCap) revert CapExceeded();
        t.releaseUsed += uint128(amount);
        nullifierSpent[nullifier] = true;

        _pay(token, recipient, amount - fee);
        if (fee != 0) _pay(token, relayer, fee);
        emit Release(nullifier, token, amount, recipient, relayer, fee);
    }

    // ---- token plumbing: works with tokens whose transfer functions return nothing (USDT, F.17) ----

    function _pay(address token, address payable to, uint256 amount) internal {
        if (token == ETH) {
            (bool ok,) = to.call{value: amount}("");
            if (!ok) revert TransferFailed();
        } else {
            _call(token, abi.encodeWithSelector(0xa9059cbb, to, amount)); // transfer
        }
    }

    function _call(address token, bytes memory data) internal {
        if (token.code.length == 0) revert TransferFailed();
        (bool ok, bytes memory ret) = token.call(data);
        if (!ok || (ret.length != 0 && (ret.length != 32 || abi.decode(ret, (uint256)) != 1))) revert TransferFailed();
    }

    function _balanceOf(address token) internal view returns (uint256) {
        (bool ok, bytes memory ret) = token.staticcall(abi.encodeWithSelector(0x70a08231, address(this)));
        if (!ok || ret.length < 32) revert TransferFailed();
        return abi.decode(ret, (uint256));
    }
}

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
/// spend a nullifier once (R.3).
///
/// Value at risk is bounded by rate limits over a rolling window (F.11, Security #4): per token and
/// direction, in token units, and globally per direction, in a common value unit. Each limit is a
/// bucket that fills with every deposit or release and drains linearly, `cap` per `WINDOW`; a
/// transfer is refused if it would take the bucket over `cap`. So no burst exceeds `cap`, and
/// over any interval of length T at most `cap * (1 + T / WINDOW)` moves: the boundary burst of a
/// fixed window (a full cap just before the reset, another just after) is gone.
///
/// Roles (minimal stand-ins for the RFP-001 and RFP-002 libraries):
/// - the freezer (guardian) pauses, at once and with no delay, deposits and/or releases of one
///   token or of all. It can do nothing else: it cannot unpause, configure, or move funds.
/// - the admin registers tokens and tightens limits at once. Anything that loosens (raising a
///   cap, lowering a token's value, relisting, adding a denomination, shortening the delay) and
///   anything that touches a pause or a role (unpausing, rotating the freezer or the admin) runs
///   only as an operation the admin scheduled `delay` earlier, within `GRACE` after it matured.
///   An unpause names the pause count it lifts, so one scheduled before the freezer's latest
///   pause cannot lift it: the admin cannot bypass a pause, even with an operation kept ready.
///   Rotating the admin bumps the epoch every operation is keyed on, voiding the old admin's
///   pending operations. The admin and the freezer cannot be rotated into the same account.
contract WrapVault {
    // ---- storage: `knownRoot` must stay the first variable (slot 0); the LEZ program proves it ----
    mapping(bytes32 => bool) public knownRoot;

    struct TokenConfig {
        bool listed; // accepts deposits; a delisted token still releases (no funds trapped)
        bool registered; // has ever been listed: releases stay possible
        bool depositsFrozen;
        bool releasesFrozen;
        uint8 decimals;
        uint128 depositCap; // bucket size in token units: at most this in a burst, refilled per WINDOW; 0 = no deposits
        uint128 releaseCap;
        uint128 value; // value units per whole token (10**decimals units), for the global caps; 0 = unpriced
    }

    /// A rolling-window rate limit: `level` drains linearly at `cap / WINDOW` per second since `updated`.
    struct Bucket {
        uint128 level;
        uint64 updated;
    }

    mapping(address => TokenConfig) public tokens;
    mapping(address => mapping(uint256 => bool)) public permitted;
    mapping(bytes32 => bool) public nullifierSpent;

    uint256 public constant DEPTH = 24;
    bytes32 public constant LEAF_TAG = "rfp022/wrap/leaf/v1";
    bytes32 public constant RELEASE_TAG = "rfp022/wrap/release/v1";
    address public constant ETH = address(0);

    // ---- hard bounds ----
    uint64 public constant MIN_WINDOW = 1 hours;
    uint64 public constant MAX_WINDOW = 30 days;
    uint64 public constant MIN_DELAY = 1 days;
    uint64 public constant MAX_DELAY = 30 days;
    uint64 public constant DEFAULT_DELAY = 2 days;
    uint64 public constant GRACE = 14 days; // a matured operation not executed by then is void
    uint8 public constant MAX_DECIMALS = 36;
    uint96 public constant UNCAPPED = type(uint96).max; // a global cap at this value is off

    bytes32[DEPTH] public zeros;
    bytes32[DEPTH] public filledSubtrees;
    uint256 public nextIndex;
    bytes32 public root;

    /// sha256("rfp022/pair/v1" ‖ LEZ chain ‖ zone ‖ program ‖ deployment): the one LEZ deployment
    /// whose claims and burns this vault honours (F.15).
    bytes32 public immutable PAIRING;
    IBurnVerifier public immutable VERIFIER;
    uint64 public immutable WINDOW; // rolling window, seconds, in [MIN_WINDOW, MAX_WINDOW]

    address public admin;
    address public freezer;
    address public pendingAdmin;
    uint64 public delay;
    uint64 public adminEpoch;
    uint64 public pauseCount; // bumped by every pause that freezes something not yet frozen
    uint256 private locked = 1;

    // One slot, read by every deposit and release: the global pause and the global caps per
    // direction, in value units (see `TokenConfig.value`); UNCAPPED = off.
    bool public depositsFrozen;
    bool public releasesFrozen;
    uint96 public globalDepositCap;
    uint96 public globalReleaseCap;

    mapping(address => Bucket) public depositBucket;
    mapping(address => Bucket) public releaseBucket;
    Bucket public globalDepositBucket;
    Bucket public globalReleaseBucket;

    /// Scheduled operations: keccak256(abi.encode(adminEpoch, calldata)) => earliest execution time.
    mapping(bytes32 => uint256) public readyAt;

    event Deposit(uint256 indexed index, bytes32 leaf, bytes32 commitment, address indexed token, uint256 amount, bytes32 root);
    event Release(bytes32 indexed nullifier, address indexed token, uint256 amount, address recipient, address relayer, uint256 fee);
    event TokenRegistered(address indexed token, uint8 decimals, uint256[] amounts, uint128 depositCap, uint128 releaseCap);
    event TokenDelisted(address indexed token);
    event TokenValueSet(address indexed token, uint128 value);
    event GlobalCapsSet(uint96 depositCap, uint96 releaseCap);
    event Frozen(address indexed token, bool deposits, bool releases); // the state after the change
    event Scheduled(bytes32 indexed id, bytes data, uint256 readyAt);
    event Cancelled(bytes32 indexed id);
    event Executed(bytes32 indexed id);
    event DelaySet(uint64 delay);
    event FreezerSet(address indexed freezer);
    event AdminProposed(address indexed pendingAdmin);
    event AdminSet(address indexed admin);

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
    error NotScheduled();
    error TooEarly();
    error Expired();
    error AlreadyScheduled();
    error StalePause();
    error Unpriced();

    modifier onlyAdmin() {
        if (msg.sender != admin) revert NotAdmin();
        _;
    }

    modifier onlyFreezer() {
        if (msg.sender != freezer) revert NotFreezer();
        _;
    }

    modifier nonReentrant() {
        if (locked != 1) revert Reentrancy();
        locked = 2;
        _;
        locked = 1;
    }

    /// `admin_` may equal `freezer_` only in a test rig: the separation of the two roles is then
    /// void. A rotation can never make them equal.
    constructor(bytes32 pairing, IBurnVerifier verifier, uint64 window, address admin_, address freezer_) {
        if (address(verifier) == address(0) || window < MIN_WINDOW || window > MAX_WINDOW) revert BadConfig();
        if (admin_ == address(0) || freezer_ == address(0)) revert BadConfig();
        PAIRING = pairing;
        VERIFIER = verifier;
        WINDOW = window;
        admin = admin_;
        freezer = freezer_;
        delay = DEFAULT_DELAY;
        globalDepositCap = UNCAPPED;
        globalReleaseCap = UNCAPPED;
        bytes32 z;
        for (uint256 i; i < DEPTH; ++i) {
            zeros[i] = z;
            filledSubtrees[i] = z;
            z = sha256(abi.encodePacked(z, z));
        }
        root = z;
        knownRoot[z] = true;
    }

    // ---- timelock ----

    function operationId(bytes calldata data) public view returns (bytes32) {
        return keccak256(abi.encode(adminEpoch, data));
    }

    /// Schedule `data`, the exact calldata of a later call by the admin to this contract.
    function schedule(bytes calldata data) external onlyAdmin returns (bytes32 id, uint256 at) {
        if (data.length < 4) revert BadConfig();
        id = operationId(data);
        if (readyAt[id] != 0) revert AlreadyScheduled();
        at = block.timestamp + delay;
        readyAt[id] = at;
        emit Scheduled(id, data, at);
    }

    function cancel(bytes32 id) external onlyAdmin {
        if (readyAt[id] == 0) revert NotScheduled();
        delete readyAt[id];
        emit Cancelled(id);
    }

    /// Consume the operation that is this very call, or revert. Called after `onlyAdmin`.
    function _timelocked() internal {
        bytes32 id = keccak256(abi.encode(adminEpoch, msg.data));
        uint256 at = readyAt[id];
        if (at == 0) revert NotScheduled();
        if (block.timestamp < at) revert TooEarly();
        if (block.timestamp > at + GRACE) revert Expired();
        readyAt[id] = 0;
        emit Executed(id);
    }

    function _authorize(bool loosens) internal {
        if (loosens) _timelocked();
    }

    // ---- admin (RFP-001 stand-in) ----

    /// Register or update a token. Every permitted amount must be a whole number of base units
    /// that fits LEZ's u128 balances, which is what the LEZ program mints one to one. A first
    /// registration, and an update that only lowers caps, take effect at once; an update that
    /// raises a cap, relists, or adds a denomination must have been scheduled.
    function registerToken(address token, uint8 decimals, uint256[] calldata amounts, uint128 depositCap, uint128 releaseCap)
        external
        onlyAdmin
    {
        if (amounts.length == 0) revert InvalidAmount();
        if (decimals > MAX_DECIMALS) revert BadConfig();
        TokenConfig storage t = tokens[token];
        bool loosens;
        if (t.registered) {
            if (decimals != t.decimals) revert BadConfig(); // the value of a unit cannot change under the caps
            loosens = !t.listed || depositCap > t.depositCap || releaseCap > t.releaseCap;
            _settle(depositBucket[token], t.depositCap);
            _settle(releaseBucket[token], t.releaseCap);
        }
        for (uint256 i; i < amounts.length; ++i) {
            if (amounts[i] == 0 || amounts[i] > type(uint128).max) revert InvalidAmount();
            if (t.registered && !permitted[token][amounts[i]]) loosens = true;
            permitted[token][amounts[i]] = true;
        }
        _authorize(loosens);
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

    /// The value of one whole token, in the unit of the global caps. Raising it only tightens and
    /// is immediate; lowering it, or pricing an unpriced token, must have been scheduled. While a
    /// global cap is on, an unpriced token is refused in that direction.
    function setTokenValue(address token, uint128 value) external onlyAdmin {
        TokenConfig storage t = tokens[token];
        if (!t.registered) revert UnsupportedToken();
        uint128 old = t.value;
        _authorize(value != 0 && (old == 0 || value < old));
        t.value = value;
        emit TokenValueSet(token, value);
    }

    /// Global caps per direction, in value units; UNCAPPED turns one off. Lowering is immediate,
    /// raising must have been scheduled.
    function setGlobalCaps(uint96 depositCap, uint96 releaseCap) external onlyAdmin {
        _authorize(depositCap > globalDepositCap || releaseCap > globalReleaseCap);
        _settle(globalDepositBucket, globalDepositCap);
        _settle(globalReleaseBucket, globalReleaseCap);
        globalDepositCap = depositCap;
        globalReleaseCap = releaseCap;
        emit GlobalCapsSet(depositCap, releaseCap);
    }

    /// Lengthening the delay is immediate, shortening it must have been scheduled.
    function setDelay(uint64 d) external onlyAdmin {
        if (d < MIN_DELAY || d > MAX_DELAY) revert BadConfig();
        _authorize(d < delay);
        delay = d;
        emit DelaySet(d);
    }

    /// Rotate the freezer. Always scheduled; the freezer cannot block it.
    function setFreezer(address f) external onlyAdmin {
        if (f == address(0) || f == admin) revert BadConfig();
        _timelocked();
        freezer = f;
        emit FreezerSet(f);
    }

    /// Propose a new admin. Always scheduled; takes effect when the new admin accepts.
    function transferAdmin(address a) external onlyAdmin {
        if (a == address(0) || a == freezer) revert BadConfig();
        _timelocked();
        pendingAdmin = a;
        emit AdminProposed(a);
    }

    /// The proposed admin takes over. Every operation the previous admin scheduled is void.
    function acceptAdmin() external {
        if (msg.sender != pendingAdmin || msg.sender == address(0)) revert NotAdmin();
        if (msg.sender == freezer) revert BadConfig();
        admin = msg.sender;
        pendingAdmin = address(0);
        ++adminEpoch;
        emit AdminSet(msg.sender);
    }

    // ---- freeze (RFP-002 stand-in): independent of the LEZ half (Security #5) and of the admin ----

    /// Pause deposits and/or releases of one token, at once. `false` leaves a direction as it is:
    /// the freezer cannot unpause.
    function freeze(address token, bool deposits, bool releases) external onlyFreezer {
        TokenConfig storage t = tokens[token];
        if (!t.registered) revert UnsupportedToken();
        if ((deposits && !t.depositsFrozen) || (releases && !t.releasesFrozen)) ++pauseCount;
        if (deposits) t.depositsFrozen = true;
        if (releases) t.releasesFrozen = true;
        emit Frozen(token, t.depositsFrozen, t.releasesFrozen);
    }

    /// Pause deposits and/or releases of every token, at once.
    function freezeAll(bool deposits, bool releases) external onlyFreezer {
        if ((deposits && !depositsFrozen) || (releases && !releasesFrozen)) ++pauseCount;
        if (deposits) depositsFrozen = true;
        if (releases) releasesFrozen = true;
        emit Frozen(address(type(uint160).max), depositsFrozen, releasesFrozen);
    }

    /// Lift a pause of one token. Admin only, scheduled, and only if no pause happened since
    /// `count` (the `pauseCount` the operation was scheduled against).
    function unfreeze(address token, bool deposits, bool releases, uint64 count) external onlyAdmin {
        if (count != pauseCount) revert StalePause();
        _timelocked();
        TokenConfig storage t = tokens[token];
        if (deposits) t.depositsFrozen = false;
        if (releases) t.releasesFrozen = false;
        emit Frozen(token, t.depositsFrozen, t.releasesFrozen);
    }

    /// Lift a global pause, under the same rules as `unfreeze`.
    function unfreezeAll(bool deposits, bool releases, uint64 count) external onlyAdmin {
        if (pauseCount != count) revert StalePause();
        _timelocked();
        if (deposits) depositsFrozen = false;
        if (releases) releasesFrozen = false;
        emit Frozen(address(type(uint160).max), depositsFrozen, releasesFrozen);
    }

    // ---- rate limits ----

    function _level(Bucket storage b, uint256 cap) internal view returns (uint256) {
        uint256 level = b.level;
        if (level == 0) return 0;
        uint256 drained = (block.timestamp - b.updated) * cap / WINDOW;
        return drained >= level ? 0 : level - drained;
    }

    function _spend(Bucket storage b, uint256 cap, uint256 amount) internal {
        uint256 level = _level(b, cap) + amount;
        if (level > cap) revert CapExceeded();
        b.level = uint128(level);
        b.updated = uint64(block.timestamp);
    }

    /// Drain a bucket at the old cap's rate before the cap changes.
    function _settle(Bucket storage b, uint256 cap) internal {
        b.level = uint128(_level(b, cap));
        b.updated = uint64(block.timestamp);
    }

    /// `amount` in value units, rounded up.
    function _value(TokenConfig storage t, uint256 amount) internal view returns (uint256) {
        uint256 v = t.value;
        if (v == 0) revert Unpriced();
        uint256 unit = 10 ** t.decimals;
        return (amount * v + unit - 1) / unit;
    }

    /// What can still move now, in `token` units for the token's own limit and in value units for
    /// the global one (UNCAPPED when off).
    function available(address token, bool releases) external view returns (uint256 tokenUnits, uint256 globalValue) {
        TokenConfig storage t = tokens[token];
        uint256 cap = releases ? t.releaseCap : t.depositCap;
        uint256 level = _level(releases ? releaseBucket[token] : depositBucket[token], cap);
        tokenUnits = level >= cap ? 0 : cap - level;
        uint256 g = releases ? globalReleaseCap : globalDepositCap;
        if (g == UNCAPPED) return (tokenUnits, UNCAPPED);
        level = _level(releases ? globalReleaseBucket : globalDepositBucket, g);
        globalValue = level >= g ? 0 : g - level;
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
        _spend(depositBucket[token], t.depositCap, amount);
        uint256 g = globalDepositCap;
        if (g != UNCAPPED) _spend(globalDepositBucket, g, _value(t, amount));

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
        _spend(releaseBucket[token], t.releaseCap, amount);
        uint256 g = globalReleaseCap;
        if (g != UNCAPPED) _spend(globalReleaseBucket, g, _value(t, amount));
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

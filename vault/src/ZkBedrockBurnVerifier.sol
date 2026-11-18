// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {IBurnVerifier} from "./IBurnVerifier.sol";
import {BedrockHeaderCommittee} from "./BedrockHeaderCommittee.sol";
import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";

/// @title ZkBedrockBurnVerifier: the release check (F.7) with the burn-tree root derived from LEZ
/// blocks and the chain's tip anchored in Bedrock, inside one proof.
///
/// `ZkChainBurnVerifier` derives the root from LEZ blocks, and a committee says the tip LEZ block
/// is final. Here the proof also shows where that tip is on Bedrock. One Groth16 seal covers
/// three RISC Zero guests, each verifying the previous one's receipt inside:
///
/// 1. **The chain** (`zk/lez_chain`, `CHAIN_IMAGE_ID`): LEZ v0.3.1 blocks from the pinned anchor
///    to a tip, each hash against its contents and its link, and the tree of the bridge's private
///    burns (as in `ZkChainBurnVerifier`).
/// 2. **The anchor** (`zk/bedrock_anchor`, `ANCHOR_IMAGE_ID`): that tip block, byte for byte, is
///    the inscription of the first operation of a Mantle transaction on `CHANNEL_ID`, signed by
///    the key the journal names (Ed25519 `verify_strict` over the transaction hash); the transaction is
///    under the Merkle root a Bedrock header's body root commits to, and a chain of headers (each
///    the parent of the next, at later slots) is built on that header, `depth` of them, the last
///    being the top header id.
/// 3. **The burn** (`zk/bedrock_release`, `IMAGE_ID`): the opening under the chain tree's root,
///    as in `ZkChainBurnVerifier`, the journal starting with the vault's 352-byte preimage.
///
/// The verifier then requires that key to be one of the channel's keys this deployment pins
/// (`channelKeys`), `depth >= MIN_DEPTH` (Bedrock's security parameter k, 30 blocks on the
/// deployment the node compiles in), and that `BedrockHeaderCommittee` attests the top header
/// canonical, so the inscription is final by Bedrock's rule.
///
/// **The channel's keys.** A Bedrock channel accredits a list of Ed25519 keys and rotates which of
/// them may post (`posting_timeframe`, `posting_timeout`); changing the list takes
/// `configuration_threshold` of them (a ChannelConfig operation). The public LEZ testnet's channel
/// accredits four keys, configuration threshold 3, and its inscriptions are signed by all four in
/// turn. So the deployment pins the whole set, sorted, fixed at construction like every other
/// parameter (Security #8: a new key set is a new verifier, through the vault's timelocked
/// redeploy path), and accepts an inscription signed by any key in it. `CHANNEL_KEYS_HASH` is
/// sha256(KEYS_TAG ‖ the sorted keys), published so off-chain tools compare one word. Bedrock,
/// not the proof, checks that the signer held the posting turn at that slot and was accredited
/// then; a key the channel adds later is refused here until a new verifier pins it (fail-closed),
/// and a key it removes stays accepted here, but Bedrock no longer accepts its inscriptions on the
/// canonical chain the committee attests.
///
/// **Trust assumption, stated plainly.** Cryptographic: RISC Zero's proof system, Groth16 with
/// RISC Zero's setup, Blake2b-256, Ed25519, SHA-256. Not proven: (a) that the top header is on
/// Bedrock's canonical chain: the committee says so (a header chain can be made up; checking a
/// proof of leadership needs Bedrock's note-set roots, which no header commits to); (b) that the
/// leader signatures of those blocks are valid, and that the signing key held the channel's
/// posting turn at that slot (both Bedrock-checked, neither in headers); (c) that the LEZ blocks
/// are valid (the sequencer and LEZ nodes check them); (d) that the inscribed block is the only
/// LEZ block the channel carries at its height (a sequencer could inscribe two). Forging a release
/// against a burn that never happened needs the committee threshold **and** one of the channel's
/// keys, or a channel key alone inscribing a block LEZ would refuse.
contract ZkBedrockBurnVerifier is BedrockHeaderCommittee, IBurnVerifier {
    struct Release {
        bytes seal;
        bytes journal;
        bytes[] signatures;
    }

    /// The release guest's journal: the 11-word release preimage, then 7 chain words, 7 Bedrock words.
    uint256 public constant JOURNAL_LEN = 25 * 32;

    IRiscZeroVerifier public immutable RISC0;
    bytes32 public immutable IMAGE_ID;
    bytes32 public immutable CHAIN_IMAGE_ID;
    bytes32 public immutable ANCHOR_IMAGE_ID;
    bytes32 public immutable PROGRAM_ID;
    bytes32 public immutable BURNS_ACCOUNT;
    uint64 public immutable ANCHOR_ID;
    bytes32 public immutable ANCHOR_HASH;
    bytes32 public immutable CHANNEL_ID;
    uint32 public immutable MIN_DEPTH;
    /// sha256(KEYS_TAG ‖ the pinned channel keys, ascending).
    bytes32 public immutable CHANNEL_KEYS_HASH;
    bytes32 public constant KEYS_TAG = bytes32("rfp022/bedrock-channel-keys/v1");
    /// A Bedrock channel's key list is indexed by a u16; a deployment pins far fewer.
    uint256 public constant MAX_CHANNEL_KEYS = 32;

    bytes32[] private _channelKeys;
    mapping(bytes32 => bool) public isChannelKey;

    error NotThisRelease();
    error NotThisChain();
    error NotThisChannel();
    error NotDeepEnough();

    struct Config {
        IRiscZeroVerifier risc0;
        bytes32 imageId;
        bytes32 chainImageId;
        bytes32 anchorImageId;
        bytes32 programId;
        bytes32 burnsAccount;
        uint64 anchorId;
        bytes32 anchorHash;
        bytes32 channelId;
        bytes32[] channelKeys;
        uint32 minDepth;
    }

    constructor(Config memory c, address[] memory committee, uint256 threshold)
        BedrockHeaderCommittee(committee, threshold)
    {
        if (
            address(c.risc0) == address(0) || c.imageId == bytes32(0) || c.chainImageId == bytes32(0)
                || c.anchorImageId == bytes32(0) || c.programId == bytes32(0) || c.burnsAccount == bytes32(0)
                || c.anchorHash == bytes32(0) || c.channelId == bytes32(0) || c.minDepth == 0
        ) revert BadConfig();
        RISC0 = c.risc0;
        IMAGE_ID = c.imageId;
        CHAIN_IMAGE_ID = c.chainImageId;
        ANCHOR_IMAGE_ID = c.anchorImageId;
        PROGRAM_ID = c.programId;
        BURNS_ACCOUNT = c.burnsAccount;
        ANCHOR_ID = c.anchorId;
        ANCHOR_HASH = c.anchorHash;
        CHANNEL_ID = c.channelId;
        MIN_DEPTH = c.minDepth;
        // The channel's keys: at least one, at most MAX_CHANNEL_KEYS, strictly ascending (so no
        // duplicate, and one set has one hash), none zero.
        uint256 n = c.channelKeys.length;
        if (n == 0 || n > MAX_CHANNEL_KEYS) revert BadConfig();
        for (uint256 i; i < n; ++i) {
            bytes32 k = c.channelKeys[i];
            if (k == bytes32(0) || (i > 0 && uint256(k) <= uint256(c.channelKeys[i - 1]))) revert BadConfig();
            _channelKeys.push(k);
            isChannelKey[k] = true;
        }
        CHANNEL_KEYS_HASH = sha256(abi.encodePacked(KEYS_TAG, c.channelKeys));
    }

    /// The pinned channel keys, ascending.
    function channelKeys() external view returns (bytes32[] memory) {
        return _channelKeys;
    }

    function verify(bytes calldata proof, bytes32 publicInputs) external view returns (bool) {
        Release memory r = abi.decode(proof, (Release));
        bytes memory j = r.journal;
        if (j.length != JOURNAL_LEN) revert NotThisRelease();

        // 1. Its first 352 bytes are the release the vault asks about.
        if (_sha256Prefix(j, 352) != publicInputs) revert NotThisRelease();

        // 2. This deployment's chain, from the pinned anchor, by the pinned chain guest.
        if (
            _word(j, 11) != CHAIN_IMAGE_ID || _word(j, 12) != PROGRAM_ID || _word(j, 13) != BURNS_ACCOUNT
                || _word(j, 14) != bytes32(uint256(ANCHOR_ID)) || _word(j, 15) != ANCHOR_HASH
        ) revert NotThisChain();

        // 3. Anchored by the pinned anchor guest, on the pinned channel, signed by one of the
        //    channel's pinned keys (the anchor guest proved the signature by the key it names), deep enough.
        if (_word(j, 18) != ANCHOR_IMAGE_ID || _word(j, 19) != CHANNEL_ID || !isChannelKey[_word(j, 20)]) {
            revert NotThisChannel();
        }
        if (uint256(_word(j, 24)) < MIN_DEPTH) revert NotDeepEnough();

        // 4. The release guest proved the whole journal (and, inside, the anchor and the chain).
        RISC0.verify(r.seal, IMAGE_ID, sha256(j));

        // 5. The top Bedrock header: canonical, attested by THRESHOLD distinct members.
        _requireCanonicalHeader(msg.sender, _word(j, 3), _word(j, 22), r.signatures);
        return true;
    }

    /// sha256 of the first `n` bytes of `j`.
    function _sha256Prefix(bytes memory j, uint256 n) internal view returns (bytes32 d) {
        assembly {
            let ok := staticcall(gas(), 0x02, add(j, 32), n, 0x00, 0x20)
            if iszero(ok) { revert(0, 0) }
            d := mload(0x00)
        }
    }

    /// Word `i` of the journal.
    function _word(bytes memory j, uint256 i) internal pure returns (bytes32 w) {
        assembly {
            w := mload(add(j, add(32, mul(32, i))))
        }
    }
}

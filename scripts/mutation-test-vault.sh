#!/usr/bin/env bash
# The release path's checks, each removed in turn, must fail the Foundry tests
# (vault/test/CommitteeBurnVerifier.t.sol, ZkBurnVerifier.t.sol,
# ZkChainBurnVerifier.t.sol and ZkBedrockBurnVerifier.t.sol, whose seals are
# real Groth16 proofs, and WrapVault.t.sol). The sources are restored at the end.
#   scripts/mutation-test-vault.sh
set -uo pipefail
cd "$(dirname "$0")/.."
V=vault/src/CommitteeBurnVerifier.sol
C=vault/src/BurnRootCommittee.sol
Z=vault/src/ZkBurnVerifier.sol
W=vault/src/WrapVault.sol
ZC=vault/src/ZkChainBurnVerifier.sol
LB=vault/src/LezBlockCommittee.sol
ZB=vault/src/ZkBedrockBurnVerifier.sol
BH=vault/src/BedrockHeaderCommittee.sol
SAVE="$(mktemp -d /tmp/rfp022-mut-vault.XXXXXX)"
for f in "$V" "$C" "$Z" "$W" "$ZC" "$LB" "$ZB" "$BH"; do cp "$f" "$SAVE/"; done
restore() { for f in "$V" "$C" "$Z" "$W" "$ZC" "$LB" "$ZB" "$BH"; do cp "$SAVE/$(basename "$f")" "$f"; done; }
trap restore EXIT

# name @@ file @@ literal text @@ replacement (\& is &, \n a newline)
MUTATIONS=(
  "release digest not compared@@$V@@ != publicInputs) {@@ != publicInputs \&\& false) {"
  "finality depth not checked@@$C@@if (uint256(lezFinalBlock) < uint256(lezBlock) + FINALITY_DEPTH) revert NotFinal();@@if (false) revert NotFinal();"
  "threshold not checked@@$C@@if (count < THRESHOLD) revert RootNotAttested();@@if (false) revert RootNotAttested();"
  "a non-member's signature counts@@$C@@uint256 m = memberIndex[_recover(signed, signatures[i])];@@uint256 m = memberIndex[_recover(signed, signatures[i])]; if (m == 0) m = 1;"
  "a member counted twice@@$C@@ || seen \& (1 << (m - 1)) != 0) continue;@@) continue;"
  "high-s signatures accepted@@$C@@if (uint256(s) > 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0 || (v != 27 \&\& v != 28)) {@@if (v != 27 \&\& v != 28) {"
  "attestation not bound to chain and vault@@$C@@abi.encode(ATTEST_TAG, block.chainid, vault, pairing,@@abi.encode(ATTEST_TAG, uint256(31337), address(0), pairing,"
  "pairing not read from the vault@@$V@@bytes32 pairing = vault.PAIRING();@@bytes32 pairing = keccak256(\"test pairing\");"
  "fee not bound by the burn@@$V@@abi.encode(ETH_COMMIT_TAG, recipient, fee, secret)@@abi.encode(ETH_COMMIT_TAG, recipient, uint256(0), secret)"
  "index range not checked@@$V@@if (index >= 1 << DEPTH) revert BadOpening();@@"
  "burn nullifier ignores the leaf index@@$V@@bytes32 nullifier = burnNullifier(pairing, r.secret, r.index);@@bytes32 nullifier = burnNullifier(pairing, r.secret, 0);"
  "zk: the journal not compared with the vault's digest@@$Z@@if (sha256(r.journal) != publicInputs) revert NotThisRelease();@@"
  "zk: the seal not verified@@$Z@@RISC0.verify(r.seal, IMAGE_ID, publicInputs);@@"
  "zk: the seal checked against a fixed digest, not the vault's@@$Z@@RISC0.verify(r.seal, IMAGE_ID, publicInputs);@@RISC0.verify(r.seal, IMAGE_ID, bytes32(0xfa8b2865ae30da194ae79a768b3ed4660c58d4396d2d769a62901142dfa6db67));"
  "zk: the committee not consulted@@$Z@@_requireAttested(msg.sender, _word(r.journal, 3), _word(r.journal, 9), r.lezBlock, r.lezFinalBlock, r.signatures);@@"
  "zk: the attested root is not the journal's@@$Z@@_word(r.journal, 9), r.lezBlock@@_word(r.journal, 10), r.lezBlock"
  "zk: the attestation not bound to the journal's pairing@@$Z@@_requireAttested(msg.sender, _word(r.journal, 3),@@_requireAttested(msg.sender, keccak256(\"other pairing\"),"
  "chain: the journal's length not checked@@$ZC@@if (j.length != JOURNAL_LEN) revert NotThisRelease();@@"
  "chain: the release preimage not compared with the vault's digest@@$ZC@@if (_sha256Prefix(j, 352) != publicInputs) revert NotThisRelease();@@"
  "chain: the preimage compared on 320 bytes@@$ZC@@_sha256Prefix(j, 352)@@_sha256Prefix(j, 320)"
  "chain: the chain guest's image id not checked@@$ZC@@_word(j, 11) != CHAIN_IMAGE_ID || @@"
  "chain: the program not checked@@$ZC@@_word(j, 12) != PROGRAM_ID || @@"
  "chain: the burn-tree account not checked@@$ZC@@_word(j, 13) != BURNS_ACCOUNT@@false"
  "chain: the anchor's height not checked@@$ZC@@_word(j, 14) != bytes32(uint256(ANCHOR_ID)) || @@"
  "chain: the anchor's hash not checked@@$ZC@@ || _word(j, 15) != ANCHOR_HASH@@"
  "chain: the seal not verified@@$ZC@@RISC0.verify(r.seal, IMAGE_ID, sha256(j));@@"
  "chain: the seal checked against the preimage's digest only@@$ZC@@RISC0.verify(r.seal, IMAGE_ID, sha256(j));@@RISC0.verify(r.seal, IMAGE_ID, publicInputs);"
  "chain: the tip not attested@@$ZC@@_requireFinalBlock(msg.sender, _word(j, 3), _word(j, 17), uint64(tipId), r.lezFinalBlock, r.signatures);@@"
  "chain: the attested hash is not the journal's tip@@$ZC@@_word(j, 17), uint64(tipId)@@_word(j, 15), uint64(tipId)"
  "chain: the attested height is not the journal's tip@@$ZC@@uint64(tipId), r.lezFinalBlock@@uint64(tipId) + 1, r.lezFinalBlock"
  "block committee: finality depth not checked@@$LB@@if (uint256(lezFinalBlock) < uint256(lezBlock) + FINALITY_DEPTH) revert NotFinal();@@"
  "block committee: threshold not checked@@$LB@@if (count < THRESHOLD) revert BlockNotAttested();@@"
  "block committee: a non-member's signature counts@@$LB@@uint256 m = memberIndex[_recover(signed, signatures[i])];@@uint256 m = memberIndex[_recover(signed, signatures[i])]; if (m == 0) m = 1;"
  "block committee: a member counted twice@@$LB@@ || seen \& (1 << (m - 1)) != 0) continue;@@) continue;"
  "block committee: high-s signatures accepted@@$LB@@if (uint256(s) > 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0 || (v != 27 \&\& v != 28)) {@@if (v != 27 \&\& v != 28) {"
  "block committee: attestation not bound to the vault@@$LB@@abi.encode(BLOCK_ATTEST_TAG, block.chainid, vault,@@abi.encode(BLOCK_ATTEST_TAG, block.chainid, address(0),"
  "block committee: attestation not bound to the pairing@@$LB@@block.chainid, vault, pairing, blockHash,@@block.chainid, vault, bytes32(0), blockHash,"
  "bedrock: the journal's length not checked@@$ZB@@if (j.length != JOURNAL_LEN) revert NotThisRelease();@@"
  "bedrock: the release preimage not compared with the vault's digest@@$ZB@@if (_sha256Prefix(j, 352) != publicInputs) revert NotThisRelease();@@"
  "bedrock: the chain guest's image id not checked@@$ZB@@_word(j, 11) != CHAIN_IMAGE_ID || @@"
  "bedrock: the program not checked@@$ZB@@_word(j, 12) != PROGRAM_ID || @@"
  "bedrock: the burn-tree account not checked@@$ZB@@_word(j, 13) != BURNS_ACCOUNT@@false"
  "bedrock: the LEZ anchor's height not checked@@$ZB@@_word(j, 14) != bytes32(uint256(ANCHOR_ID)) || @@"
  "bedrock: the LEZ anchor's hash not checked@@$ZB@@ || _word(j, 15) != ANCHOR_HASH@@"
  "bedrock: the anchor guest's image id not checked@@$ZB@@_word(j, 18) != ANCHOR_IMAGE_ID || @@"
  "bedrock: the channel not checked@@$ZB@@_word(j, 19) != CHANNEL_ID || @@"
  "bedrock: the sequencer key not checked@@$ZB@@ || _word(j, 20) != SEQUENCER_KEY@@"
  "bedrock: the depth not checked@@$ZB@@if (uint256(_word(j, 24)) < MIN_DEPTH) revert NotDeepEnough();@@"
  "bedrock: the depth checked one short@@$ZB@@uint256(_word(j, 24)) < MIN_DEPTH@@uint256(_word(j, 24)) + 1 < MIN_DEPTH"
  "bedrock: the seal not verified@@$ZB@@RISC0.verify(r.seal, IMAGE_ID, sha256(j));@@"
  "bedrock: the seal checked against the preimage's digest only@@$ZB@@RISC0.verify(r.seal, IMAGE_ID, sha256(j));@@RISC0.verify(r.seal, IMAGE_ID, publicInputs);"
  "bedrock: the top header not attested@@$ZB@@_requireCanonicalHeader(msg.sender, _word(j, 3), _word(j, 22), r.signatures);@@"
  "bedrock: the attested header is the inscription's block, not the top@@$ZB@@_word(j, 3), _word(j, 22), r.signatures@@_word(j, 3), _word(j, 21), r.signatures"
  "bedrock: the attestation not bound to the journal's pairing@@$ZB@@_requireCanonicalHeader(msg.sender, _word(j, 3),@@_requireCanonicalHeader(msg.sender, bytes32(0),"
  "bedrock: a zero minimum depth accepted@@$ZB@@ || c.sequencerKey == bytes32(0) || c.minDepth == 0@@ || c.sequencerKey == bytes32(0)"
  "header committee: threshold not checked@@$BH@@if (count < THRESHOLD) revert HeaderNotAttested();@@"
  "header committee: a non-member's signature counts@@$BH@@uint256 m = memberIndex[_recover(signed, signatures[i])];@@uint256 m = memberIndex[_recover(signed, signatures[i])]; if (m == 0) m = 1;"
  "header committee: a member counted twice@@$BH@@ || seen \& (1 << (m - 1)) != 0) continue;@@) continue;"
  "header committee: high-s signatures accepted@@$BH@@if (uint256(s) > 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0 || (v != 27 \&\& v != 28)) {@@if (v != 27 \&\& v != 28) {"
  "header committee: attestation not bound to the vault@@$BH@@abi.encode(HEADER_ATTEST_TAG, block.chainid, vault,@@abi.encode(HEADER_ATTEST_TAG, block.chainid, address(0),"
  "header committee: attestation not bound to the header@@$BH@@vault, pairing, headerId));@@vault, pairing, bytes32(0)));"
  "vault: release digest not bound to the chain@@$W@@abi.encode(RELEASE_TAG, block.chainid, address(this),@@abi.encode(RELEASE_TAG, uint256(31337), address(this),"
  "vault: release digest not bound to the vault@@$W@@abi.encode(RELEASE_TAG, block.chainid, address(this),@@abi.encode(RELEASE_TAG, block.chainid, address(0xfa22),"
  "vault: release digest omits the relayer@@$W@@token, amount, recipient, relayer, fee, lezBurnRoot, nullifier)@@token, amount, recipient, address(0), fee, lezBurnRoot, nullifier)"
  "vault: a spent nullifier not refused@@$W@@if (nullifierSpent[nullifier]) revert AlreadyReleased();@@"
  "vault: the nullifier not recorded@@$W@@nullifierSpent[nullifier] = true;@@"
  "vault: deposits not rate-limited@@$W@@_spend(depositBucket[token], t.depositCap, amount);@@"
  "vault: releases not rate-limited@@$W@@_spend(releaseBucket[token], t.releaseCap, amount);@@"
  "vault: a bucket never drains@@$W@@uint256 drained = (block.timestamp - b.updated) * cap / WINDOW;@@uint256 drained = 0;"
  "vault: a bucket empties at once after a window (fixed window)@@$W@@return drained >= level ? 0 : level - drained;@@return block.timestamp - b.updated >= WINDOW ? 0 : level;"
  "vault: the global deposit cap ignored@@$W@@if (g != UNCAPPED) _spend(globalDepositBucket, g, _value(t, amount));@@"
  "vault: the global release cap ignored@@$W@@if (g != UNCAPPED) _spend(globalReleaseBucket, g, _value(t, amount));@@"
  "vault: an unpriced token moves free under a global cap@@$W@@if (v == 0) revert Unpriced();@@"
  "vault: a bucket not settled at the old cap before a change@@$W@@_settle(depositBucket[token], t.depositCap);@@"
  "vault: the constructor accepts any window@@$W@@window < MIN_WINDOW || window > MAX_WINDOW@@window == 0"
  "vault: a loosening runs without the timelock@@$W@@if (loosens) _timelocked();@@"
  "vault: a raised deposit cap is not a loosening@@$W@@depositCap > t.depositCap@@false"
  "vault: a raised release cap is not a loosening@@$W@@releaseCap > t.releaseCap;@@false;"
  "vault: relisting is not a loosening@@$W@@loosens = !t.listed || @@loosens = "
  "vault: a new denomination is not a loosening@@$W@@if (t.registered \&\& !permitted[token][amounts[i]]) loosens = true;@@"
  "vault: the decimals of a registered token change@@$W@@if (decimals != t.decimals) revert BadConfig();@@"
  "vault: a lower value is not a loosening@@$W@@_authorize(value != 0 \&\& (old == 0 || value < old));@@_authorize(false);"
  "vault: a raised global cap is not a loosening@@$W@@_authorize(depositCap > globalDepositCap || releaseCap > globalReleaseCap);@@_authorize(false);"
  "vault: a shorter delay is not a loosening@@$W@@_authorize(d < delay);@@_authorize(false);"
  "vault: the delay bounds not checked@@$W@@if (d < MIN_DELAY || d > MAX_DELAY) revert BadConfig();@@"
  "timelock: an operation runs before its delay@@$W@@if (block.timestamp < at) revert TooEarly();@@"
  "timelock: an operation never expires@@$W@@if (block.timestamp > at + GRACE) revert Expired();@@"
  "timelock: an operation is not consumed@@$W@@readyAt[id] = 0;@@"
  "timelock: operations not keyed on the admin epoch@@$W@@bytes32 id = keccak256(abi.encode(adminEpoch, msg.data));@@bytes32 id = keccak256(abi.encode(uint64(0), msg.data));"
  "timelock: a cancel does nothing@@$W@@delete readyAt[id];\n        emit Cancelled(id);@@emit Cancelled(id);"
  "guardian: the admin may pause@@$W@@if (msg.sender != freezer) revert NotFreezer();@@if (msg.sender != freezer \&\& msg.sender != admin) revert NotFreezer();"
  "guardian: freeze can unpause a token@@$W@@if (deposits) t.depositsFrozen = true;@@t.depositsFrozen = deposits;"
  "guardian: freezeAll can unpause@@$W@@if (releases) releasesFrozen = true;@@releasesFrozen = releases;"
  "guardian: a token pause does not count@@$W@@if ((deposits \&\& !t.depositsFrozen) || (releases \&\& !t.releasesFrozen)) ++pauseCount;@@"
  "guardian: a global pause does not count@@$W@@if ((deposits \&\& !depositsFrozen) || (releases \&\& !releasesFrozen)) ++pauseCount;@@"
  "guardian: a repeated pause counts@@$W@@if ((deposits \&\& !depositsFrozen) || (releases \&\& !releasesFrozen)) ++pauseCount;@@if (deposits || releases) ++pauseCount;"
  "unpause: a token unpause runs without the timelock@@$W@@revert StalePause();\n        _timelocked();\n        TokenConfig@@revert StalePause();\n        TokenConfig"
  "unpause: a global unpause runs without the timelock@@$W@@if (pauseCount != count) revert StalePause();\n        _timelocked();@@if (pauseCount != count) revert StalePause();"
  "unpause: a token unpause lifts a later pause@@$W@@if (count != pauseCount) revert StalePause();@@"
  "unpause: a global unpause lifts a later pause@@$W@@if (pauseCount != count) revert StalePause();@@"
  "rotation: the freezer rotates without the timelock@@$W@@_timelocked();\n        freezer = f;@@freezer = f;"
  "rotation: the freezer may become the admin@@$W@@if (f == address(0) || f == admin) revert BadConfig();@@if (f == address(0)) revert BadConfig();"
  "rotation: the admin is proposed without the timelock@@$W@@_timelocked();\n        pendingAdmin = a;@@pendingAdmin = a;"
  "rotation: anyone accepts the admin role@@$W@@if (msg.sender != pendingAdmin || msg.sender == address(0)) revert NotAdmin();@@"
  "rotation: the freezer may accept the admin role@@$W@@if (msg.sender == freezer) revert BadConfig();@@"
  "rotation: the old admin's operations survive@@$W@@++adminEpoch;@@"
)

restore
forge test --root vault >/dev/null 2>&1 || { echo "BASELINE FAIL: the unmutated sources do not pass the tests"; exit 1; }
echo "baseline    the unmutated sources pass ($(forge test --root vault 2>&1 | grep -o '[0-9]* tests passed' | tail -1))"

caught=0; missed=0
for m in "${MUTATIONS[@]}"; do
  name=${m%%@@*}; rest=${m#*@@}; file=${rest%%@@*}; rest=${rest#*@@}; from=${rest%%@@*}; to=${rest#*@@}
  restore
  python3 - "$file" "$from" "$to" <<'EOF' || { echo "SETUP FAIL  $name (text not found)"; missed=$((missed+1)); continue; }
import sys
p, a, b = sys.argv[1], sys.argv[2].replace('\\&', '&').replace('\\n', '\n'), sys.argv[3].replace('\\&', '&').replace('\\n', '\n')
s = open(p).read()
if a not in s:
    sys.exit(1)
open(p, 'w').write(s.replace(a, b, 1))
EOF
  out="$(forge test --root vault 2>&1)"
  if echo "$out" | grep -q "Compiler run failed\|Error (" ; then echo "BUILD FAIL  $name"; missed=$((missed+1)); continue; fi
  if echo "$out" | grep -q "FAIL"; then
    echo "caught      $name: $(echo "$out" | grep -m1 -o 'FAIL[^]]*\] test_[a-z_]*' | sed 's/.*\] //')"; caught=$((caught+1))
  else
    echo "MISSED      $name"; missed=$((missed+1))
  fi
done
restore
echo "$caught caught, $missed missed"
[ "$missed" = 0 ]

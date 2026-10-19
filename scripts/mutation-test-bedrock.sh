#!/usr/bin/env bash
# lez-bedrock's checks, each removed or weakened in turn, must fail its tests
# (bedrock/src/tests.rs: the real public-testnet anchor in vectors/bedrock and
# synthetic blocks; and the composition with lez-chain). The sources are
# restored at the end. The weak-key check before verify_strict is redundant
# with it (verify_strict refuses a small-order key itself), as it is in
# Bedrock's own preverify; it is mutated together with the strict check.
#   scripts/mutation-test-bedrock.sh
set -uo pipefail
cd "$(dirname "$0")/.."
L=bedrock/src/lib.rs
C=bedrock/src/compose.rs
SAVE="$(mktemp -d /tmp/rfp022-mut-bedrock.XXXXXX)"
for f in "$L" "$C"; do cp "$f" "$SAVE/"; done
restore() { for f in "$L" "$C"; do cp "$SAVE/$(basename "$f")" "$f"; done; }
trap restore EXIT

# name @@ file @@ literal text @@ replacement
MUTATIONS=(
  "header id: the slot not hashed@@$L@@[b\"BLOCK_ID_V1\", parent, slot, body,@@[b\"BLOCK_ID_V1\", parent, body,"
  "header id: the proof not hashed@@$L@@voucher, entropy, proof, leader]@@voucher, entropy, leader]"
  "header id: fields in canonical order, not the hashed one@@$L@@[b\"BLOCK_ID_V1\", parent, slot, body, voucher, entropy, proof, leader]@@[b\"BLOCK_ID_V1\", slot, parent, body, proof, entropy, leader, voucher]"
  "genesis slot accepted@@$L@@    if h.slot() == 0 {
        return Err(Error::GenesisSlot);
    }@@"
  "uncles: count not checked against the bytes@@$L@@ || rest.len() != *n as usize * SIGNED_HEADER_LEN {@@ {"
  "uncles: more than MAX_UNCLES accepted@@$L@@if *n as usize > MAX_UNCLES || @@if "
  "body root: compared with nothing@@$L@@if body_root(&a.uncles, &root) != h.body_root() {@@if false {"
  "body root: the uncles left out@@$L@@blake2b256(&[b\"BODY_ROOT_V1\", uncles, tx_root])@@blake2b256(\&[b\"BODY_ROOT_V1\", \&[0u8], tx_root])"
  "tx hash: no domain prefix@@$L@@blake2b256(&[b\"MANTLE_TXHASH_V1\", ops])@@blake2b256(\&[ops])"
  "merkle: path length not bounded@@$L@@if path.len() > MAX_PATH || @@if "
  "merkle: index range not checked@@$L@@ || (index as u64) >> path.len() != 0 {@@ {"
  "merkle: left and right swapped@@$L@@if (index >> k) & 1 == 0 { node(&h, s) } else { node(s, &h) }@@if (index >> k) \& 1 == 1 { node(\&h, s) } else { node(s, \&h) }"
  "inscription: any first opcode@@$L@@if count == 0 || *ops.get(1).ok_or(e)? != OP_CHANNEL_INSCRIBE {@@if count == 0 {"
  "inscription: a zero op count accepted@@$L@@if count == 0 || *ops.get(1)@@if *ops.get(1)"
  "inscription: length cap not checked@@$L@@    if len > MAX_INSCRIPTION {
        return Err(e);
    }@@"
  "inscription: trailing bytes accepted when alone@@$L@@    if count == 1 \&\& b.len() != 36 + len + 64 {
        return Err(e);
    }@@"
  "channel not checked@@$L@@    if ins.channel_id != p.channel_id {
        return Err(Error::Channel);
    }@@"
  "signer not checked@@$L@@    if ins.signer != p.signer {
        return Err(Error::Signer);
    }@@"
  "signature not verified@@$L@@    verify_signature(\&ins.signer, \&txh, sig)?;@@"
  "signature verified non-strictly@@$L@@key.verify_strict(tx_hash,@@ed25519_dalek::Verifier::verify(\&key, tx_hash,"
  "weak keys accepted and verified non-strictly@@$L@@    if key.is_weak() {
        return Err(Error::Signature);
    }
    key.verify_strict(tx_hash,@@    ed25519_dalek::Verifier::verify(\&key, tx_hash,"
  "descendant parent not checked@@$L@@        if d.parent() != top_id {
            return Err(Error::Parent);
        }@@"
  "descendant slot not checked@@$L@@        if d.slot() <= top_slot {
            return Err(Error::Slot);
        }@@"
  "depth not checked@@$L@@    if depth < min_depth {
        return Err(Error::Depth);
    }@@"
  "compose: inscribed block's hash not recomputed@@$C@@    if lez_chain::content_hash(lez) != h.hash {
        return Err(ComposeError::NotALezBlock);
    }@@"
  "compose: the inscription is not checked to be the tip@@$C@@    if h.block_id != state.tip_id || h.hash != state.tip_hash {
        return Err(ComposeError::NotTheTip);
    }@@"
  "compose: only the tip's id compared@@$C@@if h.block_id != state.tip_id || h.hash != state.tip_hash {@@if h.block_id != state.tip_id {"
  "compose: the anchor journal's tag not checked@@$C@@    if w(0) != ANCHOR_TAG {
        return None;
    }@@"
  "compose: the depth committed as the minimum@@$C@@wrap_core::word_u64(a.depth as u64),
    ] {
        out.extend_from_slice(\&w);
    }
    out.extend_from_slice(\&i.chain_journal);@@wrap_core::word_u64(0),
    ] {
        out.extend_from_slice(\&w);
    }
    out.extend_from_slice(\&i.chain_journal);"
  "compose: the release tail names the inscription block for the top@@$C@@        a.block_id,
        a.top_id,@@        a.block_id,
        a.block_id,"
)

restore
(cd bedrock && cargo test --features release >/dev/null 2>&1) || { echo "BASELINE FAIL: the unmutated sources do not pass the tests"; exit 1; }
echo "baseline    the unmutated sources pass ($(cd bedrock && cargo test --features release 2>&1 | grep -o '[0-9]* passed' | head -1))"

caught=0; missed=0
for m in "${MUTATIONS[@]}"; do
  name=${m%%@@*}; rest=${m#*@@}; file=${rest%%@@*}; rest=${rest#*@@}; from=${rest%%@@*}; to=${rest#*@@}
  restore
  python3 - "$file" "$from" "$to" <<'EOF' || { echo "SETUP FAIL  $name (text not found)"; missed=$((missed+1)); continue; }
import sys
p, a, b = sys.argv[1], sys.argv[2].replace('\\&', '&'), sys.argv[3].replace('\\&', '&')
s = open(p).read()
if a not in s:
    sys.exit(1)
open(p, 'w').write(s.replace(a, b, 1))
EOF
  out="$(cd bedrock && cargo test --features release 2>&1)"
  if echo "$out" | grep -q "could not compile" ; then echo "BUILD FAIL  $name"; missed=$((missed+1)); continue; fi
  if echo "$out" | grep -q "test result: FAILED"; then
    echo "caught      $name: $(echo "$out" | grep -m1 -o '^test [a-z_:]* \.\.\. FAILED' | sed 's/^test //; s/ \.\.\. FAILED//')"; caught=$((caught+1))
  else
    echo "MISSED      $name"; missed=$((missed+1))
  fi
done
restore
echo "$caught caught, $missed missed"
[ "$missed" = 0 ]

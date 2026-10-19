#!/usr/bin/env bash
# The Bedrock-anchored release, end to end, on the sequencer, the Bedrock
# devnet node and anvil that a KEEP=1 scripts/e2e-local.sh run with
# BEDROCK_NODE left running, after scripts/e2e-chain.sh on it:
#
#   0. Bedrock: the channel the sequencer inscribes on and its accredited key
#      (the node's /channel), and the Bedrock blocks from the node's tip back to
#      the one that inscribes the chain proof's tip block, read-only; every one
#      decoded with the node's own types and checked against lez-bedrock
#      (bedrock-tools check); the anchor: that block, the transaction's path,
#      and the MIN_DEPTH headers above it (bedrock-tools anchor), and the same
#      MIN_DEPTH - 1 deep.
#   1. Ethereum (anvil): a third vault, with the same pairing, whose verifier
#      is ZkBedrockBurnVerifier: pinned to the three guests, the bridge program
#      and its burn-tree account, the chain proof's anchor, the channel, the
#      sequencer's key and MIN_DEPTH; a 2-of-3 committee (anvil accounts 5, 6, 7)
#      attests Bedrock *headers* canonical.
#   2. The anchor proofs (zk/bedrock_anchor, the chain receipt verified inside):
#      the real one, the shallow one, a made-up Bedrock block inscribing the
#      made-up LEZ block of e2e-chain signed by the sequencer's key (a key this
#      local run made; nothing in a header proves it is Bedrock's), and the same
#      signed by another key.
#   3. The release proofs (zk/bedrock_release, the anchor receipt verified
#      inside; one Groth16 seal for the three guests): carol's burn, the same
#      over the shallow anchor, and the burn that never happened over both
#      made-up anchors.
#   4. The releases on anvil: refusals (a header the members' node does not
#      have, outsiders, one member, redirected, another amount, front-run,
#      journal tampered, another anchor, another channel, a chain-derived proof,
#      too shallow, the made-up Bedrock block signed by the committee and by
#      outsiders, another key), carol's release by the relayer, its replay.
#
# Usage: RUN=<the e2e run dir> BASE=<its manifest> BEDROCK_NODE=<URL> scripts/e2e-bedrock.sh
# Writes $OUT (default evidence/local-bedrock/e2e-bedrock.tsv) and the raw
# measurements next to it. Everything is local: anvil, a LEZ sequencer and a
# single-node Bedrock devnet whose proofs of leadership are in dev mode.
set -uo pipefail
cd "$(dirname "$0")/.."

: "${RUN:?the run directory of a KEEP=1 e2e-local.sh run}"
: "${BASE:?the manifest of that run}"
: "${BEDROCK_NODE:?the Bedrock node the sequencer inscribes on}"
CLI="${CLI:-cli/target/release/wrap}"
BT="${BT:-bedrock/tools/target/release/bedrock-tools}"
PORT="${PORT:-3051}"; ANVIL_PORT="${ANVIL_PORT:-8551}"
RPC="http://127.0.0.1:$PORT"; ANVIL="http://127.0.0.1:$ANVIL_PORT"
OUT="${OUT:-evidence/local-bedrock/e2e-bedrock.tsv}"
EVID="$(dirname "$OUT")"
HEAVY="${HEAVY:-}"; [ -x "$HEAVY" ] || HEAVY=""
MIN_DEPTH="${MIN_DEPTH:-5}"
export WRAP_RPC="$RPC" WRAP_RUN="$RUN/state" LEE_WALLET_HOME_DIR="$RUN/wallet"
CH="$RUN/chain"; BD="$RUN/bedrock"; mkdir -p "$EVID" "$BD"
TIME=(/usr/bin/time -l); [ "$(uname)" = Darwin ] || TIME=(/usr/bin/time -v)

jget() { python3 -c 'import json,sys
v=json.loads(sys.argv[1])
for k in sys.argv[2].split("."): v=(v or {}).get(k) if isinstance(v, dict) else None
print("" if v is None else v)' "$1" "$2"; }
fget() { python3 -c 'import json,sys
v=json.load(open(sys.argv[1]))
for k in sys.argv[2].split("."): v=(v or {}).get(k) if isinstance(v, dict) else None
print("" if v is None else v)' "$1" "$2"; }
fail=0
check() {
  if [ "$2" = "$3" ]; then echo "  ok  $1: $2"; echo "# check ok: $1 = $2" >> "$OUT"
  else echo "  FAIL $1: got $2, want $3"; echo "# check FAIL: $1: $2 != $3" >> "$OUT"; fail=1; fi
}
DEP2="$RUN/state/anvil/deployment-chain.json"
CHAIN_REC="${CHAIN_REC:-$CH/chain-2.json}"
[ -f "$DEP2" ] && [ -f "$CHAIN_REC" ] && [ -f "$BASE" ] || { echo "no e2e-chain run at $RUN"; exit 2; }
curl -s -m 5 "$BEDROCK_NODE/cryptarchia/info" | grep -q '"tip"' || { echo "no Bedrock node at $BEDROCK_NODE"; exit 2; }
{
  echo "# the Bedrock-anchored release on the base run $(head -1 "$BASE" | cut -c3-)"
  echo "# commit $(git rev-parse --short HEAD)$(git diff --quiet -- . ':!evidence' || echo ' (uncommitted changes)'), $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "# bedrock node $BEDROCK_NODE $(curl -s "$BEDROCK_NODE/cryptarchia/info")"
} > "$OUT"

echo "== 0. Bedrock: the channel, its key, the blocks down to the chain tip's inscription"
CHANNEL="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["bedrock_config"]["channel_id"])' "$RUN/seq/sequencer_config.json")"
CHAN_STATE="$(curl -s "$BEDROCK_NODE/channel/$CHANNEL")"
echo "# bedrock-channel $CHANNEL $CHAN_STATE" >> "$OUT"
KEYS="$(jget "$CHAN_STATE" accredited_keys)"
SEQ_KEY="$(python3 -c 'import json,sys;k=json.loads(sys.argv[1])["accredited_keys"];assert len(k)==1,k;print(k[0])' "$CHAN_STATE")" || { echo "the channel has not one accredited key: $KEYS"; exit 1; }
TIP_HASH="$(fget "$CHAIN_REC" tip.hash)"; TIP_ID="$(fget "$CHAIN_REC" tip.id)"
echo "  channel $CHANNEL, accredited key $SEQ_KEY; the chain proof ends at LEZ block $TIP_ID $TIP_HASH"
# Wait until MIN_DEPTH Bedrock blocks are above the inscription (the walk back finds it, then counts).
for _ in $(seq 120); do
  rm -rf "$BD/blocks"
  F="$(python3 scripts/bedrock_fetch.py --node "$BEDROCK_NODE" --out "$BD/blocks" --count 20000 --until-lez-hash "$TIP_HASH" --channel "$CHANNEL")" || { echo "fetch failed"; exit 1; }
  [ "$(jget "$F" blocks)" -gt "$MIN_DEPTH" ] && break
  sleep 5
done
echo "  $F"; echo "# bedrock-fetch $F" >> "$OUT"
C="$("$BT" check "$BD/blocks")" || { echo "bedrock-tools check failed"; exit 1; }
echo "  $C"; echo "# bedrock-check $C" >> "$OUT"
check "the node's types and lez-bedrock agree on every block fetched" "$(jget "$C" identical)" True
A="$("$BT" anchor "$BD/blocks" --channel "$CHANNEL" --signer "$SEQ_KEY" --lez-hash "$TIP_HASH" --depth "$MIN_DEPTH" --out "$BD/anchor-block.json")" || { echo "bedrock-tools anchor failed"; exit 1; }
echo "  $A"; echo "# bedrock-anchor $A" >> "$OUT"
S="$("$BT" anchor "$BD/blocks" --channel "$CHANNEL" --signer "$SEQ_KEY" --lez-hash "$TIP_HASH" --depth $((MIN_DEPTH - 1)) --out "$BD/anchor-block-shallow.json")" || { echo "shallow anchor failed"; exit 1; }
echo "# bedrock-anchor-shallow $S" >> "$OUT"
check "the inscribed LEZ block is the chain proof's tip" "$(fget "$BD/anchor-block.json" lez_block_id)" "$TIP_ID"
TOP="$(fget "$BD/anchor-block.json" top.id)"
cp "$BD/anchor-block.json" "$EVID/anchor-block.json"

echo "== 1. Ethereum (anvil): a vault with ZkBedrockBurnVerifier, the same pairing"
DEPLOYER_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 # anvil's well-known account 0
ALICE_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d   # anvil's well-known account 1
create() { forge create --root vault --rpc-url "$ANVIL" --private-key "$DEPLOYER_KEY" --broadcast "$@" 2>&1 | awk '/^Deployed to:/ {print $3}'; }
DV() { fget "$DEP2" "$1"; }
COMMITTEE="[$(python3 -c 'import json,sys;print(",".join(json.load(open(sys.argv[1]))["committee"]))' "$DEP2")]"
IMG="$("$CLI" image-id)"
ANCHOR_IMAGE="0x$(jget "$IMG" bedrock_anchor.image_id)"; RELEASE_IMAGE="0x$(jget "$IMG" bedrock_release.image_id)"
CHAIN_IMAGE="$(DV chain_image_id)"
for g in bedrock_anchor bedrock_release; do
  echo "# binary $g $(jget "$IMG" $g.bytes) bytes sha256 $(jget "$IMG" $g.sha256) image_id 0x$(jget "$IMG" $g.image_id)" >> "$OUT"
done
CFG="($(DV risc0_verifier),$RELEASE_IMAGE,$CHAIN_IMAGE,$ANCHOR_IMAGE,$(DV program_id),$(DV burns_account),$(DV anchor_id),$(DV anchor_hash),0x$CHANNEL,0x$SEQ_KEY,$MIN_DEPTH)"
VERIFIER="$(create src/ZkBedrockBurnVerifier.sol:ZkBedrockBurnVerifier --constructor-args "$CFG" "$COMMITTEE" 2)"
DEPLOYER="$(cast wallet address "$DEPLOYER_KEY")"
VAULT="$(create src/WrapVault.sol:WrapVault --constructor-args "$(DV pairing)" "$VERIFIER" 86400 "$DEPLOYER" "$DEPLOYER")"
[ -n "$VERIFIER" ] && [ -n "$VAULT" ] || { echo "deployment failed"; exit 1; }
cast send --private-key "$DEPLOYER_KEY" --rpc-url "$ANVIL" "$VAULT" "registerToken(address,uint8,uint256[],uint128,uint128)" \
  0x0000000000000000000000000000000000000000 18 "[100000000000000000,1000000000000000000]" 100000000000000000000 100000000000000000000 > /dev/null
cast send --private-key "$ALICE_KEY" --rpc-url "$ANVIL" --value 1000000000000000000 "$VAULT" "deposit(address,uint256,bytes32)" \
  0x0000000000000000000000000000000000000000 1000000000000000000 "0x$(openssl rand -hex 32)" > /dev/null
DEP3="$RUN/state/anvil/deployment-bedrock.json"
python3 - "$DEP2" "$DEP3" "$VAULT" "$VERIFIER" "$ANCHOR_IMAGE" "$RELEASE_IMAGE" "$CHANNEL" "$SEQ_KEY" "$MIN_DEPTH" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
d.update(vault=sys.argv[3], verifier=sys.argv[4], release_verifier="zkbedrock", anchor_image_id=sys.argv[5], image_id=sys.argv[6],
         channel_id=sys.argv[7], sequencer_key=sys.argv[8], min_depth=int(sys.argv[9]))
json.dump(d, open(sys.argv[2], "w"), indent=1)
PY
cp "$DEP3" "$EVID/deployment-bedrock.json"
echo "  vault $VAULT, verifier $VERIFIER"
echo "# anvil vault $VAULT verifier $VERIFIER pairing $(DV pairing) channel $CHANNEL sequencer_key $SEQ_KEY min_depth $MIN_DEPTH" >> "$OUT"
for k in IMAGE_ID:$RELEASE_IMAGE CHAIN_IMAGE_ID:$CHAIN_IMAGE ANCHOR_IMAGE_ID:$ANCHOR_IMAGE CHANNEL_ID:0x$CHANNEL SEQUENCER_KEY:0x$SEQ_KEY ANCHOR_HASH:$(DV anchor_hash); do
  check "ZkBedrockBurnVerifier.${k%%:*}" "$(cast call "$VERIFIER" "${k%%:*}()(bytes32)" --rpc-url "$ANVIL")" "${k#*:}"
done
check "ZkBedrockBurnVerifier.MIN_DEPTH" "$(cast call "$VERIFIER" 'MIN_DEPTH()(uint32)' --rpc-url "$ANVIL")" "$MIN_DEPTH"

measure() { # name -- wrap args: run under heavy and time(1), keep the raw output
  local name="$1"; shift 2
  echo "# $name load before: $(uptime | sed 's/.*load/load/')" >> "$OUT"
  ${HEAVY:+"$HEAVY"} "${TIME[@]}" "$CLI" "$@" > "$EVID/$name.txt" 2>&1
  local rc=$?
  grep '^{' "$EVID/$name.txt" | tail -1
  return $rc
}
echo "== 2. the anchor proofs"
measure anchor-prove -- anchor-prove --chain "$CHAIN_REC" --block "$BD/anchor-block.json" --out "$BD/anchor.json" > /dev/null || { echo "anchor proof failed"; tail -20 "$EVID/anchor-prove.txt"; exit 1; }
measure anchor-prove-shallow -- anchor-prove --chain "$CHAIN_REC" --block "$BD/anchor-block-shallow.json" --out "$BD/anchor-shallow.json" > /dev/null || { echo "shallow anchor proof failed"; tail -20 "$EVID/anchor-prove-shallow.txt"; exit 1; }
# The made-up LEZ block of e2e-chain, in a made-up Bedrock block signed by the sequencer's key, and by another key.
FORGED_BLOCK="$(ls "$CH/forged-blocks/"*.borsh | head -1)"
FB="$("$BT" forge --key "$RUN/seq/bedrock_signing_key" --lez-block "$FORGED_BLOCK" --channel "$CHANNEL" --depth "$MIN_DEPTH" --out "$BD/forged-block.json")" || { echo "forge failed"; exit 1; }
echo "# bedrock-forge $FB" >> "$OUT"
check "the made-up Bedrock block is signed by the channel's accredited key" "$(jget "$FB" signer)" "$SEQ_KEY"
openssl rand 32 > "$BD/other.key"
FO="$("$BT" forge --key "$BD/other.key" --lez-block "$FORGED_BLOCK" --channel "$CHANNEL" --depth "$MIN_DEPTH" --out "$BD/forged-block-other-key.json")" || { echo "forge failed"; exit 1; }
echo "# bedrock-forge-other-key $FO" >> "$OUT"
measure anchor-prove-forged -- anchor-prove --chain "$CH/chain-forged.json" --block "$BD/forged-block.json" --out "$BD/anchor-forged.json" > /dev/null || { echo "forged anchor proof failed"; tail -20 "$EVID/anchor-prove-forged.txt"; exit 1; }
measure anchor-prove-forged-other-key -- anchor-prove --chain "$CH/chain-forged.json" --block "$BD/forged-block-other-key.json" --out "$BD/anchor-forged-other-key.json" > /dev/null || { echo "forged anchor proof failed"; tail -20 "$EVID/anchor-prove-forged-other-key.txt"; exit 1; }
python3 - "$BD" "$EVID" >> "$OUT" <<'PY'
import json, re, sys
bd, evid = sys.argv[1:3]
for n, raw in (("anchor", "anchor-prove"), ("anchor-shallow", "anchor-prove-shallow"), ("anchor-forged", "anchor-prove-forged"), ("anchor-forged-other-key", "anchor-prove-forged-other-key")):
    r, t = json.load(open(f"{bd}/{n}.json")), open(f"{evid}/{raw}.txt").read()
    rss = re.search(r"(\d+)\s+maximum resident set size", t) or re.search(r"Maximum resident set size \(kbytes\): (\d+)", t)
    wall = re.search(r"([\d.]+) real", t)
    s, i = r["stats"], r["input"]
    print(f"# anchor-proof {n} bedrock_block {r['bedrock_block']['id']} slot {r['bedrock_block']['slot']} top {r['top']['id']} depth {r['top']['depth']} "
          f"lez_tip {r['tip']['id']} {r['tip']['hash']} signer {r['signer']} ops_bytes {i['ops_bytes']} path_len {i['path_len']} descendants {i['descendants']} "
          f"user_cycles {s['user_cycles']} total_cycles {s['total_cycles']} paging_cycles {s['paging_cycles']} segments {s['segments']} "
          f"stark_succinct_ms {s['stark_succinct_ms']} wall_s {wall.group(1) if wall else '?'} host_peak_rss_bytes {rss.group(1) if rss else '?'} "
          f"image_id {r['image_id']} dev_mode {r['dev_mode']}")
PY
check "the four anchor proofs are real (RISC0 dev mode off)" "$(grep -c '^# anchor-proof .* dev_mode False$' "$OUT")" 4
check "the anchor proof's top is the anchor's" "$(fget "$BD/anchor.json" top.id | sed 's/^0x//') $(fget "$BD/anchor.json" top.depth)" "$TOP $MIN_DEPTH"

echo "== 3. the release proofs"
RELAYER="$(DV relayer)"
zkp() { # out raw -- wrap args (zk-prove-measured.sh: heavy, time(1), the Groth16 container's memory)
  local o="$1" raw="$2"; shift 3
  echo "# zk-prove load before: $(uptime | sed 's/.*load/load/')" >> "$OUT"
  CLI="$CLI" scripts/zk-prove-measured.sh "$o" "$raw" -- "$@" > "$BD/zk-prove.log" 2>&1 || { echo "prove failed"; cat "$BD/zk-prove.log"; tail -20 "$raw"; exit 1; }
}
zkp "$BD/proof-carol.json" "$EVID/zk-bedrock-prove-carol.txt" -- release-proof carol --zk --chain "$CHAIN_REC" --bedrock "$BD/anchor.json" --relayer "$RELAYER" --vault "$VAULT"
zkp "$BD/proof-carol-shallow.json" "$EVID/zk-bedrock-prove-carol-shallow.txt" -- release-proof carol --zk --chain "$CHAIN_REC" --bedrock "$BD/anchor-shallow.json" --relayer "$RELAYER" --vault "$VAULT"
FORGED_IN="$BD/forged-input.json"
python3 - "$CH/forged-input.json" "$VAULT" > "$FORGED_IN" <<'PY'
import json, sys
i = json.load(open(sys.argv[1])); i["vault"] = sys.argv[2]; json.dump(i, sys.stdout)
PY
zkp "$BD/proof-forged.json" "$EVID/zk-bedrock-prove-forged.txt" -- zk-prove "$FORGED_IN" --bedrock "$BD/anchor-forged.json"
zkp "$BD/proof-forged-other-key.json" "$EVID/zk-bedrock-prove-forged-other-key.txt" -- zk-prove "$FORGED_IN" --bedrock "$BD/anchor-forged-other-key.json"
python3 - "$BD" "$EVID" >> "$OUT" <<'PY'
import json, re, sys
bd, evid = sys.argv[1:3]
for who in ("carol", "carol-shallow", "forged", "forged-other-key"):
    p = json.load(open(f"{bd}/proof-{who}.json"))
    z = p.get("zk", p)
    raw = open(f"{evid}/zk-bedrock-prove-{who}.txt").read()
    rss = re.search(r"(\d+)\s+maximum resident set size", raw) or re.search(r"Maximum resident set size \(kbytes\): (\d+)", raw)
    peak = re.search(r"container peak memory \(docker stats samples\) ([\d.]+) GiB", raw)
    wall = re.search(r"([\d.]+) real", raw)
    s, f = z["stats"], z["fields"]
    print(f"# zk-bedrock-proof {who} image_id {z['image_id']} journal_bytes {z['journal_bytes']} journal_digest {z['journal_digest']} release_digest {z['release_digest']} "
          f"root {f['root']} nullifier {f['nullifier']} tip {int(f['tip_id'], 16)} {f['tip_hash']} anchor_image_id {f['anchor_image_id']} channel {f['channel_id']} "
          f"signer {f['signer']} bedrock_block {f['bedrock_block_id']} top {f['top_header_id']} depth {int(f['depth'], 16)} "
          f"seal_bytes {z['seal_bytes']} user_cycles {s['user_cycles']} total_cycles {s['total_cycles']} stark_succinct_ms {s['stark_succinct_ms']} groth16_ms {s['groth16_ms']} "
          f"wall_s {wall.group(1) if wall else '?'} host_peak_rss_bytes {rss.group(1) if rss else '?'} groth16_container_peak_gib {peak.group(1) if peak else '?'} "
          f"dev_mode {z['dev_mode']} platform {z['docker_default_platform']}")
PY
check "the four release proofs are real (RISC0 dev mode off)" "$(grep -c '^# zk-bedrock-proof .* dev_mode False ' "$OUT")" 4
check "carol's proof names the anchor guest, the channel, the key, the top header" \
  "$(python3 -c 'import json,sys;f=json.load(open(sys.argv[1]))["zk"]["fields"];print(f["anchor_image_id"],f["channel_id"][2:],f["signer"][2:],f["top_header_id"][2:],int(f["depth"],16))' "$BD/proof-carol.json")" \
  "$ANCHOR_IMAGE $CHANNEL $SEQ_KEY $TOP $MIN_DEPTH"
check "and the chain's tip" "$(python3 -c 'import json,sys;f=json.load(open(sys.argv[1]))["zk"]["fields"];print(int(f["tip_id"],16),f["tip_hash"])' "$BD/proof-carol.json")" "$TIP_ID $TIP_HASH"
check "the forged proof is signed by the sequencer's key, over a Bedrock block no node has" "$(fget "$BD/proof-forged.json" fields.signer | cut -c3-)" "$SEQ_KEY"
python3 - "$BD/proof-carol.json" vault/test/fixtures/zk_bedrock_release.json "$BD/anchor-block.json" <<'PY'
import json, sys
z = json.load(open(sys.argv[1]))["zk"]
a = json.load(open(sys.argv[3]))
out = {k: z[k] for k in ("image_id", "journal", "journal_digest", "release_digest", "fields", "seal", "seal_bytes", "stats", "dev_mode", "docker_default_platform")}
out["bedrock"] = {"bedrock_block": a["bedrock_block"], "top": a["top"]}
json.dump(out, open(sys.argv[2], "w"), indent=1)
PY

echo "== 4. the releases (ZkBedrockBurnVerifier)"
wei() { cast balance "$1" --rpc-url "$ANVIL"; }
CAROL_ETH="$(DV carol)"; VAULT0="$(wei "$VAULT")"; CAROL0="$(wei "$CAROL_ETH")"
rel() {
  local label="$1" expect="$2"; shift 3
  local out
  out="$(python3 scripts/anvil_release.py --anvil "$ANVIL" --lez "$RPC" --deployment "$DEP3" --out "$OUT" --label "$label" --expect "$expect" --zkbedrock --bedrock-node "$BEDROCK_NODE" --gas-limit 900000 "$@" 2>&1 | tail -1)"
  echo "$out" | cut -c1-400
  if [ "$(jget "$out" ok)" = True ]; then echo "  ok  $label: $(jget "$out" verdict) ($(jget "$out" precheck)), block $(jget "$out" block), gas $(jget "$out" gas_used)"
  else echo "  FAIL $label: $out"; fail=1; fi
}
P="$BD/proof-carol.json"
rel release-carol-outsiders HeaderNotAttested -- --proof "$P" --sender relayer --outsiders
rel release-carol-one-member HeaderNotAttested -- --proof "$P" --sender relayer --members 1
rel release-carol-redirected-to-relayer NotThisRelease -- --proof "$P" --sender relayer --recipient-as-sender
rel release-carol-another-amount NotThisRelease -- --proof "$P" --sender relayer --amount 100000000000000000
rel release-carol-front-run NotThisRelease -- --proof "$P" --sender outsider
rel release-carol-tampered-journal VerificationFailed -- --proof "$P" --sender relayer --tamper-journal
rel release-carol-another-anchor NotThisChain -- --proof "$P" --sender relayer --tamper-anchor
rel release-carol-another-channel NotThisChannel -- --proof "$P" --sender relayer --tamper-channel
rel release-carol-chain-proof-only NotThisRelease -- --proof "$P" --chain-proof "$CH/proof-carol.json" --sender relayer
rel release-carol-too-shallow NotDeepEnough -- --proof "$BD/proof-carol-shallow.json" --sender relayer
rel release-forged-bedrock-committee HeaderNotAttested -- --proof "$P" --forged "$BD/proof-forged.json" --sender outsider
rel release-forged-bedrock-outsiders HeaderNotAttested -- --proof "$P" --forged "$BD/proof-forged.json" --sender outsider --outsiders
rel release-forged-bedrock-other-key NotThisChannel -- --proof "$P" --forged "$BD/proof-forged-other-key.json" --sender outsider --outsiders
rel release-carol applied -- --proof "$P" --sender relayer
rel release-carol-replay AlreadyReleased -- --proof "$P" --sender relayer
VAULT1="$(wei "$VAULT")"; CAROL1="$(wei "$CAROL_ETH")"
check "the Bedrock vault paid out exactly carol's release (1 ETH)" "$(python3 -c "print($VAULT0 - $VAULT1)")" 1000000000000000000
check "carol received 1 ETH less the 0.01 ETH fee her burn fixed" "$(python3 -c "print($CAROL1 - $CAROL0)")" 990000000000000000
NF="$(fget "$P" zk.fields.nullifier)"
check "the vault recorded the release's nullifier" "$(cast call "$VAULT" 'nullifierSpent(bytes32)(bool)' "$NF" --rpc-url "$ANVIL")" true
check "the members found carol's top header on their Bedrock node" "$(awk '/^# eth-header-attestation release-carol / {print $7}' "$OUT")" True
check "and did not find the made-up one" "$(awk '/^# eth-header-attestation release-forged-bedrock-committee / {print $7}' "$OUT")" False
check "the release sent no leaf, secret, Ethereum commitment or path node" \
  "$(awk '/^# eth-zk release-carol / {if ($7=="True" && $9=="True" && $11=="True") { split($13, p, "/"); if (p[1]==p[2]) print "hidden" }}' "$OUT" | sort -u)" hidden
python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));json.dump({k: d[k] for k in ("op","owner","token","amount","recipient","fee","relayer","chain_derived","bedrock","zk") if k in d}, open(sys.argv[2],"w"), indent=1)' "$P" "$EVID/proof-carol.json"
cp "$BD/anchor.json" "$EVID/anchor.json"
eth_applied=$(awk '/^# eth / && $4=="applied" && $5=="applied"' "$OUT" | wc -l | tr -d ' ')
eth_refused=$(awk '/^# eth / && $4!="applied" && $5=="reverted"' "$OUT" | wc -l | tr -d ' ')
echo "== $( [ $fail = 0 ] && echo PASS || echo FAIL ): $((eth_applied + eth_refused)) Bedrock-anchored releases ($eth_applied applied, $eth_refused refused as expected), $(grep -c '^# check ok' "$OUT") checks ok"
exit $fail

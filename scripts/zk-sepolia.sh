#!/usr/bin/env bash
# The zero-knowledge release verified on Sepolia: carol's private burn on a
# local LEZ v0.3.1 sequencer that inscribes every block on a local Bedrock
# devnet, released by the vault deployed on the public Sepolia testnet
# (`sepolia_vault.py deploy-zk`), whose verifier is ZkBedrockBurnVerifier.
# The public LEZ testnet halted on 2026-11-17 (tip 83005, its inscriptions on
# Bedrock stopped), so LEZ and Bedrock are local here; Sepolia is public.
#
# Run after scripts/testnet-inbound.sh (MODE=local, BEDROCK_NODE set: deposits
# into the Sepolia vault, Sextant Init from a Sepolia checkpoint, the claims)
# and scripts/testnet-outbound.sh burn (PRIVATE=1) on the same RUN:
#
#   1. LEZ: the chain guest (zk/lez_chain) from the anchor the verifier pins
#      (a block before the bridge's Init) to a tip after carol's burn, in two
#      recursive steps; the burn tree it rebuilds holds carol's leaf.
#   2. Bedrock: the blocks from the devnet's tip back to the one inscribing
#      that tip, decoded with the node's own types (bedrock-tools check), and
#      the anchor: that block, the transaction's Merkle path and MIN_DEPTH
#      headers above it (bedrock-tools anchor); the anchor guest proves it.
#   3. The release guest (zk/bedrock_release), one Groth16 seal for the three.
#   4. Sepolia: refusals (one member: HeaderNotAttested; the journal
#      tampered: VerificationFailed), carol's
#      release by the relayer, its replay (AlreadyReleased). Each pre-checked
#      with eth_call, sent with an explicit gas limit, mined, recorded.
#
#   RUN=<dir> BEDROCK_NODE=<URL> RPC=<sequencer URL> SEPOLIA_KEY_FILE=<key> scripts/zk-sepolia.sh
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2
: "${RUN:?}" "${BEDROCK_NODE:?}" "${RPC:?}" "${SEPOLIA_KEY_FILE:?}"
CLI="${CLI:-cli/target/release/wrap}"
BT="${BT:-bedrock/tools/target/release/bedrock-tools}"
EVID="${EVID:-evidence/zk-sepolia}"
OUT="$EVID/zk-sepolia.tsv"
HEAVY="${HEAVY:-}"; [ -x "$HEAVY" ] || HEAVY=""
export WRAP_RPC="$RPC" WRAP_RUN="$RUN/state" WRAP_ARTIFACTS="$RUN/artifacts" LEE_WALLET_HOME_DIR="${LEE_WALLET_HOME_DIR:-$RUN/wallet}"
DEP="$RUN/state/anvil/deployment.json"
CH="$RUN/chain"; BD="$RUN/bedrock"; mkdir -p "$EVID" "$CH" "$BD"
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
measure() { local name="$1"; shift 2
  echo "# $name load before: $(uptime | sed 's/.*load/load/')" >> "$OUT"
  ${HEAVY:+"$HEAVY"} "${TIME[@]}" "$CLI" "$@" > "$EVID/$name.txt" 2>&1; local rc=$?
  grep '^{' "$EVID/$name.txt" | tail -1; return $rc; }
tip() { curl -s -X POST "$RPC" -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getLastBlockId","params":[]}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["result"])'; }
MIN_DEPTH="$(fget "$DEP" min_depth)"; CHANNEL="$(fget "$DEP" channel_id)"; SEQ_KEY="$(fget "$DEP" sequencer_key)"
# a deployment pinning the channel's key set (sepolia_vault.py deploy-zk today) records channel_keys: one devnet key
[ -n "$SEQ_KEY" ] || SEQ_KEY="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["channel_keys"][0])' "$DEP")"
ANCHOR="$(fget "$DEP" anchor_id)"; VAULT="$(fget "$DEP" vault)"; RELAYER="$(fget "$DEP" relayer)"
BURN_BLOCK="$(awk -F'\t' '$1=="burn-carol-private" {print $4}' "$EVID/outbound.tsv")"
[ -n "$BURN_BLOCK" ] || { echo "no burn-carol-private in $EVID/outbound.tsv"; exit 2; }
# A re-run keeps the Sepolia attempts already made (they are on chain) and redoes the rest of the record.
KEPT="$(mktemp)"; [ -f "$OUT" ] && grep -E '^# eth(-header-attestation|-zk)? ' "$OUT" > "$KEPT"
{
  echo "# the zero-knowledge release verified on Sepolia (chain 11155111): LEZ v0.3.1 sequencer $RPC and Bedrock devnet $BEDROCK_NODE (local), $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "# commit $(git rev-parse --short HEAD)$([ -z "$(git status --porcelain -- . ':!evidence')" ] || echo ' (uncommitted changes)')"
  echo "# sepolia vault $VAULT verifier $(fget "$DEP" verifier) risc0 $(fget "$DEP" risc0_verifier) pairing $(fget "$DEP" pairing) anchor $ANCHOR $(fget "$DEP" anchor_hash) channel $CHANNEL sequencer_key $SEQ_KEY min_depth $MIN_DEPTH"
  echo "# bedrock node $(curl -s "$BEDROCK_NODE/cryptarchia/info")"
  echo "# bedrock-channel $CHANNEL $(curl -s "$BEDROCK_NODE/channel/$CHANNEL")"
} > "$OUT"
check "the channel has one accredited key, the one the verifier pins" "$(python3 -c 'import json,sys;print(",".join(json.loads(sys.argv[1])["accredited_keys"]))' "$(curl -s "$BEDROCK_NODE/channel/$CHANNEL")")" "$SEQ_KEY"

echo "== 1. LEZ: the chain guest from block $ANCHOR (the bridge's Init is after it) past carol's burn (block $BURN_BLOCK)"
if [ ! -f "$CH/chain-1.json" ]; then
  S1="$(measure chain-prove-1 -- chain-prove --anchor "$ANCHOR" --to $((BURN_BLOCK - 1)) --out "$CH/chain-1.json" --save-blocks "$CH/blocks")" || { echo "chain step 1 failed"; tail -20 "$EVID/chain-prove-1.txt"; exit 1; }
fi
if [ ! -f "$CH/chain-2.json" ]; then
  T="$(tip)"
  S2="$(measure chain-prove-2 -- chain-prove --prior "$CH/chain-1.json" --to "$T" --out "$CH/chain-2.json" --save-blocks "$CH/blocks")" || { echo "chain step 2 failed"; tail -20 "$EVID/chain-prove-2.txt"; exit 1; }
fi
for n in 1 2; do
  python3 - "$CH/chain-$n.json" "$EVID/chain-prove-$n.txt" "$n" >> "$OUT" <<'PY'
import json, re, sys
r, raw, n = json.load(open(sys.argv[1])), open(sys.argv[2]).read(), sys.argv[3]
rss = re.search(r"(\d+)\s+maximum resident set size", raw); wall = re.search(r"([\d.]+) real", raw)
s = r["stats"]
print(f"# chain-proof {n} from {r['from']} tip {r['tip']['id']} {r['tip']['hash']} anchor {r['anchor']['id']} {r['anchor']['hash']} blocks {r['blocks']} block_bytes {r['block_bytes']} "
      f"prior {r['prior']} root {r['root']} next_index {r['next_index']} user_cycles {s['user_cycles']} total_cycles {s['total_cycles']} segments {s['segments']} "
      f"stark_succinct_ms {s['stark_succinct_ms']} wall_s {wall.group(1) if wall else '?'} host_peak_rss_bytes {rss.group(1) if rss else '?'} image_id {r['image_id']} dev_mode {r['dev_mode']}")
for l in r["leaves"]:
    print(f"# chain-leaf {n} index {l['index']} block {l['block']} tx {l['tx_index']} leaf {l['leaf']}")
PY
done
check "the second step continues the first (its prior)" "$(fget "$CH/chain-2.json" prior)" True
check "both chain proofs are real (RISC0 dev mode off)" "$(fget "$CH/chain-1.json" dev_mode) $(fget "$CH/chain-2.json" dev_mode)" "False False"
check "the chain proof starts at the anchor the verifier pins" "$(fget "$CH/chain-2.json" anchor.hash)" "$(fget "$DEP" anchor_hash)"
TIP_HASH="$(fget "$CH/chain-2.json" tip.hash)"; TIP_ID="$(fget "$CH/chain-2.json" tip.id)"

echo "== 2. Bedrock: the blocks down to the inscription of LEZ block $TIP_ID, and the anchor"
for _ in $(seq 120); do
  rm -rf "$BD/blocks"
  F="$(python3 scripts/bedrock_fetch.py --node "$BEDROCK_NODE" --out "$BD/blocks" --count 20000 --until-lez-hash "$TIP_HASH" --channel "$CHANNEL")" || { echo "fetch failed"; exit 1; }
  [ "$(jget "$F" blocks)" -gt "$MIN_DEPTH" ] && break
  sleep 5
done
echo "# bedrock-fetch $F" >> "$OUT"
C="$("$BT" check "$BD/blocks")" || { echo "bedrock-tools check failed"; exit 1; }
echo "# bedrock-check $C" >> "$OUT"
check "the node's types and lez-bedrock agree on every block fetched" "$(jget "$C" identical)" True
A="$("$BT" anchor "$BD/blocks" --channel "$CHANNEL" --signer "$SEQ_KEY" --lez-hash "$TIP_HASH" --depth "$MIN_DEPTH" --out "$BD/anchor-block.json")" || { echo "bedrock-tools anchor failed"; exit 1; }
echo "# bedrock-anchor $A" >> "$OUT"
check "the inscribed LEZ block is the chain proof's tip" "$(fget "$BD/anchor-block.json" lez_block_id)" "$TIP_ID"
cp "$BD/anchor-block.json" "$EVID/anchor-block.json"
[ -f "$BD/anchor.json" ] || measure anchor-prove -- anchor-prove --chain "$CH/chain-2.json" --block "$BD/anchor-block.json" --out "$BD/anchor.json" > /dev/null \
  || { echo "anchor proof failed"; tail -20 "$EVID/anchor-prove.txt"; exit 1; }
python3 - "$BD/anchor.json" "$EVID/anchor-prove.txt" >> "$OUT" <<'PY'
import json, re, sys
r, t = json.load(open(sys.argv[1])), open(sys.argv[2]).read()
rss = re.search(r"(\d+)\s+maximum resident set size", t); wall = re.search(r"([\d.]+) real", t); s, i = r["stats"], r["input"]
print(f"# anchor-proof bedrock_block {r['bedrock_block']['id']} slot {r['bedrock_block']['slot']} top {r['top']['id']} depth {r['top']['depth']} lez_tip {r['tip']['id']} {r['tip']['hash']} "
      f"signer {r['signer']} ops_bytes {i['ops_bytes']} path_len {i['path_len']} descendants {i['descendants']} user_cycles {s['user_cycles']} total_cycles {s['total_cycles']} "
      f"segments {s['segments']} stark_succinct_ms {s['stark_succinct_ms']} wall_s {wall.group(1) if wall else '?'} host_peak_rss_bytes {rss.group(1) if rss else '?'} image_id {r['image_id']} dev_mode {r['dev_mode']}")
PY
check "the anchor proof is real (RISC0 dev mode off)" "$(fget "$BD/anchor.json" dev_mode)" False

echo "== 3. the release proof (one Groth16 seal for the three guests)"
if [ ! -f "$BD/proof-carol.json" ]; then
  CLI="$CLI" scripts/zk-prove-measured.sh "$BD/proof-carol.json" "$EVID/zk-prove-carol.txt" -- release-proof carol --zk --chain "$CH/chain-2.json" \
    --bedrock "$BD/anchor.json" --relayer "$RELAYER" --vault "$VAULT" > "$BD/zk-prove.log" 2>&1 || { echo "prove failed"; tail -20 "$BD/zk-prove.log"; exit 1; }
fi
python3 - "$BD/proof-carol.json" "$EVID/zk-prove-carol.txt" >> "$OUT" <<'PY'
import json, re, sys
z = json.load(open(sys.argv[1]))["zk"]; raw = open(sys.argv[2]).read()
peak = re.search(r"container peak memory \(docker stats samples\) ([\d.]+) GiB", raw); wall = re.search(r"([\d.]+) real", raw); s, f = z["stats"], z["fields"]
print(f"# zk-bedrock-proof carol image_id {z['image_id']} journal_bytes {z['journal_bytes']} journal_digest {z['journal_digest']} release_digest {z['release_digest']} "
      f"root {f['root']} nullifier {f['nullifier']} tip {int(f['tip_id'], 16)} {f['tip_hash']} channel {f['channel_id']} signer {f['signer']} bedrock_block {f['bedrock_block_id']} "
      f"top {f['top_header_id']} depth {int(f['depth'], 16)} seal_bytes {z['seal_bytes']} user_cycles {s['user_cycles']} total_cycles {s['total_cycles']} "
      f"stark_succinct_ms {s['stark_succinct_ms']} groth16_ms {s['groth16_ms']} wall_s {wall.group(1) if wall else '?'} groth16_container_peak_gib {peak.group(1) if peak else '?'} "
      f"dev_mode {z['dev_mode']} platform {z['docker_default_platform']}")
PY
check "the release proof is real (RISC0 dev mode off)" "$(fget "$BD/proof-carol.json" zk.dev_mode)" False
check "it names Sepolia's chain id" "$(python3 -c 'import json,sys;print(int(json.load(open(sys.argv[1]))["zk"]["fields"]["chain_id"],16))' "$BD/proof-carol.json")" 11155111
python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));json.dump({k: d[k] for k in ("op","owner","token","amount","recipient","fee","relayer","chain_derived","bedrock","zk","leaf") if k in d}, open(sys.argv[2],"w"), indent=1)' "$BD/proof-carol.json" "$EVID/proof-carol.json"
cp "$CH/chain-1.json" "$CH/chain-2.json" "$BD/anchor.json" "$EVID/"

echo "== 4. Sepolia: the releases"
cat "$KEPT" >> "$OUT"; rm -f "$KEPT"
REL() { local label="$1" expect="$2"; shift 3
  local out; out="$(python3 -W ignore scripts/sepolia_zk_release.py --run "$RUN" --deployment "$DEP" --proof "$BD/proof-carol.json" --bedrock-node "$BEDROCK_NODE" \
    --label "$label" --expect "$expect" --out "$OUT" "$@" 2>&1 | tail -1)"
  echo "$out" | cut -c1-300
  if [ "$(jget "$out" ok)" = True ]; then echo "  ok  $label: $(jget "$out" verdict) ($(jget "$out" precheck)), block $(jget "$out" block), gas $(jget "$out" gas_used)"
  else echo "  FAIL $label: $out"; fail=1; fi; }
grep -q '^# eth release-carol-one-member ' "$OUT" || REL release-carol-one-member HeaderNotAttested -- --members 0
grep -q '^# eth release-carol-tampered-journal ' "$OUT" || REL release-carol-tampered-journal VerificationFailed -- --tamper-journal
grep -q '^# eth release-carol ' "$OUT" || REL release-carol applied --
grep -q '^# eth release-carol-replay ' "$OUT" || REL release-carol-replay AlreadyReleased --
python3 - "$OUT" "$RUN/state/anvil/deposit-txs.json" "$EVID/releases.json" <<'PY'
import json, sys
att = []
for l in open(sys.argv[1]):
    if l.startswith("# eth "):
        p = l.split()
        att.append({"label": p[2], "expect": p[3], "verdict": p[4], "block": int(p[5]), "tx": p[6], "selector": p[7], "gas_used": int(p[8])})
json.dump({"deposits": json.load(open(sys.argv[2])), "attempts": att}, open(sys.argv[3], "w"), indent=1)
PY
python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));print(json.dumps({k: v for k, v in d.items()}, indent=1))' "$DEP" > "$EVID/deployment.json"
applied=$(awk '/^# eth / && $4=="applied" && $5=="applied"' "$OUT" | wc -l | tr -d ' ')
refused=$(awk '/^# eth / && $4!="applied" && $5=="reverted"' "$OUT" | wc -l | tr -d ' ')
echo "== $( [ $fail = 0 ] && echo PASS || echo FAIL ): $((applied + refused)) releases on Sepolia ($applied applied, $refused refused as expected), $(grep -c '^# check ok' "$OUT") checks ok"
exit $fail

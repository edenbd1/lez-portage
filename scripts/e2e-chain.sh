#!/usr/bin/env bash
# The chain-derived release, end to end, on the sequencer and anvil a
# scripts/e2e-local.sh run left running (KEEP=1):
#
#   1. Ethereum (anvil): a second vault, with the same pairing (the same LEZ
#      deployment), whose verifier is ZkChainBurnVerifier: pinned to the
#      release guest zk/burn_release, the chain guest zk/lez_chain, the
#      bridge program and its burn-tree account, and an anchor (the LEZ block
#      before the bridge's Init); a 2-of-3 committee (anvil accounts 5, 6, 7)
#      attests LEZ *blocks* final, FINALITY_DEPTH blocks deep. ETH registered,
#      1 ETH deposited so that it can pay a release.
#   2. LEZ: the chain guest proves the sequencer's blocks in two steps, from
#      the anchor to the block before carol's private burn, then from there to
#      the tip with the first receipt as its prior (verified inside): each
#      block's hash against its contents, its link to the previous one, and
#      the tree of the bridge's private burns.
#   3. The release proofs: carol's burn in that tree (zk/burn_release, which
#      verifies the chain receipt inside; one Groth16 seal for both); alice's
#      public burn, which the client refuses to prove (a public transaction
#      that reverted is still in its block, so public burns are not in the
#      tree); and a burn that never happened, really proven over a made-up LEZ
#      block after the anchor.
#   4. The releases on anvil: refusals (not final, redirected, another amount,
#      front-run, journal tampered, another anchor, a bare membership proof,
#      the made-up block signed by the committee and by outsiders, one
#      member), carol's release by the relayer, its replay.
#
# Usage: RUN=<the e2e run dir> BASE=<its manifest> scripts/e2e-chain.sh
# Writes $OUT (default evidence/local-chain/e2e-chain.tsv) and the raw
# measurements next to it. Everything is local: anvil and a standalone
# sequencer, no Bedrock. "Final" is the sequencer's tip, as in the base run.
set -uo pipefail
cd "$(dirname "$0")/.."

: "${RUN:?the run directory of a KEEP=1 e2e-local.sh run}"
: "${BASE:?the manifest of that run}"
CLI="${CLI:-cli/target/release/wrap}"
PORT="${PORT:-3051}"; ANVIL_PORT="${ANVIL_PORT:-8551}"
RPC="http://127.0.0.1:$PORT"; ANVIL="http://127.0.0.1:$ANVIL_PORT"
OUT="${OUT:-evidence/local-chain/e2e-chain.tsv}"
EVID="$(dirname "$OUT")"
HEAVY="${HEAVY:-}"; [ -x "$HEAVY" ] || HEAVY=""
FINALITY_DEPTH="${FINALITY_DEPTH:-3}"
export WRAP_RPC="$RPC" WRAP_RUN="$RUN/state" LEE_WALLET_HOME_DIR="$RUN/wallet"
CH="$RUN/chain"; mkdir -p "$EVID" "$CH"
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
DEP="$RUN/state/anvil/deployment.json"
[ -f "$DEP" ] && [ -f "$BASE" ] || { echo "no base run at $RUN ($BASE)"; exit 2; }
curl -s -m 5 -X POST "$RPC" -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getLastBlockId","params":[]}' | grep -q result \
  || { echo "the base run's sequencer is not running on $RPC"; exit 2; }
{
  echo "# the chain-derived release on the base run $(head -1 "$BASE" | cut -c3-)"
  echo "# commit $(git rev-parse --short HEAD)$(git diff --quiet -- . ':!evidence' || echo ' (uncommitted changes)'), $(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "$OUT"

block_of() { awk -F'\t' -v l="$1" '$1==l {print $4}' "$BASE"; }
INIT_BLOCK="$(block_of init-bridge)"; BURN_BLOCK="$(block_of burn-carol-private)"
ANCHOR=$((INIT_BLOCK - 1))
echo "== 0. the anchor: LEZ block $ANCHOR (the bridge's Init is in $INIT_BLOCK; carol's private burn in $BURN_BLOCK)"
CP="$("$CLI" chain-params --at "$ANCHOR" | grep '^{' | tail -1)"; echo "  $CP"
echo "# chain-params $CP" >> "$OUT"
ANCHOR_HASH="$(jget "$CP" block_hash)"; PROGRAM_ID="$(jget "$CP" program_id)"; BURNS_ID="$(jget "$CP" burns_account)"
IMG="$("$CLI" image-id)"
CHAIN_IMAGE="0x$(jget "$IMG" lez_chain.image_id)"; RELEASE_IMAGE="0x$(jget "$IMG" burn_release.image_id)"
echo "# binary lez_chain $(jget "$IMG" lez_chain.bytes) bytes sha256 $(jget "$IMG" lez_chain.sha256) image_id $CHAIN_IMAGE" >> "$OUT"
echo "# binary burn_release $(jget "$IMG" burn_release.bytes) bytes sha256 $(jget "$IMG" burn_release.sha256) image_id $RELEASE_IMAGE" >> "$OUT"

echo "== 1. Ethereum (anvil): a vault with ZkChainBurnVerifier, the same pairing"
DEPLOYER_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 # anvil's well-known account 0
ALICE_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d   # anvil's well-known account 1
create() { forge create --root vault --rpc-url "$ANVIL" --private-key "$DEPLOYER_KEY" --broadcast "$@" 2>&1 | awk '/^Deployed to:/ {print $3}'; }
COMMITTEE="[$(python3 -c 'import json,sys;print(",".join(json.load(open(sys.argv[1]))["committee"]))' "$DEP")]"
PAIRING="$(fget "$DEP" pairing)"; RISC0="$(fget "$DEP" risc0_verifier)"
check "the LEZ deployment's pairing is the vault's" "$(jget "$CP" pairing)" "$PAIRING"
VERIFIER="$(create src/ZkChainBurnVerifier.sol:ZkChainBurnVerifier --constructor-args "$RISC0" "$RELEASE_IMAGE" "$CHAIN_IMAGE" "$PROGRAM_ID" "$BURNS_ID" \
  "$ANCHOR" "$ANCHOR_HASH" "$COMMITTEE" 2 "$FINALITY_DEPTH")"
DEPLOYER="$(cast wallet address "$DEPLOYER_KEY")"
VAULT="$(create src/WrapVault.sol:WrapVault --constructor-args "$PAIRING" "$VERIFIER" 86400 "$DEPLOYER" "$DEPLOYER")"
[ -n "$VERIFIER" ] && [ -n "$VAULT" ] || { echo "deployment failed"; exit 1; }
cast send --private-key "$DEPLOYER_KEY" --rpc-url "$ANVIL" "$VAULT" "registerToken(address,uint8,uint256[],uint128,uint128)" \
  0x0000000000000000000000000000000000000000 18 "[100000000000000000,1000000000000000000]" 100000000000000000000 100000000000000000000 > /dev/null
cast send --private-key "$ALICE_KEY" --rpc-url "$ANVIL" --value 1000000000000000000 "$VAULT" "deposit(address,uint256,bytes32)" \
  0x0000000000000000000000000000000000000000 1000000000000000000 "0x$(openssl rand -hex 32)" > /dev/null
DEP2="$RUN/state/anvil/deployment-chain.json"
python3 - "$DEP" "$DEP2" "$VAULT" "$VERIFIER" "$ANCHOR" "$ANCHOR_HASH" "$PROGRAM_ID" "$BURNS_ID" "$CHAIN_IMAGE" "$RELEASE_IMAGE" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
d.update(vault=sys.argv[3], verifier=sys.argv[4], release_verifier="zkchain", anchor_id=int(sys.argv[5]), anchor_hash=sys.argv[6],
         program_id=sys.argv[7], burns_account=sys.argv[8], chain_image_id=sys.argv[9], image_id=sys.argv[10])
json.dump(d, open(sys.argv[2], "w"), indent=1)
PY
cp "$DEP2" "$EVID/deployment-chain.json"
echo "  vault $VAULT, verifier $VERIFIER"
echo "# anvil vault $VAULT verifier $VERIFIER risc0 $RISC0 pairing $PAIRING anchor $ANCHOR $ANCHOR_HASH program $PROGRAM_ID burns $BURNS_ID finality_depth $FINALITY_DEPTH" >> "$OUT"
for k in CHAIN_IMAGE_ID IMAGE_ID PROGRAM_ID BURNS_ACCOUNT ANCHOR_HASH; do
  want="$(eval echo "\$$(echo "$k" | sed 's/^IMAGE_ID$/RELEASE_IMAGE/; s/^CHAIN_IMAGE_ID$/CHAIN_IMAGE/; s/^BURNS_ACCOUNT$/BURNS_ID/')")"
  check "ZkChainBurnVerifier.$k" "$(cast call "$VERIFIER" "$k()(bytes32)" --rpc-url "$ANVIL")" "$want"
done
check "ZkChainBurnVerifier.ANCHOR_ID" "$(cast call "$VERIFIER" 'ANCHOR_ID()(uint64)' --rpc-url "$ANVIL")" "$ANCHOR"
check "the vault's verifier" "$(cast call "$VAULT" 'VERIFIER()(address)' --rpc-url "$ANVIL" | tr A-F a-f)" "$(echo "$VERIFIER" | tr A-F a-f)"

echo "== 2. LEZ: the chain guest over the sequencer's blocks, in two steps"
measure() { # name -- wrap args: run under heavy and time(1), keep the raw output
  local name="$1"; shift 2
  echo "# $name load before: $(uptime | sed 's/.*load/load/')" >> "$OUT"
  ${HEAVY:+"$HEAVY"} "${TIME[@]}" "$CLI" "$@" > "$EVID/$name.txt" 2>&1
  local rc=$?
  grep '^{' "$EVID/$name.txt" | tail -1
  return $rc
}
S1="$(measure chain-prove-1 -- chain-prove --anchor "$ANCHOR" --to $((BURN_BLOCK - 1)) --out "$CH/chain-1.json" --save-blocks "$CH/blocks")" || { echo "chain step 1 failed"; tail -20 "$EVID/chain-prove-1.txt"; exit 1; }
echo "  $(echo "$S1" | cut -c1-400)"
TIP="$(curl -s -X POST "$RPC" -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getLastBlockId","params":[]}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["result"])')"
S2="$(measure chain-prove-2 -- chain-prove --prior "$CH/chain-1.json" --to "$TIP" --out "$CH/chain-2.json" --save-blocks "$CH/blocks")" || { echo "chain step 2 failed"; tail -20 "$EVID/chain-prove-2.txt"; exit 1; }
echo "  $(echo "$S2" | cut -c1-400)"
for n in 1 2; do
  python3 - "$CH/chain-$n.json" "$EVID/chain-prove-$n.txt" "$n" >> "$OUT" <<'PY'
import json, re, sys
r, raw, n = json.load(open(sys.argv[1])), open(sys.argv[2]).read(), sys.argv[3]
rss = re.search(r"(\d+)\s+maximum resident set size", raw) or re.search(r"Maximum resident set size \(kbytes\): (\d+)", raw)
wall = re.search(r"([\d.]+) real", raw)
s = r["stats"]
print(f"# chain-proof {n} from {r['from']} tip {r['tip']['id']} {r['tip']['hash']} anchor {r['anchor']['id']} {r['anchor']['hash']} blocks {r['blocks']} block_bytes {r['block_bytes']} "
      f"prior {r['prior']} root {r['root']} next_index {r['next_index']} user_cycles {s['user_cycles']} total_cycles {s['total_cycles']} paging_cycles {s['paging_cycles']} "
      f"segments {s['segments']} stark_succinct_ms {s['stark_succinct_ms']} wall_s {wall.group(1) if wall else '?'} host_peak_rss_bytes {rss.group(1) if rss else '?'} "
      f"image_id {r['image_id']} dev_mode {r['dev_mode']}")
for l in r["leaves"]:
    print(f"# chain-leaf {n} index {l['index']} block {l['block']} tx {l['tx_index']} leaf {l['leaf']}")
PY
done
"$CLI" chain-layout "$CH/blocks" > "$CH/layout.json"; cat "$CH/layout.json"
echo "# chain-layout $(cat "$CH/layout.json")" >> "$OUT"
check "LEZ's Block and the guest's mirror decode, re-encode and hash every block identically" "$(jget "$(cat "$CH/layout.json")" identical)" True
check "the second step continues the first (its prior)" "$(fget "$CH/chain-2.json" prior)" True
check "both chain proofs are real (RISC0 dev mode off)" "$(fget "$CH/chain-1.json" dev_mode) $(fget "$CH/chain-2.json" dev_mode)" "False False"
lez_hash() { # the hash of LEZ block $1 as the sequencer returns it (borsh header: id, prev, hash, timestamp)
  python3 - "$RPC" "$1" <<'PY'
import base64, json, sys, time, urllib.request
for i in range(6):
    try:
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getBlock", "params": [int(sys.argv[2])]}).encode()
        r = json.load(urllib.request.urlopen(urllib.request.Request(sys.argv[1], body, {"Content-Type": "application/json"}), timeout=20))
        if "result" in r:
            print("0x" + base64.b64decode(r["result"])[40:72].hex()); sys.exit(0)
        print(f"getBlock {sys.argv[2]}: {r.get('error')}", file=sys.stderr)
    except Exception as e:
        print(f"getBlock {sys.argv[2]}: {e}", file=sys.stderr)
    time.sleep(2 * (i + 1))
sys.exit(1)
PY
}
TIP2="$(fget "$CH/chain-2.json" tip.id)"
check "the chain tip the proof ends at is the sequencer's block $TIP2" "$(fget "$CH/chain-2.json" tip.hash)" "$(lez_hash "$TIP2")"
CAROL_LEAF="$(python3 -c 'import json,sys;print(next(b["leaf"] for b in json.load(open(sys.argv[1])) if b["owner"]=="carol"))' "$RUN/state/burns.json")"
ALICE_LEAF="$(python3 -c 'import json,sys;print(next(b["leaf"] for b in json.load(open(sys.argv[1])) if b["owner"]=="alice"))' "$RUN/state/burns.json")"
check "the chain-derived tree holds carol's private burn, from its block" \
  "$(python3 -c 'import json,sys;print(" ".join(l["leaf"] + "@" + str(l["block"]) for l in json.load(open(sys.argv[1]))["leaves"]))' "$CH/chain-2.json")" "$CAROL_LEAF@$BURN_BLOCK"
check "alice's public burn is not in it" "$(grep -c "$ALICE_LEAF" "$CH/chain-2.json")" 0
cp "$CH/blocks/$BURN_BLOCK.borsh" "$EVID/lez-block-$BURN_BLOCK.borsh"

echo "== 3. the release proofs"
DEPV() { fget "$DEP2" "$1"; }
RELAYER="$(DEPV relayer)"; CAROL_ETH="$(DEPV carol)"; OUTSIDER="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["outsiders"][0])' "$DEP2")"
zkp() { # out raw -- wrap args (zk-prove-measured.sh: heavy, time(1), the Groth16 container's memory)
  local o="$1" raw="$2"; shift 3
  echo "# zk-prove load before: $(uptime | sed 's/.*load/load/')" >> "$OUT"
  CLI="$CLI" scripts/zk-prove-measured.sh "$o" "$raw" -- "$@" > "$CH/zk-prove.log" 2>&1 || { echo "prove failed"; cat "$CH/zk-prove.log"; tail -20 "$raw"; exit 1; }
}
# The release binds the vault it is for: the chain vault.
zkp "$CH/proof-carol.json" "$EVID/zk-prove-carol.txt" -- release-proof carol --zk --chain "$CH/chain-2.json" --relayer "$RELAYER" --vault "$VAULT"
"$CLI" release-proof alice --zk --chain "$CH/chain-2.json" --relayer "$RELAYER" --vault "$VAULT" > /dev/null 2> "$CH/alice.err"
ALICE_RC=$?
check "the client refuses to prove alice's public burn against the chain-derived tree" "$ALICE_RC $(grep -c 'not in the chain-derived tree' "$CH/alice.err")" "1 1"
echo "# alice-refused $(tail -1 "$CH/alice.err")" >> "$OUT"

# A burn that never happened: a leaf paying an outsider, in a made-up block after the anchor.
FORGED_IN="$CH/forged-input.json"
python3 - "$DEP2" > "$FORGED_IN" <<'PY'
import hashlib, json, sys
d = json.load(open(sys.argv[1]))
z, path = bytes(32), []
for _ in range(24):
    path.append("0x" + z.hex()); z = hashlib.sha256(z + z).digest()
json.dump({"chain_id": d["chain_id"], "vault": d["vault"], "pairing": d["pairing"], "relayer": d["outsiders"][0],
           "token": "0x" + "00" * 20, "amount": "1000000000000000000", "recipient": d["outsiders"][0], "fee": "0",
           "secret": "0x" + "66" * 32, "index": 0, "path": path}, sys.stdout)
PY
FORGED_LEAF="$(python3 - "$FORGED_IN" <<'PY'
import hashlib, json, sys
i = json.load(open(sys.argv[1]))
w = lambda b: bytes.fromhex(b[2:]).rjust(32, b"\0")
ec = hashlib.sha256(b"rfp022/eth-commit/v1".ljust(32, b"\0") + w(i["recipient"]) + int(i["fee"]).to_bytes(32, "big") + w(i["secret"])).digest()
leaf = hashlib.sha256(b"rfp022/burn/v1" + w(i["pairing"]) + bytes.fromhex(i["token"][2:]) + int(i["amount"]).to_bytes(16, "little") + ec).digest()
print("0x" + leaf.hex())
PY
)"
F="$("$CLI" chain-forge --anchor "$ANCHOR" --anchor-hash "$ANCHOR_HASH" --leaf "$FORGED_LEAF" --out-dir "$CH/forged-blocks" | grep '^{' | tail -1)"
echo "  forged: $F"; echo "# chain-forge $F" >> "$OUT"
measure chain-prove-forged -- chain-prove --anchor "$ANCHOR" --anchor-hash "$ANCHOR_HASH" --to $((ANCHOR + 1)) --blocks-dir "$CH/forged-blocks" --out "$CH/chain-forged.json" > /dev/null \
  || { echo "forged chain proof failed"; tail -20 "$EVID/chain-prove-forged.txt"; exit 1; }
zkp "$CH/proof-forged.json" "$EVID/zk-prove-forged.txt" -- zk-prove "$FORGED_IN" --chain "$CH/chain-forged.json"
python3 - "$CH" "$EVID" >> "$OUT" <<'PY'
import json, re, sys
ch, evid = sys.argv[1:3]
for who in ("carol", "forged"):
    p = json.load(open(f"{ch}/proof-{who}.json"))
    z = p.get("zk", p)
    raw = open(f"{evid}/zk-prove-{who}.txt").read()
    rss = re.search(r"(\d+)\s+maximum resident set size", raw) or re.search(r"Maximum resident set size \(kbytes\): (\d+)", raw)
    peak = re.search(r"container peak memory \(docker stats samples\) ([\d.]+) GiB", raw)
    wall = re.search(r"([\d.]+) real", raw)
    s, f = z["stats"], z["fields"]
    print(f"# zk-chain-proof {who} image_id {z['image_id']} journal_bytes {z['journal_bytes']} journal_digest {z['journal_digest']} release_digest {z['release_digest']} "
          f"root {f['root']} nullifier {f['nullifier']} chain_image_id {f['chain_image_id']} anchor {int(f['anchor_id'], 16)} {f['anchor_hash']} tip {int(f['tip_id'], 16)} {f['tip_hash']} "
          f"seal_bytes {z['seal_bytes']} user_cycles {s['user_cycles']} total_cycles {s['total_cycles']} stark_succinct_ms {s['stark_succinct_ms']} groth16_ms {s['groth16_ms']} "
          f"wall_s {wall.group(1) if wall else '?'} host_peak_rss_bytes {rss.group(1) if rss else '?'} groth16_container_peak_gib {peak.group(1) if peak else '?'} "
          f"dev_mode {z['dev_mode']} platform {z['docker_default_platform']}")
PY
check "both release proofs are real (RISC0 dev mode off)" "$(grep -c '^# zk-chain-proof .* dev_mode False ' "$OUT")" 2
check "carol's proof names the chain verifier's chain guest" "$(fget "$CH/proof-carol.json" zk.fields.chain_image_id)" "$CHAIN_IMAGE"
check "and the chain's tip" "$(python3 -c 'import json,sys;p=json.load(open(sys.argv[1]));print(int(p["zk"]["fields"]["tip_id"],16), p["zk"]["fields"]["tip_hash"])' "$CH/proof-carol.json")" \
  "$(fget "$CH/chain-2.json" tip.id) $(fget "$CH/chain-2.json" tip.hash)"
check "the forged proof is over the made-up block" "$(fget "$CH/proof-forged.json" fields.tip_hash)" "$(jget "$F" block_hash)"

# The Foundry fixture (vault/test/ZkChainBurnVerifier.t.sol): carol's proof, as published (no opening).
python3 - "$CH/proof-carol.json" vault/test/fixtures/zk_chain_release.json <<'PY'
import json, sys
z = json.load(open(sys.argv[1]))["zk"]
json.dump({k: z[k] for k in ("image_id", "journal", "journal_digest", "release_digest", "fields", "seal", "seal_bytes", "stats", "dev_mode", "docker_default_platform")},
          open(sys.argv[2], "w"), indent=1)
PY

echo "== 4. the releases (ZkChainBurnVerifier)"
wei() { cast balance "$1" --rpc-url "$ANVIL"; }
VAULT0="$(wei "$VAULT")"; CAROL0="$(wei "$CAROL_ETH")"
rel() {
  local label="$1" expect="$2"; shift 3
  local out
  out="$(python3 scripts/anvil_release.py --anvil "$ANVIL" --lez "$RPC" --deployment "$DEP2" --out "$OUT" --label "$label" --expect "$expect" --zkchain --gas-limit 900000 "$@" 2>&1 | tail -1)"
  echo "$out" | cut -c1-400
  if [ "$(jget "$out" ok)" = True ]; then echo "  ok  $label: $(jget "$out" verdict) ($(jget "$out" precheck)), block $(jget "$out" block), gas $(jget "$out" gas_used)"
  else echo "  FAIL $label: $out"; fail=1; fi
}
P="$CH/proof-carol.json"
rel release-carol-not-final NotFinal -- --proof "$P" --sender relayer
rel release-carol-redirected-to-relayer NotThisRelease -- --proof "$P" --sender relayer --recipient-as-sender --wait-final
rel release-carol-another-amount NotThisRelease -- --proof "$P" --sender relayer --amount 100000000000000000 --wait-final
rel release-carol-front-run NotThisRelease -- --proof "$P" --sender outsider --wait-final
rel release-carol-tampered-journal VerificationFailed -- --proof "$P" --sender relayer --tamper-journal --wait-final
rel release-carol-another-anchor NotThisChain -- --proof "$P" --sender relayer --tamper-anchor --wait-final
rel release-carol-membership-proof-only NotThisRelease -- --proof "$P" --membership-proof "$RUN/proof-carol.json" --sender relayer --wait-final
rel release-forged-chain-committee BlockNotAttested -- --proof "$P" --forged "$CH/proof-forged.json" --sender outsider --wait-final
rel release-forged-chain-outsiders BlockNotAttested -- --proof "$P" --forged "$CH/proof-forged.json" --sender outsider --outsiders --wait-final
rel release-carol-one-member BlockNotAttested -- --proof "$P" --sender relayer --members 1 --wait-final
rel release-carol applied -- --proof "$P" --sender relayer --wait-final
rel release-carol-replay AlreadyReleased -- --proof "$P" --sender relayer --wait-final
VAULT1="$(wei "$VAULT")"; CAROL1="$(wei "$CAROL_ETH")"
check "the chain vault paid out exactly carol's release (1 ETH)" "$(python3 -c "print($VAULT0 - $VAULT1)")" 1000000000000000000
check "carol received 1 ETH less the 0.01 ETH fee her burn fixed" "$(python3 -c "print($CAROL1 - $CAROL0)")" 990000000000000000
NF="$(fget "$P" zk.fields.nullifier)"
check "the vault recorded the release's nullifier" "$(cast call "$VAULT" 'nullifierSpent(bytes32)(bool)' "$NF" --rpc-url "$ANVIL")" true
check "the release sent no leaf, secret, Ethereum commitment or path node" \
  "$(awk '/^# eth-zk release-carol / {if ($7=="True" && $9=="True" && $11=="True") { split($13, p, "/"); if (p[1]==p[2]) print "hidden" }}' "$OUT" | sort -u)" hidden
cp "$P" "$EVID/proof-carol.json"; cp "$CH/chain-1.json" "$CH/chain-2.json" "$EVID/"
eth_applied=$(awk '/^# eth / && $4=="applied" && $5=="applied"' "$OUT" | wc -l | tr -d ' ')
eth_refused=$(awk '/^# eth / && $4!="applied" && $5=="reverted"' "$OUT" | wc -l | tr -d ' ')
echo "== $( [ $fail = 0 ] && echo PASS || echo FAIL ): $((eth_applied + eth_refused)) chain-derived releases ($eth_applied applied, $eth_refused refused as expected), $(grep -c '^# check ok' "$OUT") checks ok"
exit $fail

#!/usr/bin/env bash
# The bridge end to end on a local LEZ v0.3.1 standalone sequencer and a local
# anvil chain, through the client (cli/, `wrap`):
#
#   1. LEZ: load the four committed binaries through program_loader (Sextant's
#      sextant_lc, LEZ v0.3.0's token and associated-token-account programs,
#      wrap_bridge immutable); create the run's wallet accounts. The programs
#      are loaded first because the vault is deployed with the pairing, which
#      names wrap_bridge's program id.
#   2. Ethereum (anvil): deploy WrapVault and a USDT-shaped token, register
#      both tokens, make four deposits (ETH and USDT, one of them committed to
#      carol's private LEZ account); record the block and the storage proofs.
#   3. LEZ: Init a Sextant instance from a beacon header built around that
#      anvil block (anvil has no beacon chain; the caveat is printed and
#      recorded), Init and Register the bridge, then the claims: public ones,
#      a replay and other refusals, a privacy-preserving claim into carol's
#      shielded account with real proving (under $HEAVY if
#      present, and /usr/bin/time for the peak memory), carol's wallet syncing
#      and decrypting it; then the burns: alice's public one, and carol's
#      private one from her private account (a privacy-preserving BurnFrom,
#      real proving, measured the same way).
#   4. Ethereum (anvil): the releases against the LEZ burn tree. A 2-of-3
#      committee attests the burn tree's root as each member reads it from the
#      LEZ sequencer. With RELEASE_VERIFIER=zk (the default) the vault's
#      verifier is ZkBurnVerifier: each release carries a Groth16 proof made
#      here (`wrap release-proof --zk`: the guest zk/burn_membership, real
#      proving, the Groth16 step in RISC Zero's x86_64 Docker image, emulated
#      on Apple Silicon), checked on chain by RISC Zero's
#      RiscZeroGroth16Verifier, and the burn's opening is never sent.
#      Refusals: not final yet, a replay, a redirected recipient, another
#      amount, a front-run (another relayer), the journal tampered to match a
#      redirection, a burn that never happened (really proven, in a made-up
#      tree; signed by the committee, then by outsiders), one member alone.
#      With RELEASE_VERIFIER=committee, CommitteeBurnVerifier: each release
#      opens its burn leaf, with the refusals of that path. Then both releases.
#   5. The manifest $OUT, one line per LEZ transaction:
#        label expected(applied|refused) verdict block tx attempts precheck gas_limit charged_gas
#      plus `#` lines the verifier checks (programs, deployments, final state,
#      the private transactions' hidden accounts, the Ethereum releases and
#      their final state), then scripts/verify-onchain.sh and
#      scripts/mutation-test-verifier.sh against the same sequencer.
#
# Everything is local and discarded at the end (KEEP=1 leaves the sequencer and
# anvil running). Nothing here reaches a public network.
#
# Needs:
#   SEQUENCER  a v0.3.1 `sequencer_service` built with --features standalone (1a003bfb)
#   WALLET     the v0.3.0 `wallet` binary (db66590a), whose config and storage the CLI reads
#   LEZ_SRC    a LEZ checkout at 1a003bfb (its debug sequencer config, and the
#              public genesis test key of the debug configuration's first account)
#   anvil, forge, cast (Foundry), python3, r0vm 3.0.5, cli/target/release/wrap
# Ports: sequencer $PORT (3051), metrics $METRICS_PORT (9471), anvil $ANVIL_PORT (8551).
# FINALITY_DEPTH (3): the LEZ blocks a burn-tree root must be under the final head.
# RELEASE_VERIFIER (zk): zk (ZkBurnVerifier, Groth16) or committee (CommitteeBurnVerifier).
set -uo pipefail
cd "$(dirname "$0")/.."

: "${SEQUENCER:?path to a v0.3.1 standalone sequencer_service}"
: "${WALLET:?path to the v0.3.0 wallet binary}"
: "${LEZ_SRC:?a LEZ checkout at 1a003bfb}"
CLI="${CLI:-cli/target/release/wrap}"
PORT="${PORT:-3051}"; METRICS_PORT="${METRICS_PORT:-9471}"; ANVIL_PORT="${ANVIL_PORT:-8551}"
BLOCK_TIME="${BLOCK_TIME:-5s}"
OUT="${OUT:-evidence/local/e2e.tsv}"
EVID="$(dirname "$OUT")"
RUN="${RUN:-$(mktemp -d "${TMPDIR:-/tmp}/rfp022-e2e.XXXXXX")}"
HEAVY="${HEAVY:-}"; [ -x "$HEAVY" ] || HEAVY=""
FINALITY_DEPTH="${FINALITY_DEPTH:-3}"
RELEASE_VERIFIER="${RELEASE_VERIFIER:-zk}"
case "$RELEASE_VERIFIER" in zk|committee) ;; *) echo "RELEASE_VERIFIER is zk or committee"; exit 2 ;; esac
# DEV_MODE_DEBUG=1: RISC Zero dev mode for the sequencer and the client, to
# iterate on this script in seconds. Such a run cannot pass: the checks that
# dev mode is off fail, and the manifest records it.
if [ "${DEV_MODE_DEBUG:-0}" = 1 ]; then export RISC0_DEV_MODE=1; echo "DEV_MODE_DEBUG=1: dev-mode receipts, this run will FAIL its real-proving checks"; fi
RPC="http://127.0.0.1:$PORT"; ANVIL="http://127.0.0.1:$ANVIL_PORT"
export WRAP_RPC="$RPC" WRAP_RUN="$RUN/state" LEE_WALLET_HOME_DIR="$RUN/wallet"
mkdir -p "$EVID" "$RUN/seq" "$RUN/wallet" "$RUN/state"

jget() { python3 -c 'import json,sys
v=json.loads(sys.argv[1])
for k in sys.argv[2].split("."): v=(v or {}).get(k) if isinstance(v, dict) else None
print("" if v is None else v)' "$1" "$2"; }
rpc() { curl -s -m 20 -X POST "$RPC" -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}"; }
up() { rpc getLastBlockId '[]' | grep '"result"' > /dev/null; }
free() { ! (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; }

for p in "$PORT" "$METRICS_PORT" "$ANVIL_PORT"; do
  free "$p" || { echo "port $p is in use: choose PORT, METRICS_PORT, ANVIL_PORT"; exit 2; }
done
[ -x "$CLI" ] || { echo "build the client first: cargo build --release --manifest-path cli/Cargo.toml"; exit 2; }

PIDS=()
cleanup() {
  if [ "${KEEP:-0}" = 1 ]; then echo "KEEP=1: sequencer and anvil left running (pids ${PIDS[*]}), run dir $RUN"; return; fi
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
}
trap cleanup EXIT

echo "== 0. a local v0.3.1 sequencer on $PORT (blocks every $BLOCK_TIME) and anvil on $ANVIL_PORT"
python3 - "$LEZ_SRC/lez/sequencer/service/configs/debug/sequencer_config.json" "$RUN/seq/sequencer_config.json" "$BLOCK_TIME" <<'PY'
import json, sys
c = json.load(open(sys.argv[1])); c["block_create_timeout"] = sys.argv[3]
json.dump(c, open(sys.argv[2], "w"), indent=2)
PY
(cd "$RUN/seq" && exec "$SEQUENCER" sequencer_config.json --listen-address 127.0.0.1 --port "$PORT" \
   --home "$RUN/seq" --metrics-address "127.0.0.1:$METRICS_PORT" > "$RUN/seq/seq.log" 2>&1) &
PIDS+=($!)
anvil --port "$ANVIL_PORT" --chain-id 31337 --hardfork cancun --silent > "$RUN/anvil.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 90); do up && break; sleep 2; done
up || { echo "the sequencer did not answer"; tail -30 "$RUN/seq/seq.log"; exit 1; }
for _ in $(seq 50); do cast chain-id --rpc-url "$ANVIL" >/dev/null 2>&1 && break; sleep 0.2; done

# The wallet: its config names only the local sequencer before it ever runs.
cat > "$RUN/wallet/wallet_config.json" <<EOF
{
  "sequencers": [{ "sequencer_addr": "$RPC/" }],
  "seq_poll_timeout": "10s",
  "seq_tx_poll_max_blocks": 40,
  "seq_poll_max_retries": 30,
  "seq_block_poll_max_amount": 100,
  "multi_sequencer_client_config": { "distribution_limit": 1, "calibration_limit": 10 },
  "gas_limit": 10000000
}
EOF
printf 'local-e2e\n' | "$WALLET" check-health > /dev/null 2>&1 || true
# A funded genesis account of the debug configuration: its key is a public test
# key in LEZ's source (PRIVATE_KEY_PUB_ACC_A), read from there and never
# printed. The v0.3.0 wallet derives its id as 6iArKUX…, which the v0.3.1
# sequencer funds at genesis.
KEY="$(python3 - "$LEZ_SRC/lez/testnet_initial_state/src/lib.rs" <<'PY'
import re, sys
s = open(sys.argv[1]).read()
m = re.search(r"PRIVATE_KEY_PUB_ACC_A: \[u8; 32\] = \[([^\]]*)\]", s)
print(bytes(int(x) for x in m.group(1).replace("\n", " ").split(",") if x.strip()).hex())
PY
)"
"$WALLET" account import public --private-key "$KEY" > /dev/null 2>&1; unset KEY
PAYER="${PAYER:-6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV}"
export WRAP_PAYER="$PAYER"
PB="$(rpc getAccountBalance "[\"$PAYER\"]" | python3 -c 'import json,sys;print(json.load(sys.stdin)["result"])')"
echo "  payer $PAYER holds $PB"
[ "${PB:-0}" -gt 1000000000 ] 2>/dev/null || { echo "the payer is not funded on this sequencer"; exit 1; }
"$WALLET" account list > "$RUN/wallet-accounts.txt" 2>/dev/null
grep -q "Public/$PAYER" "$RUN/wallet-accounts.txt" || { echo "the wallet does not hold the payer's key"; exit 1; }

{
  echo "# wrap_bridge e2e against a local LEZ v0.3.1 standalone sequencer ($RPC) and anvil (chain 31337), $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "# commit $(git rev-parse --short HEAD)$(git diff --quiet -- . ':!evidence' || echo ' (uncommitted changes)')"
  echo "# payer $PAYER"
} > "$OUT"

fail=0
check() { # label got want
  if [ "$2" = "$3" ]; then echo "  ok  $1: $2"; echo "# check ok: $1 = $2" >> "$OUT"
  else echo "  FAIL $1: got $2, want $3"; echo "# check FAIL: $1: $2 != $3" >> "$OUT"; fail=1; fi
}
attempts_of() { python3 -c 'import json,sys;print(",".join(json.loads(sys.argv[1]).get("attempts") or []) or "-")' "$1"; }

echo "== 1. LEZ: the four committed binaries, through program_loader"
"$CLI" image-id | tee "$RUN/images.json"
python3 - "$RUN/images.json" >> "$OUT" <<'PY'
import json, sys
for k, v in json.load(open(sys.argv[1])).items():
    print(f"# binary {k} {v['bytes']} bytes sha256 {v['sha256']} image_id {v['image_id']}")
PY
echo "{}" > "$RUN/state/programs.json"
for name in sextant_lc token associated_token_account wrap_bridge; do
  imm=(); [ "$name" = wrap_bridge ] && imm=(--immutable)
  line="$("$CLI" --wait 120 deploy "$name" ${imm[@]+"${imm[@]}"} 2>>"$RUN/deploy.err" | grep '^{' | tail -1)"
  [ -n "$line" ] || { echo "deploy $name failed"; tail -20 "$RUN/deploy.err"; exit 1; }
  echo "$line" | cut -c1-200
  python3 - "$RUN/state/programs.json" "$line" "$OUT" <<'PY'
import json, sys
p, d = json.load(open(sys.argv[1])), json.loads(sys.argv[2])
p[d["name"]] = d["header"]
json.dump(p, open(sys.argv[1], "w"), indent=1)
with open(sys.argv[3], "a") as out:
    out.write(f"# program {d['header']} {d['image_id']} {d['name']}{' immutable' if d['immutable'] else ''}\n")
    for s in d["sent"]:
        what, h = s.split(":")
        out.write(f"# deploy {what} {h}\n")
PY
done
ACC="$("$CLI" setup 2>/dev/null | grep '^{' | tail -1)"
echo "  accounts $ACC"
CAROL="$(jget "$ACC" carol_private)"; ALICE="$(jget "$ACC" alice)"
echo "# accounts admin $(jget "$ACC" admin) freezer $(jget "$ACC" freezer) alice $ALICE bob $(jget "$ACC" bob)" >> "$OUT"
echo "# private-account $CAROL carol" >> "$OUT"

echo "== 2. Ethereum (anvil): the vault, both tokens, four deposits"
"$CLI" plan > "$RUN/plan.json" || exit 1
ZK_IMAGE="0x$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["burn_membership"]["image_id"])' "$RUN/images.json")"
python3 scripts/anvil_deposits.py --rpc "$ANVIL" --plan "$RUN/plan.json" --out "$RUN/state/anvil" --finality-depth "$FINALITY_DEPTH" \
  --release-verifier "$RELEASE_VERIFIER" --image-id "$ZK_IMAGE" || exit 1
mkdir -p "$EVID/anvil"
cp "$RUN/state/anvil/deployment.json" "$RUN/state/anvil/deposits.json" "$RUN/plan.json" "$EVID/anvil/"
python3 - "$RUN/state/anvil" >> "$OUT" <<'PY'
import json, sys
d = json.load(open(sys.argv[1] + "/deployment.json"))
b = json.load(open(sys.argv[1] + "/block.json"))["result"]
print(f"# anvil vault {d['vault']} usdt {d['usdt']} pairing {d['pairing']} block {int(b['number'], 16)} state_root {b['stateRoot']}")
print(f"# anvil verifier {d['verifier']} committee {','.join(d['committee'])} threshold {d['threshold']} finality_depth {d['finality_depth']} relayer {d['relayer']}")
if d.get("release_verifier") == "zk":
    print(f"# anvil zk-verifier {d['verifier']} risc0 {d['risc0_verifier']} image_id {d['image_id']}")
for x in json.load(open(sys.argv[1] + "/deposits.json")):
    print(f"# anvil deposit {x['index']} token {x['token']} amount {x['amount']} tx {x['transactionHash']}")
PY

# step label expected(applied|E72xx|E74xx) -- cli args...
step() {
  local label="$1" expect="$2"; shift 3
  local line verdict pre want force=()
  [ "$expect" != applied ] && force=(--force)
  line="$("$CLI" --label "$label" ${force[@]+"${force[@]}"} "$@" 2>"$RUN/step.err" | grep '^{' | tail -1)"
  [ -n "$line" ] || line="$(python3 -c 'import json,sys;print(json.dumps({"verdict":"error","error":sys.stdin.read().strip()[-300:]}))' < "$RUN/step.err")"
  echo "$line" | cut -c1-600
  verdict="$(jget "$line" verdict)"
  pre="$(jget "$line" precheck.gas)"; [ -n "$pre" ] && pre="gas=$pre" || pre="$(jget "$line" precheck.refused | cut -c1-5)"
  want=applied; [ "$expect" = applied ] || want=refused
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$label" "$want" "$verdict" "$(jget "$line" block)" "$(jget "$line" tx)" \
    "$(attempts_of "$line")" "${pre:--}" "$(jget "$line" gas_limit)" "$(jget "$line" block_gas_exec)" >> "$OUT"
  echo "# line $label $line" >> "$OUT"
  case "$expect:$verdict:$pre" in
    (applied:applied:gas=*) echo "  ok  $label: applied ($pre)" ;;
    (E7*:reverted:"$expect") echo "  ok  $label: refused ($pre), included, charged its whole limit, nothing changed" ;;
    (*) echo "  FAIL $label: $verdict ($pre), expected $expect"; fail=1 ;;
  esac
}

echo "== 3. LEZ: Sextant from the anvil block, the bridge, the claims"
step init-sextant applied -- init-sextant
CAVEAT="$(grep -o '"caveat":"[^"]*"' "$RUN/step.err" | head -1 | cut -d'"' -f4)"
echo "  CAVEAT: $CAVEAT"
echo "# caveat $CAVEAT" >> "$OUT"
step init-bridge applied -- init-bridge
step register-eth applied -- register eth
step register-usdt applied -- register usdt

echo "-- public claims (deposit 0: alice 1 ETH; 1: alice 100 USDT)"
step claim-alice-eth applied -- claim 0
check "alice holds 1 wrapped ETH" "$(jget "$("$CLI" balance alice eth)" balance)" 1000000000000000000
step claim-alice-eth-replay E7402 -- claim 0
step claim-alice-eth-replay-old-root E7402 -- claim 0 --root-after 1
step claim-alice-usdt applied -- claim 1
check "alice holds 100 wrapped USDT" "$(jget "$("$CLI" balance alice usdt)" balance)" 100000000
echo "-- refusals: a redirected claim, a root the vault never had"
step claim-bob-redirected-to-alice E7403 -- claim 2 --redirect-to alice
step claim-phantom-unknown-root E7208 -- claim 0 --phantom
check "wrapped ETH supply after the public claims" "$(jget "$("$CLI" balance alice eth)" supply)" 1000000000000000000

echo "-- the private claim (deposit 3: carol 1 ETH, committed to her private account), real proving"
[ "${DEV_MODE_DEBUG:-0}" = 1 ] || unset RISC0_DEV_MODE
TIMEF="$RUN/private-time.txt"
TIME=(/usr/bin/time -l); [ "$(uname)" = Darwin ] || TIME=(/usr/bin/time -v)
echo "# private-claim load before: $(uptime | sed 's/.*load/load/'); r0vm processes before: $(pgrep -f 'r0vm' | wc -l | tr -d ' ')" >> "$OUT"
line="$(${HEAVY:+"$HEAVY"} "${TIME[@]}" -o "$TIMEF" "$CLI" --label claim-carol-private --wait 600 claim 3 --private 2>"$RUN/private.err" | grep '^{' | tail -1)"
[ -n "$line" ] || line="$(python3 -c 'import json,sys;print(json.dumps({"verdict":"error","error":sys.stdin.read().strip()[-400:]}))' < "$RUN/private.err")"
echo "$line"
RSS="$(awk '/maximum resident set size/ {print $1} /Maximum resident set size/ {print $NF * 1024}' "$TIMEF" | head -1)"
WALL="$(awk '/ real / {print $1}' "$TIMEF" | head -1)"
printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' claim-carol-private applied "$(jget "$line" verdict)" "$(jget "$line" block)" "$(jget "$line" tx)" \
  "$(attempts_of "$line")" private - - >> "$OUT"
echo "# line claim-carol-private $line" >> "$OUT"
echo "# private claim-carol-private hides $CAROL tx_bytes $(jget "$line" tx_bytes)" >> "$OUT"
echo "# private-claim prove_and_submit_ms $(jget "$line" prove_and_submit_ms) wall_s ${WALL:-?} peak_rss_bytes ${RSS:-?} tx_bytes $(jget "$line" tx_bytes) kind $(jget "$line" kind) dev_mode $(jget "$line" dev_mode)" >> "$OUT"
check "the private claim landed" "$(jget "$line" verdict)" applied
check "a privacy-preserving transaction" "$(jget "$line" kind)" privacy-preserving
check "RISC0 dev mode off (real proving)" "$(jget "$line" dev_mode)" False
check "the opening is absent from the transaction" "$(python3 -c 'import json,sys;a=json.loads(sys.argv[1]).get("absent_from_tx") or {};n=a.get("path_nodes","0 of 1").split(" of ");print(all(a.get(k) for k in ("destination","secret","commitment","leaf")) and n[0]==n[1])' "$line")" True
echo "-- carol's wallet syncs and decrypts her private account"
SP="$("$CLI" show-private 2>/dev/null | grep '^{' | tail -1)"
echo "$SP"
echo "# show-private $SP" >> "$OUT"
check "carol decrypts 1 wrapped ETH in Private/$CAROL" "$(jget "$SP" wrapped_balance)" 1000000000000000000
check "her note's commitment is in the chain's commitment set" "$(jget "$SP" commitment_on_chain)" True
# The wallet binary reads the account the client's sync decrypted (the v0.3.0
# binary cannot decode v0.3.1 blocks itself; the client links v0.3.1's `common`).
TOKEN_HDR="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["token"])' "$RUN/state/programs.json")"
"$WALLET" account get --account-id "Private/$CAROL" --scope "$TOKEN_HDR" > "$RUN/wallet-get.txt" 2>/dev/null
REC="$(grep -A1 "Record of program $TOKEN_HDR" "$RUN/wallet-get.txt" | tail -1)"
echo "  wallet account get --account-id Private/$CAROL --scope $TOKEN_HDR: $REC"
echo "# wallet-get Private/$CAROL $TOKEN_HDR $REC" >> "$OUT"
check "the wallet shows the same record: a holding of 1 wrapped ETH" "$(python3 -c 'import sys
b=bytes.fromhex(sys.argv[1]) if sys.argv[1] else b""
print(int.from_bytes(b[33:49],"little") if len(b)==49 and b[0]==0 else "none")' "$REC")" 1000000000000000000
check "the private account is not public: no token shard on chain under its id" \
  "$(rpc getAccount "[\"$CAROL\"]" | python3 -c 'import json,sys;r=json.load(sys.stdin).get("result") or {};print(len((r.get("data") or {}).get("shards") or {}))')" 0
check "wrapped ETH supply after the private claim" "$(jget "$("$CLI" balance alice eth)" supply)" 2000000000000000000

echo "-- a public burn (alice, 1 ETH, Ethereum recipient hidden in a commitment that also fixes the relayer fee, 0)"
step burn-alice-eth-not-a-denomination E7408 -- burn alice eth 300000000000000000
step burn-alice-eth applied -- burn alice eth 1000000000000000000
check "alice holds no wrapped ETH" "$(jget "$("$CLI" balance alice eth)" balance)" 0
check "wrapped ETH supply after the public burn" "$(jget "$("$CLI" balance alice eth)" supply)" 1000000000000000000

echo "-- the private burn (carol, 1 ETH, from her private account; fee 0.01 ETH), real proving"
PRE="$("$CLI" --label burn-carol-private-not-a-denomination burn carol eth 300000000000000000 --private 2>/dev/null | grep '^{' | tail -1)"
echo "$PRE"
echo "# line burn-carol-private-not-a-denomination $PRE" >> "$OUT"
check "a private burn of an amount that is not a denomination is refused before proving" "$(jget "$PRE" verdict) $(jget "$PRE" precheck.refused | cut -c1-5)" "refused-precheck E7408"
TIMEB="$RUN/private-burn-time.txt"
echo "# private-burn load before: $(uptime | sed 's/.*load/load/'); r0vm processes before: $(pgrep -f 'r0vm' | wc -l | tr -d ' ')" >> "$OUT"
line="$(${HEAVY:+"$HEAVY"} "${TIME[@]}" -o "$TIMEB" "$CLI" --label burn-carol-private --wait 600 burn carol eth 1000000000000000000 --fee 10000000000000000 --private 2>"$RUN/private-burn.err" | grep '^{' | tail -1)"
[ -n "$line" ] || line="$(python3 -c 'import json,sys;print(json.dumps({"verdict":"error","error":sys.stdin.read().strip()[-400:]}))' < "$RUN/private-burn.err")"
echo "$line"
RSS="$(awk '/maximum resident set size/ {print $1} /Maximum resident set size/ {print $NF * 1024}' "$TIMEB" | head -1)"
WALL="$(awk '/ real / {print $1}' "$TIMEB" | head -1)"
printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' burn-carol-private applied "$(jget "$line" verdict)" "$(jget "$line" block)" "$(jget "$line" tx)" \
  "$(attempts_of "$line")" private - - >> "$OUT"
echo "# line burn-carol-private $line" >> "$OUT"
echo "# private burn-carol-private hides $CAROL tx_bytes $(jget "$line" tx_bytes)" >> "$OUT"
echo "# private-burn prove_and_submit_ms $(jget "$line" prove_and_submit_ms) wall_s ${WALL:-?} peak_rss_bytes ${RSS:-?} tx_bytes $(jget "$line" tx_bytes) kind $(jget "$line" kind) dev_mode $(jget "$line" dev_mode)" >> "$OUT"
check "the private burn landed" "$(jget "$line" verdict)" applied
check "a privacy-preserving transaction" "$(jget "$line" kind)" privacy-preserving
check "RISC0 dev mode off (real proving)" "$(jget "$line" dev_mode)" False
check "the holder, the Ethereum recipient and the burn secret are absent from it" "$(python3 -c 'import json,sys;a=json.loads(sys.argv[1]).get("absent_from_tx") or {};print(all(a.get(k) for k in ("holder","recipient","secret")))' "$line")" True
check "its burn leaf is public (what a release opens)" "$(jget "$line" leaf_in_tx)" True
SP="$("$CLI" show-private 2>/dev/null | grep '^{' | tail -1)"
echo "# show-private-after-burn $SP" >> "$OUT"
check "carol's wallet decrypts her account after the burn: 0 wrapped ETH" "$(jget "$SP" wrapped_balance)" 0
check "wrapped ETH supply after both burns" "$(jget "$("$CLI" balance alice eth)" supply)" 0

echo "== 4. Ethereum (anvil): releases against the LEZ burn tree ($RELEASE_VERIFIER)"
echo "# caveat-release the release committee (anvil accounts 5, 6, 7; 2 of 3) attests the LEZ burn tree's root as each member reads it from the sequencer; 'final' is the standalone sequencer's tip, $FINALITY_DEPTH blocks above the block read: there is no Bedrock in this run, and the committee is the trust assumption of this release path" >> "$OUT"
DEP="$RUN/state/anvil/deployment.json"
dep() { python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$DEP" "$1"; }
VAULT="$(dep vault)"; CAROL_ETH="$(dep carol)"; RELAYER="$(dep relayer)"; ALICE_ETH="$(dep alice)"
if [ "$RELEASE_VERIFIER" = zk ]; then
  echo "# caveat-zk each release carries a Groth16 proof of the burn's membership (guest burn_membership, image $ZK_IMAGE), checked on chain by RISC Zero's RiscZeroGroth16Verifier; the opening (secret, index, path, leaf) is never sent; the journal (token, amount, recipient, relayer, fee, root, nullifier) is public; the root is still the committee's" >> "$OUT"
  # The proof binds the address that will submit it: alice submits her own, a relayer carol's.
  zkprove() { # out raw -- wrap args
    local out="$1" raw="$2"; shift 3
    echo "# zk-prove load before: $(uptime | sed 's/.*load/load/'); r0vm processes before: $(pgrep -f 'r0vm' | wc -l | tr -d ' ')" >> "$OUT"
    CLI="$CLI" scripts/zk-prove-measured.sh "$out" "$raw" -- "$@" > "$RUN/zk-prove.log" 2>&1 || { echo "zk prove failed"; cat "$RUN/zk-prove.log"; tail -20 "$raw"; exit 1; }
    cat "$RUN/zk-prove.log"
  }
  zkprove "$RUN/proof-alice.json" "$EVID/zk-prove-alice.txt" -- release-proof alice --zk --relayer "$ALICE_ETH"
  zkprove "$RUN/proof-carol.json" "$EVID/zk-prove-carol.txt" -- release-proof carol --zk --relayer "$RELAYER"
  # A burn that never happened, really proven: a leaf paying an outsider, alone in a made-up tree.
  python3 - "$DEP" > "$RUN/forged-input.json" <<'PY'
import hashlib, json, sys
d = json.load(open(sys.argv[1]))
z, path = bytes(32), []
for _ in range(24):
    path.append("0x" + z.hex()); z = hashlib.sha256(z + z).digest()
json.dump({"chain_id": d["chain_id"], "vault": d["vault"], "pairing": d["pairing"], "relayer": d["outsiders"][0],
           "token": "0x" + "00" * 20, "amount": "1000000000000000000", "recipient": d["outsiders"][0], "fee": "0",
           "secret": "0x" + "66" * 32, "index": 0, "path": path}, sys.stdout)
PY
  zkprove "$RUN/proof-forged.json" "$EVID/zk-prove-forged.txt" -- zk-prove "$RUN/forged-input.json"
  python3 - "$RUN" "$EVID" >> "$OUT" <<'PY'
import json, re, sys
run, evid = sys.argv[1:3]
for who in ("alice", "carol", "forged"):
    p = json.load(open(f"{run}/proof-{who}.json"))
    z = p.get("zk", p)
    raw = open(f"{evid}/zk-prove-{who}.txt").read()
    rss = re.search(r"(\d+)\s+maximum resident set size", raw) or re.search(r"Maximum resident set size \(kbytes\): (\d+)", raw)
    peak = re.search(r"container peak memory \(docker stats samples\) ([\d.]+) GiB", raw)
    wall = re.search(r"([\d.]+) real", raw)
    s = z["stats"]
    print(f"# zk-proof {who} image_id {z['image_id']} journal_digest {z['journal_digest']} root {z['fields']['root']} nullifier {z['fields']['nullifier']} "
          f"selector {z['verifier_parameters']['selector']} seal_bytes {z['seal_bytes']} stark_succinct_ms {s['stark_succinct_ms']} groth16_ms {s['groth16_ms']} "
          f"total_cycles {s['total_cycles']} wall_s {wall.group(1) if wall else '?'} host_peak_rss_bytes {rss.group(1) if rss else '?'} "
          f"groth16_container_peak_gib {peak.group(1) if peak else '?'} dev_mode {z['dev_mode']} platform {z['docker_default_platform']}")
    if who != "forged":
        zeros, zz = set(), bytes(32)
        for _ in range(25):
            zeros.add("0x" + zz.hex()); zz = __import__("hashlib").sha256(zz + zz).digest()
        nodes = [x for x in p["path"] if x not in zeros]
        print(f"# release-proof {p['owner']} index {p['index']} leaf {p['leaf']} root {p['root']} nullifier {p['nullifier']} burn_tx {p['burn_tx']} private {p['private']}")
        print(f"# release-path {p['owner']} {','.join(nodes) or '-'}")
p = json.load(open(f"{run}/proof-carol.json"))
print(f"# release-root {p['root']} {p['burns_account']} {p['bridge_program']}")
PY
  check "the three Groth16 proofs are real (RISC0 dev mode off)" "$(grep -c '^# zk-proof .* dev_mode False ' "$OUT")" 3
  check "the zk verifier is pinned to the committed guest's image id" "$(cast call "$(dep verifier)" 'IMAGE_ID()(bytes32)' --rpc-url "$ANVIL")" "$ZK_IMAGE"
  check "the vault's verifier is the zk verifier" "$(cast call "$VAULT" 'VERIFIER()(address)' --rpc-url "$ANVIL" | tr A-F a-f)" "$(dep verifier | tr A-F a-f)"
  check "the seals carry RISC Zero's verifier selector" "$(cast call "$(dep risc0_verifier)" 'SELECTOR()(bytes4)' --rpc-url "$ANVIL")" \
    "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["zk"]["verifier_parameters"]["selector"])' "$RUN/proof-carol.json")"
  REL=(--zk --gas-limit 900000)
else
  for who in alice carol; do
    "$CLI" release-proof "$who" > "$RUN/proof-$who.json" 2>"$RUN/proof.err" || { echo "release-proof $who failed"; cat "$RUN/proof.err"; exit 1; }
  done
  python3 - "$RUN/proof-alice.json" "$RUN/proof-carol.json" >> "$OUT" <<'PY'
import json, sys
for f in sys.argv[1:]:
    p = json.load(open(f))
    print(f"# release-proof {p['owner']} index {p['index']} leaf {p['leaf']} root {p['root']} nullifier {p['nullifier']} burn_tx {p['burn_tx']} private {p['private']}")
print(f"# release-root {p['root']} {p['burns_account']} {p['bridge_program']}")
PY
  REL=()
fi
wei() { cast balance "$1" --rpc-url "$ANVIL"; }
VAULT0="$(wei "$VAULT")"; CAROL0="$(wei "$CAROL_ETH")"
rel() { # label expected -- args
  local label="$1" expect="$2"; shift 3
  local out
  out="$(python3 scripts/anvil_release.py --anvil "$ANVIL" --lez "$RPC" --deployment "$DEP" --out "$OUT" --label "$label" --expect "$expect" ${REL[@]+"${REL[@]}"} "$@" 2>&1 | tail -1)"
  echo "$out" | cut -c1-400
  if [ "$(jget "$out" ok)" = True ]; then echo "  ok  $label: $(jget "$out" verdict) ($(jget "$out" precheck)), block $(jget "$out" block), gas $(jget "$out" gas_used)"
  else echo "  FAIL $label: $out"; fail=1; fi
}
if [ "$RELEASE_VERIFIER" = zk ]; then
  rel release-carol-not-final NotFinal -- --proof "$RUN/proof-carol.json" --sender relayer
  rel release-alice applied -- --proof "$RUN/proof-alice.json" --wait-final
  rel release-alice-replay AlreadyReleased -- --proof "$RUN/proof-alice.json" --wait-final
  rel release-carol-redirected-to-relayer NotThisRelease -- --proof "$RUN/proof-carol.json" --sender relayer --recipient-as-sender --wait-final
  rel release-carol-another-amount NotThisRelease -- --proof "$RUN/proof-carol.json" --sender relayer --amount 100000000000000000 --wait-final
  rel release-carol-front-run NotThisRelease -- --proof "$RUN/proof-carol.json" --sender outsider --wait-final
  rel release-carol-tampered-journal VerificationFailed -- --proof "$RUN/proof-carol.json" --sender relayer --tamper-journal --wait-final
  rel release-forged-burn-committee RootNotAttested -- --proof "$RUN/proof-carol.json" --forged "$RUN/proof-forged.json" --sender outsider --wait-final
  rel release-forged-burn-outsiders RootNotAttested -- --proof "$RUN/proof-carol.json" --forged "$RUN/proof-forged.json" --sender outsider --outsiders --wait-final
  rel release-carol-one-member RootNotAttested -- --proof "$RUN/proof-carol.json" --sender relayer --members 1 --wait-final
  rel release-carol applied -- --proof "$RUN/proof-carol.json" --sender relayer --wait-final
  check "no zero-knowledge release sent its leaf, secret, Ethereum commitment or path" \
    "$(awk '/^# eth-zk / {n++; if ($7=="True" && $9=="True" && $11=="True") { split($13, p, "/"); if (p[1]==p[2]) ok++ } } END {print ok+0 "/" n+0}' "$OUT")" "9/9"
else
  rel release-carol-not-final NotFinal -- --proof "$RUN/proof-carol.json" --sender relayer
  rel release-alice applied -- --proof "$RUN/proof-alice.json" --wait-final
  rel release-alice-replay AlreadyReleased -- --proof "$RUN/proof-alice.json" --wait-final
  rel release-carol-redirected-to-relayer RootNotAttested -- --proof "$RUN/proof-carol.json" --sender relayer --recipient-as-sender --wait-final
  rel release-carol-another-amount RootNotAttested -- --proof "$RUN/proof-carol.json" --sender relayer --amount 100000000000000000 --wait-final
  rel release-forged-burn-committee RootNotAttested -- --proof "$RUN/proof-carol.json" --sender outsider --forge --wait-final
  rel release-forged-burn-outsiders RootNotAttested -- --proof "$RUN/proof-carol.json" --sender outsider --forge --outsiders --wait-final
  rel release-carol-one-member RootNotAttested -- --proof "$RUN/proof-carol.json" --sender relayer --members 1 --wait-final
  rel release-carol applied -- --proof "$RUN/proof-carol.json" --sender relayer --wait-final
fi
VAULT1="$(wei "$VAULT")"; CAROL1="$(wei "$CAROL_ETH")"
check "the vault paid out exactly the two releases (2 ETH)" "$(python3 -c "print($VAULT0 - $VAULT1)")" 2000000000000000000
check "carol received 1 ETH less the 0.01 ETH fee her burn fixed" "$(python3 -c "print($CAROL1 - $CAROL0)")" 990000000000000000
for who in alice carol; do
  NF="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["nullifier"])' "$RUN/proof-$who.json")"
  SPENT="$(cast call "$VAULT" 'nullifierSpent(bytes32)(bool)' "$NF" --rpc-url "$ANVIL")"
  echo "# eth-final nullifier $NF $SPENT release-$who" >> "$OUT"
  check "the vault recorded $who's burn nullifier as released" "$SPENT" true
done
echo "# eth-final unspent $(python3 -c 'import secrets;print("0x"+secrets.token_hex(32))') false control" >> "$OUT"
echo "# eth-final balance $VAULT $VAULT1 vault" >> "$OUT"

echo "== final state: the accounts the run wrote, as it left them"
IDS="$("$CLI" ids)"
python3 - "$RPC" "$IDS" "$RUN/state/programs.json" >> "$OUT" <<'PY'
import hashlib, json, sys, urllib.request
rpc, ids, progs = sys.argv[1], json.loads(sys.argv[2]), json.load(open(sys.argv[3]))
br, tok, lc = progs["wrap_bridge"], progs["token"], progs["sextant_lc"]
def shard(a, p):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getAccount", "params": [a]}).encode()
    r = json.load(urllib.request.urlopen(urllib.request.Request(rpc, body, {"Content-Type": "application/json"}), timeout=25))
    return bytes((((r.get("result") or {}).get("data") or {}).get("shards") or {}).get(p, []))
which = [("sextant_instance", lc), ("sextant_header", lc), ("config", br), ("eth_entry", br), ("usdt_entry", br), ("burns", br),
         ("nullifier_0", br), ("nullifier_1", br), ("nullifier_2", br), ("nullifier_3", br),
         ("eth_definition", tok), ("usdt_definition", tok), ("alice_eth_ata", tok), ("alice_usdt_ata", tok)]
for name, p in which:
    b = shard(ids[name], p)
    print(f"# final {ids[name]} {p} {len(b)} {hashlib.sha256(b).hexdigest()} {name}")
PY
grep '^# final' "$OUT" | awk '{print "  " $6 ": " $5 " bytes"}'
check "bob's deposit (2) stays unclaimed: its nullifier account is empty" "$(grep ' nullifier_2$' "$OUT" | awk '{print $5}')" 0

applied=$(awk -F'\t' '!/^#/ && $2=="applied" && $3=="applied"' "$OUT" | wc -l | tr -d ' ')
refused=$(awk -F'\t' '!/^#/ && $2=="refused" && $3=="reverted"' "$OUT" | wc -l | tr -d ' ')
total=$(grep -cv '^#' "$OUT")
eth_applied=$(awk '/^# eth / && $4=="applied" && $5=="applied"' "$OUT" | wc -l | tr -d ' ')
eth_refused=$(awk '/^# eth / && $4!="applied" && $5=="reverted"' "$OUT" | wc -l | tr -d ' ')
echo "== $( [ $fail = 0 ] && echo PASS || echo FAIL ): $total LEZ transactions in $OUT ($applied applied, $refused refused as expected), $((eth_applied + eth_refused)) Ethereum releases ($eth_applied applied, $eth_refused refused as expected), $(grep -c '^# check ok' "$OUT") state checks ok"

if [ "${VERIFY:-1}" = 1 ]; then
  echo "== verifier, against the same sequencer"
  scripts/verify-onchain.sh --manifest "$OUT" --rpc "$RPC" --eth-rpc "$ANVIL" | tee "$EVID/verify.txt" || fail=1
  echo "== mutation test of the verifier"
  scripts/mutation-test-verifier.sh --manifest "$OUT" --rpc "$RPC" --eth-rpc "$ANVIL" | tee "$EVID/mutation-test.txt" || fail=1
fi
exit $fail

#!/usr/bin/env bash
# The inbound path on public networks: a deposit into WrapVault on Sepolia,
# attested on the public LEZ testnet by Sextant (RFP-021) against a Sepolia
# checkpoint finalized after it, and minted by wrap_bridge, by a public claim
# and by a privacy-preserving one into a shielded account.
#
#   scripts/testnet-inbound.sh lez-deploy   load the ATA program and wrap_bridge (immutable),
#                                           make the run's accounts, write network.json and the plan
#   scripts/testnet-inbound.sh eth          deploy the vault on Sepolia and make the plan's deposits
#   scripts/testnet-inbound.sh fetch        once Sepolia has finalized the last deposit: a checkpoint
#                                           at a finalized header after it (Sextant's
#                                           fetch-vectors.py --finalized-after) and, at its execution
#                                           block, eth_getProof of knownRoot[root] for every vault root
#   scripts/testnet-inbound.sh sextant      Init a Sextant instance from that checkpoint (Sextant's
#                                           own scripts/e2e.sh SCENARIO=chain CHAIN=sepolia-portage)
#   scripts/testnet-inbound.sh claims       Init and Register the bridge, the public claim, two
#                                           refusals (a replay, a root the vault never had), the
#                                           private claim (real proving), the final state
#
# Every public LEZ transaction is pre-checked locally first (the client's
# pre-check, through lee::V03State with the deployed binaries and the chain's
# accounts) and sent with its measured gas plus 1% and 10,000; a refusal is
# sent with the honest instruction's gas, since on v0.3 a refused transaction
# is included and charged its whole limit. A stage refuses to start if the
# payer cannot fund it.
#
# MODE=testnet (default) or MODE=local: the same stages against a local v0.3.1
# standalone sequencer (`local-up` starts it and leaves it running; it loads
# Sextant's and the token program's binaries too), a dry run with the same
# Sepolia vault code and the same checkpoint.
#
# Needs:
#   RUN             a directory kept between stages (wallet copy, state, keys); never committed
#   LEE_WALLET_HOME_DIR  the v0.3.0 wallet home holding PAYER's key (testnet: a copy of it)
#   PAYER           the LEZ account that pays every fee
#   SEXTANT_REPO    an edenbd1/lez-sextant checkout with cli/target/release/sextant built
#   SEPOLIA_KEY_FILE  the Sepolia deployer's key (read by scripts/sepolia_vault.py only)
#   testnet: SEXTANT_PROGRAM, TOKEN_PROGRAM (the deployed Sextant v2 and LEZ v0.3.0 token headers)
#   local:   SEQUENCER (v0.3.1 standalone), WALLET (v0.3.0), LEZ_SRC (a LEZ checkout at 1a003bfb)
#   cast, forge, python3 (eth_account, pycryptodome), cli/target/release/wrap, r0vm 3.0.5
set -uo pipefail
cd "$(dirname "$0")/.."
STAGE="${1:?stage: local-up | lez-deploy | eth | fetch | sextant | claims}"
MODE="${MODE:-testnet}"
: "${RUN:?a run directory kept between stages}"
CLI="${CLI:-cli/target/release/wrap}"
: "${SEXTANT_REPO:?an edenbd1/lez-sextant checkout}"
SEXTANT_CLI="$SEXTANT_REPO/cli/target/release/sextant"
CHAIN_DIR="$SEXTANT_REPO/vectors/sepolia-portage"
# Sextant v2 (ImageID ad6fbedb...), the binary deployed on the testnet as Ebe8rifv...
SEXTANT_V2_SHA="58a600a4c9e1d608b27bd16f59aef53587ee4fcbd4152c575f754a96604b3dbd"
HEAVY="${HEAVY:-}"; [ -x "$HEAVY" ] || HEAVY=""
if [ "$MODE" = local ]; then
  RPC="${RPC:-http://127.0.0.1:3061}"; OUT="${OUT:-$RUN/run.tsv}"
else
  RPC="${RPC:-https://testnet.lez.logos.co}"; OUT="${OUT:-evidence/testnet/run.tsv}"
fi
export WRAP_RPC="$RPC" WRAP_RUN="$RUN/state" WRAP_ARTIFACTS="$RUN/artifacts" WRAP_PAYER="${PAYER:-}"
EVID="$(dirname "$OUT")"
mkdir -p "$RUN/state" "$RUN/artifacts" "$EVID"
chmod 700 "$RUN"

jget() { python3 -c 'import json,sys
v=json.loads(sys.argv[1])
for k in sys.argv[2].split("."): v=(v or {}).get(k) if isinstance(v, dict) else None
print("" if v is None else v)' "$1" "$2"; }
rpc() { curl -s -m 30 -X POST "$RPC" -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}"; }
balance() { rpc getAccountBalance "[\"$PAYER\"]" | python3 -c 'import json,sys;print(json.load(sys.stdin)["result"])'; }
need() { # stage-cost: refuse to start a stage the payer cannot fund
  local b; b="$(balance)"
  [ "$b" -ge "$1" ] 2>/dev/null || { echo "the payer holds $b, below the $1 this stage needs: stopping"; exit 1; }
  echo "  payer $PAYER holds $b (this stage needs up to $1)"
}
fail=0
check() { # label got want
  if [ "$2" = "$3" ]; then echo "  ok  $1: $2"; echo "# check ok: $1 = $2" >> "$OUT"
  else echo "  FAIL $1: got $2, want $3"; echo "# check FAIL: $1: $2 != $3" >> "$OUT"; fail=1; fi
}
attempts_of() { python3 -c 'import json,sys;print(",".join(json.loads(sys.argv[1]).get("attempts") or []) or "-")' "$1"; }
step() { # label expected(applied|E72xx|E74xx) -- cli args...
  local label="$1" expect="$2"; shift 3
  local line verdict pre want force=()
  [ "$expect" != applied ] && force=(--force)
  line="$("$CLI" --label "$label" --wait 240 ${force[@]+"${force[@]}"} "$@" 2>"$RUN/step.err" | grep '^{' | tail -1)"
  [ -n "$line" ] || line="$(python3 -c 'import json,sys;print(json.dumps({"verdict":"error","error":sys.stdin.read().strip()[-300:]}))' < "$RUN/step.err")"
  echo "$line" | cut -c1-600
  verdict="$(jget "$line" verdict)"
  pre="$(jget "$line" precheck.gas)"; [ -n "$pre" ] && pre="gas=$pre" || pre="$(jget "$line" precheck.refused | cut -c1-5)"
  want=applied; [ "$expect" = applied ] || want=refused
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$label" "$want" "$verdict" "$(jget "$line" block)" "$(jget "$line" tx)" \
    "$(attempts_of "$line")" "${pre:--}" "$(jget "$line" gas_limit)" "$(jget "$line" block_gas_exec)" >> "$OUT"
  echo "# line $label $line" >> "$OUT"
  LAST="$line"
  case "$expect:$verdict:$pre" in
    (applied:applied:gas=*) echo "  ok  $label: applied ($pre)" ;;
    (E7*:reverted:"$expect") echo "  ok  $label: refused ($pre), included, charged its whole limit, nothing changed" ;;
    (*) echo "  FAIL $label: $verdict ($pre), expected $expect"; fail=1; return 1 ;;
  esac
}
artifacts() { # the binaries the client pre-checks and proves with: the committed three, and Sextant v2
  local a=artifacts/programs/v0.3 s="$SEXTANT_REPO/artifacts/programs/v0.3/sextant_lc.bin"
  [ "$(shasum -a 256 "$s" | cut -d' ' -f1)" = "$SEXTANT_V2_SHA" ] || { echo "$s is not Sextant v2 ($SEXTANT_V2_SHA)"; exit 1; }
  cp "$s" "$RUN/artifacts/sextant_lc.bin"
  for n in token associated_token_account wrap_bridge; do cp "$a/$n.bin" "$RUN/artifacts/$n.bin"; done
  for n in token associated_token_account wrap_bridge; do cmp -s "$a/$n.bin" "$RUN/artifacts/$n.bin" || exit 1; done
}
# The Sextant client, against the same sequencer and payer, from its own checkout.
sextant() { (cd "$SEXTANT_REPO" && SEXTANT_RPC="$RPC" SEXTANT_PAYER="$PAYER" SEXTANT_PROGRAM="$SEXTANT_PROGRAM" \
  SEXTANT_CHAIN=sepolia-portage SEXTANT_INSTANCE="$INSTANCE" "$SEXTANT_CLI" "$@"); }
net() { python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$RUN/state/network.json" "$1"; }
progs() { python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$RUN/state/programs.json" "$1"; }

case "$STAGE" in
local-up)
  [ "$MODE" = local ] || { echo "local-up is MODE=local only"; exit 2; }
  : "${SEQUENCER:?}" "${WALLET:?}" "${LEZ_SRC:?}"
  PORT="${RPC##*:}"
  mkdir -p "$RUN/seq" "$LEE_WALLET_HOME_DIR"
  python3 - "$LEZ_SRC/lez/sequencer/service/configs/debug/sequencer_config.json" "$RUN/seq/sequencer_config.json" <<'PY'
import json, sys
c = json.load(open(sys.argv[1])); c["block_create_timeout"] = "5s"
json.dump(c, open(sys.argv[2], "w"), indent=2)
PY
  (cd "$RUN/seq" && exec "$SEQUENCER" sequencer_config.json --listen-address 127.0.0.1 --port "$PORT" \
     --home "$RUN/seq" --metrics-address "127.0.0.1:$((PORT + 6400))" > "$RUN/seq/seq.log" 2>&1) &
  echo $! > "$RUN/seq/pid"
  for _ in $(seq 90); do rpc getLastBlockId '[]' | grep -q '"result"' && break; sleep 2; done
  cat > "$LEE_WALLET_HOME_DIR/wallet_config.json" <<EOF
{ "sequencers": [{ "sequencer_addr": "$RPC/" }], "seq_poll_timeout": "10s", "seq_tx_poll_max_blocks": 40,
  "seq_poll_max_retries": 30, "seq_block_poll_max_amount": 100,
  "multi_sequencer_client_config": { "distribution_limit": 1, "calibration_limit": 10 }, "gas_limit": 10000000 }
EOF
  printf 'local-dry-run\n' | "$WALLET" check-health > /dev/null 2>&1 || true
  KEY="$(python3 - "$LEZ_SRC/lez/testnet_initial_state/src/lib.rs" <<'PY'
import re, sys
s = open(sys.argv[1]).read()
m = re.search(r"PRIVATE_KEY_PUB_ACC_A: \[u8; 32\] = \[([^\]]*)\]", s)
print(bytes(int(x) for x in m.group(1).replace("\n", " ").split(",") if x.strip()).hex())
PY
)"
  "$WALLET" account import public --private-key "$KEY" > /dev/null 2>&1; unset KEY
  echo "sequencer up on $RPC (pid $(cat "$RUN/seq/pid")), payer 6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV"
  ;;

lez-deploy)
  : "${PAYER:?}" "${INSTANCE:?a Sextant instance name, at most 32 bytes}"
  artifacts
  record() { python3 - "$RUN/state/programs.json" "$1" "$OUT" <<'PY'
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
  }
  if [ -f "$RUN/state/programs.json" ]; then
    echo "  resuming the deployment recorded in programs.json"
  else
  "$CLI" image-id | tee "$RUN/images.json"
  [ "$MODE" = local ] && need 1000000000 || need 12000000
  {
    echo "# Portage inbound on public networks: LEZ $RPC ($MODE), Sepolia (chain 11155111), $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "# commit $(git rev-parse --short HEAD)$(git diff --quiet -- . ':!evidence' || echo ' (uncommitted changes)')"
    echo "# payer $PAYER balance_before $(balance)"
  } > "$OUT"
  python3 - "$RUN/images.json" >> "$OUT" <<'PY'
import json, sys
for k, v in json.load(open(sys.argv[1])).items():
    if k in ("sextant_lc", "token", "associated_token_account", "wrap_bridge"):
        print(f"# binary {k} {v['bytes']} bytes sha256 {v['sha256']} image_id {v['image_id']}")
PY
  echo "{}" > "$RUN/state/programs.json"
  if [ "$MODE" = local ]; then
    for name in sextant_lc token; do
      line="$("$CLI" --wait 120 deploy "$name" 2>>"$RUN/deploy.err" | grep '^{' | tail -1)"
      [ -n "$line" ] || { echo "deploy $name failed"; tail -20 "$RUN/deploy.err"; exit 1; }
      record "$line"
    done
  else
    : "${SEXTANT_PROGRAM:?}" "${TOKEN_PROGRAM:?}"
    python3 - "$RUN/state/programs.json" "$SEXTANT_PROGRAM" "$TOKEN_PROGRAM" "$RUN/images.json" "$OUT" <<'PY'
import json, sys
p = json.load(open(sys.argv[1])); im = json.load(open(sys.argv[4]))
p["sextant_lc"], p["token"] = sys.argv[2], sys.argv[3]
json.dump(p, open(sys.argv[1], "w"), indent=1)
with open(sys.argv[5], "a") as out:
    out.write(f"# program {sys.argv[2]} {im['sextant_lc']['image_id']} sextant_lc (deployed earlier: lez-sextant v2)\n")
    out.write(f"# program {sys.argv[3]} {im['token']['image_id']} token immutable (deployed earlier: LEZ v0.3.0 token.bin)\n")
PY
  fi
  fi
  for name in associated_token_account wrap_bridge; do
    [ -n "$(progs "$name" 2>/dev/null)" ] && continue
    imm=(); [ "$name" = wrap_bridge ] && imm=(--immutable)
    # RESUME_<name>="<header> <segment 0> <segment 1> ...": a deployment cut short (its accounts in the wallet).
    var="RESUME_$name"; res=()
    if [ -n "${!var:-}" ]; then set -- ${!var}; res=(--header "$1"); shift; for sg in "$@"; do res+=(--segment "$sg"); done; fi
    line="$("$CLI" --wait 240 deploy "$name" ${imm[@]+"${imm[@]}"} ${res[@]+"${res[@]}"} 2>>"$RUN/deploy.err" | grep '^{' | tail -1)"
    [ -n "$line" ] || { echo "deploy $name failed"; tail -20 "$RUN/deploy.err"; exit 1; }
    echo "$line" | cut -c1-300
    record "$line"
  done
  if [ ! -f "$RUN/state/accounts.json" ]; then
    ACC="$("$CLI" setup 2>/dev/null | grep '^{' | tail -1)"
    echo "# accounts admin $(jget "$ACC" admin) freezer $(jget "$ACC" freezer) alice $(jget "$ACC" alice) bob $(jget "$ACC" bob)" >> "$OUT"
    echo "# private-account $(jget "$ACC" carol_private) carol" >> "$OUT"
  fi
  [ -f "$RUN/state/network.json" ] && { echo "network.json exists: deployed already"; exit 2; }
  # This deployment's identity, and fresh depositor seeds (kept in $RUN only).
  python3 - "$RUN/state/network.json" "$INSTANCE" <<'PY'
import json, secrets, sys
amount = "1000000000000000"   # 0.001 ETH: the one permitted amount
json.dump({
  "deployment_id": "portage/sepolia/deployment/0001",
  "lez_chain_id": "lez/public-testnet/v0.3.1/chain1",
  "zone_id": "lez/public-testnet/zone/00000001",
  "instance_id": sys.argv[2], "slot": 0, "chain_id": 11155111,
  "eth_name": "Wrapped Sepolia Ether",
  "eth_policy": {"denominations": [amount], "mint_cap": "10000000000000000"},
  "plan": [["alice", 0, "eth", amount], ["carol", 0, "eth", amount]],
  "alice_seed": "0x" + secrets.token_hex(32), "bob_seed": "0x" + secrets.token_hex(32), "carol_seed": "0x" + secrets.token_hex(32),
  "phantom_amount": amount,
}, open(sys.argv[1], "w"), indent=1)
PY
  chmod 600 "$RUN/state/network.json"
  "$CLI" plan > "$RUN/state/plan.json" || exit 1
  echo "# network deployment_id $(net deployment_id) lez_chain_id $(net lez_chain_id) zone_id $(net zone_id) instance $INSTANCE pairing $(jget "$(cat "$RUN/state/plan.json")" pairing)" >> "$OUT"
  echo "# payer balance_after_deploy $(balance)" >> "$OUT"
  echo "deployed; pairing $(jget "$(cat "$RUN/state/plan.json")" pairing)"
  ;;

eth)
  : "${SEPOLIA_KEY_FILE:?}"
  python3 -W ignore scripts/sepolia_vault.py deploy --plan "$RUN/state/plan.json" --out "$RUN/state/anvil" --keys "$RUN/keys" || exit 1
  python3 -W ignore scripts/sepolia_vault.py deposit --plan "$RUN/state/plan.json" --out "$RUN/state/anvil" --keys "$RUN/keys" || exit 1
  ;;

fetch)
  LASTB="$(python3 -c 'import json,sys;print(max(x["block"] for x in json.load(open(sys.argv[1]))))' "$RUN/state/anvil/deposit-txs.json")"
  if ! ls "$CHAIN_DIR"/bootstrap-*.json > /dev/null 2>&1 || [ "$(ls -d "$CHAIN_DIR"/block-* | sed 's/.*block-//')" -lt "$LASTB" ]; then
    rm -rf "$CHAIN_DIR"
    (cd "$SEXTANT_REPO" && python3 scripts/fetch-vectors.py sepolia vectors/sepolia-portage --finalized-after "$LASTB") || exit 1
  fi
  CB="$(ls -d "$CHAIN_DIR"/block-*)"
  BH="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["result"]["hash"])' "$CB/block.json")"
  CSLOT="$(ls "$CHAIN_DIR"/bootstrap-*.json | sed 's/.*bootstrap-//;s/.json//')"
  echo "checkpoint slot $CSLOT, execution block ${CB##*block-} ($BH), last deposit in block $LASTB"
  python3 -W ignore scripts/sepolia_vault.py proofs --plan "$RUN/state/plan.json" --out "$RUN/state/anvil" --block-hash "$BH" || exit 1
  python3 - "$RUN/state/network.json" "$CSLOT" <<'PY'
import json, sys
j = json.load(open(sys.argv[1])); j["slot"] = int(sys.argv[2]); json.dump(j, open(sys.argv[1], "w"), indent=1)
PY
  ;;

sextant)
  : "${PAYER:?}"; INSTANCE="$(net instance_id)"; SEXTANT_PROGRAM="$(progs sextant_lc)"
  need 18000000
  SOUT="${SEXTANT_OUT:-$RUN/sextant-run.tsv}"
  (cd "$SEXTANT_REPO" && LEE_WALLET_HOME_DIR="$LEE_WALLET_HOME_DIR" PAYER="$PAYER" RPC="$RPC" SEXTANT_PROGRAM="$SEXTANT_PROGRAM" \
     INSTANCE="$INSTANCE" SCENARIO=chain CHAIN=sepolia-portage BASE_FEE_MAX=8 SKIP_STEPS="attest-checkpoint-predicate-false" \
     OUT="$SOUT" scripts/e2e.sh) 2>&1 | tee "$RUN/sextant-transcript.txt"
  grep -q $'^init\tapplied\tapplied' "$SOUT" || { echo "Sextant's Init did not apply"; exit 1; }
  awk -F'\t' '$1=="init"' "$SOUT" | head -1 | sed 's/^init/sextant-init/' >> "$OUT"
  grep '^# line init ' "$SOUT" | head -1 | sed 's/^# line init /# line sextant-init /' >> "$OUT"
  IDS="$(sextant ids)"
  echo "# sextant instance $INSTANCE $(jget "$IDS" instance) checkpoint_header $(jget "$IDS" checkpoint_header) slot $(net slot) manifest $(basename "$SOUT")" >> "$OUT"
  ;;

claims)
  : "${PAYER:?}"
  need 25000000
  CB="$(ls -d "$CHAIN_DIR"/block-*)"
  python3 - "$RUN/state/anvil" "$CB" >> "$OUT" <<'PY'
import json, sys
d = json.load(open(sys.argv[1] + "/deployment.json"))
b = json.load(open(sys.argv[1] + "/block.json"))["result"]
print(f"# sepolia vault {d['vault']} verifier {d['verifier']} pairing {d['pairing']} txs {d['txs']['vault']} {d['txs']['register_eth']}")
print(f"# sepolia checkpoint-block {int(b['number'], 16)} {b['hash']} state_root {b['stateRoot']}")
for x in json.load(open(sys.argv[1] + "/deposits.json")):
    print(f"# sepolia deposit {x['index']} amount {x['amount']} leaf {x['leaf']} root {x['root']} tx {x['transactionHash']} block {x['blockNumber']}")
PY
  step init-bridge applied -- init-bridge || exit 1
  step register-eth applied -- register eth || exit 1
  echo "-- the public claim (deposit 0, alice), attested against the Sepolia checkpoint"
  step claim-alice applied -- claim 0 || exit 1
  G="$(jget "$LAST" precheck.gas)"
  # A refusal is sent with the honest claim's gas plus 10%, so that it is the program's.
  python3 - "$RUN/state/network.json" "$G" <<'PY'
import json, sys
j = json.load(open(sys.argv[1])); j["honest_claim"] = int(int(sys.argv[2]) * 1.1); json.dump(j, open(sys.argv[1], "w"), indent=1)
PY
  check "alice holds 0.001 wrapped ETH" "$(jget "$("$CLI" balance alice eth)" balance)" 1000000000000000
  echo "-- refusals: the same deposit again, a root the vault never had"
  step claim-alice-replay E7402 -- claim 0
  step claim-phantom-unknown-root E7208 -- claim 0 --phantom
  echo "-- the private claim (deposit 1, carol's shielded account), real proving"
  unset RISC0_DEV_MODE
  TIMEF="$RUN/private-time.txt"
  line="$(${HEAVY:+"$HEAVY"} /usr/bin/time -l -o "$TIMEF" "$CLI" --label claim-carol-private --wait 900 claim 1 --private 2>"$RUN/private.err" | grep '^{' | tail -1)"
  [ -n "$line" ] || line="$(python3 -c 'import json,sys;print(json.dumps({"verdict":"error","error":sys.stdin.read().strip()[-400:]}))' < "$RUN/private.err")"
  echo "$line"
  CAROL="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["carol_private"])' "$RUN/state/accounts.json")"
  RSS="$(awk '/maximum resident set size/ {print $1}' "$TIMEF" | head -1)"; WALL="$(awk '/ real / {print $1}' "$TIMEF" | head -1)"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' claim-carol-private applied "$(jget "$line" verdict)" "$(jget "$line" block)" "$(jget "$line" tx)" \
    "$(attempts_of "$line")" private - - >> "$OUT"
  echo "# line claim-carol-private $line" >> "$OUT"
  echo "# private claim-carol-private hides $CAROL tx_bytes $(jget "$line" tx_bytes)" >> "$OUT"
  echo "# private-claim prove_and_submit_ms $(jget "$line" prove_and_submit_ms) wall_s ${WALL:-?} peak_rss_bytes ${RSS:-?} tx_bytes $(jget "$line" tx_bytes) kind $(jget "$line" kind) dev_mode $(jget "$line" dev_mode)" >> "$OUT"
  check "the private claim landed" "$(jget "$line" verdict)" applied
  check "a privacy-preserving transaction" "$(jget "$line" kind)" privacy-preserving
  check "RISC0 dev mode off (real proving)" "$(jget "$line" dev_mode)" False
  check "the opening is absent from the transaction" "$(python3 -c 'import json,sys;a=json.loads(sys.argv[1]).get("absent_from_tx") or {};n=a.get("path_nodes","0 of 1").split(" of ");print(all(a.get(k) for k in ("destination","secret","commitment","leaf")) and n[0]==n[1])' "$line")" True
  SP="$("$CLI" show-private 2>/dev/null | grep '^{' | tail -1)"
  echo "# show-private $SP" >> "$OUT"
  check "carol decrypts 0.001 wrapped ETH in her private account" "$(jget "$SP" wrapped_balance)" 1000000000000000
  check "her note's commitment is in the chain's commitment set" "$(jget "$SP" commitment_on_chain)" True
  check "the private account is not public: no token shard on chain under its id" \
    "$(rpc getAccount "[\"$CAROL\"]" | python3 -c 'import json,sys;r=json.load(sys.stdin).get("result") or {};print(len((r.get("data") or {}).get("shards") or {}))')" 0
  check "wrapped ETH supply: both deposits, minted once each" "$(jget "$("$CLI" balance alice eth)" supply)" 2000000000000000
  echo "== final state"
  IDS="$("$CLI" ids)"
  python3 - "$RPC" "$IDS" "$RUN/state/programs.json" >> "$OUT" <<'PY'
import hashlib, json, sys, urllib.request
rpc, ids, progs = sys.argv[1], json.loads(sys.argv[2]), json.load(open(sys.argv[3]))
br, tok, lc = progs["wrap_bridge"], progs["token"], progs["sextant_lc"]
def shard(a, p):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getAccount", "params": [a]}).encode()
    r = json.load(urllib.request.urlopen(urllib.request.Request(rpc, body, {"Content-Type": "application/json"}), timeout=30))
    return bytes((((r.get("result") or {}).get("data") or {}).get("shards") or {}).get(p, []))
for name, p in [("sextant_header", lc), ("config", br), ("eth_entry", br), ("nullifier_0", br), ("nullifier_1", br),
                ("eth_definition", tok), ("alice_eth_ata", tok)]:
    b = shard(ids[name], p)
    print(f"# final {ids[name]} {p} {len(b)} {hashlib.sha256(b).hexdigest()} {name}")
PY
  grep '^# final' "$OUT" | awk '{print "  " $6 ": " $5 " bytes"}'
  check "both deposits' nullifiers are spent" "$(grep -E ' nullifier_[01]$' "$OUT" | awk '{print $5}' | tr '\n' ' ')" "1 1 "
  echo "# payer balance_after $(balance)" >> "$OUT"
  applied=$(awk -F'\t' '!/^#/ && $2=="applied" && $3=="applied"' "$OUT" | wc -l | tr -d ' ')
  refused=$(awk -F'\t' '!/^#/ && $2=="refused" && $3=="reverted"' "$OUT" | wc -l | tr -d ' ')
  echo "== $( [ $fail = 0 ] && echo PASS || echo FAIL ): $(grep -cv '^#' "$OUT") LEZ transactions ($applied applied, $refused refused as expected), $(grep -c '^# check ok' "$OUT") state checks ok"
  ;;
*) echo "unknown stage $STAGE"; exit 2 ;;
esac
exit $fail

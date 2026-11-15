#!/usr/bin/env bash
# The outbound path on public networks: wrapped Sepolia Ether, minted on the
# public LEZ testnet by the inbound run (docs/TESTNET.md), burned there, and
# released by the WrapVault deployed on Sepolia on 2026-11-08 to a fresh
# Ethereum address the burn committed to.
#
#   scripts/testnet-outbound.sh burn      alice's public Burn (and, PRIVATE=1, carol's
#                                         privacy-preserving BurnFrom from her shielded account)
#   scripts/testnet-outbound.sh release   for each burn: the committee members read the burn
#                                         tree's root from the LEZ testnet, wait FINALITY_DEPTH
#                                         blocks, check the final block's inscription on Bedrock
#                                         (BEDROCK_NODE), sign; the relayer releases on Sepolia;
#                                         refusals (not final, one member, a replay); final state
#
# The deployed vault's verifier is fixed at construction (Security #8): it is
# the CommitteeBurnVerifier deployed with it, 2 of 3, finality depth 30 LEZ
# blocks. The release therefore opens its burn leaf in public, and the burn
# tree's root is the committee's attestation. What the members check before
# signing is in scripts/sepolia_release.py; what is trusted is in
# docs/TESTNET.md.
#
# Every LEZ transaction is pre-checked locally first (the client's pre-check)
# and sent with its measured gas plus 1% and 10,000. Every Sepolia release is
# pre-checked with eth_call and sent with an explicit gas limit, so a refused
# one is mined and its revert is the recorded verdict.
#
# Needs:
#   RUN             a copy of the inbound run's directory (state, wallet, keys), never committed;
#                   state/anvil/deployment.json names the recipients (alice, carol)
#   LEE_WALLET_HOME_DIR  the v0.3.0 wallet home holding the payer's and the owners' keys
#   PAYER           the LEZ account that pays every fee
#   SEPOLIA_KEY_FILE  the relayer's key (the deployer, also committee member 0)
#   RUN/keys/committee.json  the other two members' keys
#   BEDROCK_NODE    optional: a Bedrock node following the public testnet; without it
#                   the members do not check the inscription, and say so in the manifest
#   cast, python3 (eth_account, pycryptodome), cli/target/release/wrap, r0vm 3.0.5 for PRIVATE=1
set -uo pipefail
cd "$(dirname "$0")/.."
STAGE="${1:?stage: burn | release}"
: "${RUN:?the outbound run directory}"
CLI="${CLI:-cli/target/release/wrap}"
RPC="${RPC:-https://testnet.lez.logos.co}"
OUT="${OUT:-evidence/testnet-outbound/run.tsv}"
INBOUND="${INBOUND:-evidence/testnet/run.tsv}"
HEAVY="${HEAVY:-}"; [ -x "$HEAVY" ] || HEAVY=""
export WRAP_RPC="$RPC" WRAP_RUN="$RUN/state" WRAP_ARTIFACTS="$RUN/artifacts" WRAP_PAYER="${PAYER:-}"
EVID="$(dirname "$OUT")"
mkdir -p "$EVID"
chmod 700 "$RUN"

jget() { python3 -c 'import json,sys
v=json.loads(sys.argv[1])
for k in sys.argv[2].split("."): v=(v or {}).get(k) if isinstance(v, dict) else None
print("" if v is None else v)' "$1" "$2"; }
rpc() { curl -s -m 30 -X POST "$RPC" -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}"; }
balance() { rpc getAccountBalance "[\"$PAYER\"]" | python3 -c 'import json,sys;print(json.load(sys.stdin)["result"])'; }
fail=0
check() { # label got want
  if [ "$2" = "$3" ]; then echo "  ok  $1: $2"; echo "# check ok: $1 = $2" >> "$OUT"
  else echo "  FAIL $1: got $2, want $3"; echo "# check FAIL: $1: $2 != $3" >> "$OUT"; fail=1; fi
}
attempts_of() { python3 -c 'import json,sys;print(",".join(json.loads(sys.argv[1]).get("attempts") or []) or "-")' "$1"; }
frontier() { # next_index root of the burn tree, as the chain holds it
  python3 - "$RPC" "$(jget "$("$CLI" ids)" burns)" "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["wrap_bridge"])' "$RUN/state/programs.json")" <<'PY'
import json, sys, urllib.request
rpc, acc, prog = sys.argv[1:4]
body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getAccount", "params": [acc]}).encode()
r = json.load(urllib.request.urlopen(urllib.request.Request(rpc, body, {"Content-Type": "application/json"}), timeout=30))
d = bytes((((r.get("result") or {}).get("data") or {}).get("shards") or {}).get(prog, []))
print(int.from_bytes(d[:8], "little") if d else 0, "0x" + d[-32:].hex() if d else "-")
PY
}

case "$STAGE" in
burn)
  : "${PAYER:?}"
  b="$(balance)"; [ "$b" -ge 3000000 ] 2>/dev/null || { echo "the payer holds $b, below the 3,000,000 a burn needs: stopping"; exit 1; }
  if [ ! -f "$OUT" ]; then
    {
      echo "# Portage outbound on public networks: LEZ $RPC (testnet), Sepolia (chain 11155111), $(date -u +%Y-%m-%dT%H:%M:%SZ)"
      # untracked files count too: a run's own new scripts are uncommitted changes
      echo "# commit $(git rev-parse --short HEAD)$([ -z "$(git status --porcelain -- . ':!evidence')" ] || echo ' (uncommitted changes)')"
      echo "# payer $PAYER balance_before $b"
      echo "# inbound $INBOUND"
      grep -E '^# (binary|program) ' "$INBOUND" | grep -E 'wrap_bridge|token|associated_token_account'
      grep -E '^# (accounts|private-account|network|sepolia vault) ' "$INBOUND"
    } > "$OUT"
    python3 - "$RUN/state/anvil/deployment.json" "$("$CLI" ids)" >> "$OUT" <<'PY'
import json, sys
d, ids = json.load(open(sys.argv[1])), json.loads(sys.argv[2])
print(f"# eth-vault {d['vault']} verifier {d['verifier']} committee {','.join(d['committee'])} threshold {d['threshold']} finality_depth {d['finality_depth']}")
print(f"# recipients alice {d['alice']} carol {d['carol']}")
print(f"# burns-account {ids['burns']}")
PY
  fi
  read -r n0 r0 <<< "$(frontier)"
  echo "-- the burn tree holds $n0 leaves (root $r0)"
  echo "# frontier before next_index $n0 root $r0" >> "$OUT"
  if ! grep -q $'^burn-alice\t' "$OUT"; then
    echo "-- alice's public burn of 0.001 wrapped ETH, to $(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["alice"])' "$RUN/state/anvil/deployment.json"), fee 0"
    line="$("$CLI" --label burn-alice --wait 600 burn alice eth 1000000000000000 --fee 0 2>"$RUN/burn.err" | grep '^{' | tail -1)"
    [ -n "$line" ] || line="$(python3 -c 'import json,sys;print(json.dumps({"verdict":"error","error":sys.stdin.read().strip()[-300:]}))' < "$RUN/burn.err")"
    echo "$line" | cut -c1-600
    pre="$(jget "$line" precheck.gas)"; [ -n "$pre" ] && pre="gas=$pre" || pre="$(jget "$line" precheck.refused | cut -c1-5)"
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' burn-alice applied "$(jget "$line" verdict)" "$(jget "$line" block)" "$(jget "$line" tx)" \
      "$(attempts_of "$line")" "${pre:--}" "$(jget "$line" gas_limit)" "$(jget "$line" block_gas_exec)" >> "$OUT"
    echo "# line burn-alice $line" >> "$OUT"
    check "alice's burn applied" "$(jget "$line" verdict)" applied
    check "alice holds no wrapped ETH" "$(jget "$("$CLI" balance alice eth)" balance)" 0
  fi
  if [ "${PRIVATE:-0}" = 1 ] && ! grep -q $'^burn-carol-private\t' "$OUT"; then
    echo "-- carol's privacy-preserving burn of 0.001 wrapped ETH from her shielded account, real proving"
    unset RISC0_DEV_MODE
    TIMEF="$RUN/private-burn-time.txt"
    line="$(${HEAVY:+"$HEAVY"} /usr/bin/time -l -o "$TIMEF" "$CLI" --label burn-carol-private --wait 900 burn carol eth 1000000000000000 --fee 0 --private 2>"$RUN/private-burn.err" | grep '^{' | tail -1)"
    [ -n "$line" ] || line="$(python3 -c 'import json,sys;print(json.dumps({"verdict":"error","error":sys.stdin.read().strip()[-400:]}))' < "$RUN/private-burn.err")"
    echo "$line" | cut -c1-600
    CAROL="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["carol_private"])' "$RUN/state/accounts.json")"
    RSS="$(awk '/maximum resident set size/ {print $1}' "$TIMEF" | head -1)"; WALL="$(awk '/ real / {print $1}' "$TIMEF" | head -1)"
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' burn-carol-private applied "$(jget "$line" verdict)" "$(jget "$line" block)" "$(jget "$line" tx)" \
      "$(attempts_of "$line")" private - - >> "$OUT"
    echo "# line burn-carol-private $line" >> "$OUT"
    echo "# private burn-carol-private hides $CAROL tx_bytes $(jget "$line" tx_bytes)" >> "$OUT"
    echo "# private-burn prove_and_submit_ms $(jget "$line" prove_and_submit_ms) wall_s ${WALL:-?} peak_rss_bytes ${RSS:-?} tx_bytes $(jget "$line" tx_bytes) dev_mode $(jget "$line" dev_mode)" >> "$OUT"
    check "carol's private burn landed" "$(jget "$line" verdict)" applied
    check "RISC0 dev mode off (real proving)" "$(jget "$line" dev_mode)" False
    SP="$("$CLI" show-private 2>/dev/null | grep '^{' | tail -1)"
    echo "# show-private $SP" >> "$OUT"
    check "carol's wallet decrypts 0 wrapped ETH in her private account" "$(jget "$SP" wrapped_balance)" 0
  fi
  read -r n1 r1 <<< "$(frontier)"
  echo "# frontier after next_index $n1 root $r1" >> "$OUT"
  check "the burn tree's leaves are the recorded burns'" "$n1" "$(python3 -c 'import json,sys;print(len(json.load(open(sys.argv[1]))))' "$RUN/state/burns.json")"
  echo "# payer balance_after_burns $(balance)" >> "$OUT"
  ;;

release)
  : "${SEPOLIA_KEY_FILE:?}"
  # Every burn's release runs at once (the members' finality wait is per release); the relayer's
  # transactions are serialized by a lock. REFUSALS_FOR names the owner whose release also
  # carries the refusals.
  pids=()
  for owner in $(python3 -c 'import json,sys;print(" ".join(b["owner"] for b in json.load(open(sys.argv[1]))))' "$RUN/state/burns.json"); do
    grep -q "^# eth release-$owner applied applied " "$OUT" && continue
    "$CLI" release-proof "$owner" > "$RUN/proof-$owner.json" || { echo "release-proof $owner failed"; exit 1; }
    ref=(); [ "${REFUSALS_FOR:-}" = "$owner" ] && ref=(--refusals)
    ( python3 -W ignore scripts/sepolia_release.py --lez "$RPC" --run "$RUN" --proof "$RUN/proof-$owner.json" --owner "$owner" --out "$OUT" \
        ${BEDROCK_NODE:+--bedrock-node "$BEDROCK_NODE"} ${BEDROCK_EVIDENCE:+--bedrock-evidence "$BEDROCK_EVIDENCE"} \
        ${ref[@]+"${ref[@]}"} 2>&1 | sed -u "s/^/[$owner] /"; exit "${PIPESTATUS[0]}" ) &
    pids+=($!)
    sleep 20
  done
  for p in ${pids[@]+"${pids[@]}"}; do wait "$p" || fail=1; done
  for owner in $(python3 -c 'import json,sys;print(" ".join(b["owner"] for b in json.load(open(sys.argv[1]))))' "$RUN/state/burns.json"); do
    grep -q "^# eth release-$owner applied applied " "$OUT" || { echo "  FAIL no applied release for $owner"; fail=1; }
  done
  python3 -W ignore scripts/sepolia_release.py --final --lez "$RPC" --run "$RUN" --out "$OUT" || fail=1
  echo "# payer balance_after $(balance)" >> "$OUT"
  ;;
*) echo "unknown stage $STAGE"; exit 2 ;;
esac
exit $fail

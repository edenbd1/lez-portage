#!/usr/bin/env bash
# The public re-checks (scripts/verify-testnet.sh, scripts/verify-outbound.sh)
# against evidence/archive/: every LEZ testnet and Sepolia JSON-RPC answer
# they read, recorded from the public endpoints while those still served the
# runs (scripts/rpc-archive.py). The LEZ testnet is reset when it moves to 0.4;
# after that, this is how the same verifiers, unchanged, re-check the runs.
# Every hash, proof and signature is still recomputed from the bytes; what the
# archive adds is only that the bytes are the ones the public endpoints served
# on the date in evidence/archive/RECORDED.
#
#   scripts/verify-archived.sh             re-check from the archive (offline)
#   scripts/verify-archived.sh --record    re-record it from the live endpoints
#   scripts/verify-archived.sh ... --bedrock-tools BIN   (passed to verify-outbound.sh)
set -uo pipefail
cd "$(dirname "$0")/.."
A=evidence/archive
MODE=serve; EXTRA=()
while [ $# -gt 0 ]; do
  case "$1" in
    --record) MODE=record; shift ;;
    --bedrock-tools) EXTRA=(--bedrock-tools "${2:?}"); shift 2 ;;
    *) echo "usage: $0 [--record] [--bedrock-tools BIN]" >&2; exit 2 ;;
  esac
done
LEZ_PORT="${LEZ_PORT:-8645}"; ETH_PORT="${ETH_PORT:-8646}"
PIDS=()
cleanup() { for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT
mkdir -p "$A"
# The one control that is a call rather than a lookup (verify-sepolia.py's
# knownRoot of a random root): drawn at random when recording, and replayed.
if [ $MODE = record ]; then
  export PORTAGE_CONTROL_ROOT="0x$(python3 -c 'import secrets;print(secrets.token_hex(32))')"
else
  export PORTAGE_CONTROL_ROOT="$(cat "$A/CONTROL_ROOT")"
fi
if [ $MODE = record ]; then
  T=$(mktemp -d)
  python3 scripts/rpc-archive.py record https://testnet.lez.logos.co "$T/lez.jsonl" "$LEZ_PORT" & PIDS+=($!)
  python3 scripts/rpc-archive.py record https://rpc.sepolia.ethpandaops.io "$T/sepolia.jsonl" "$ETH_PORT" & PIDS+=($!)
else
  python3 scripts/rpc-archive.py serve "$A/lez.jsonl.gz" "$LEZ_PORT" & PIDS+=($!)
  python3 scripts/rpc-archive.py serve "$A/sepolia.jsonl.gz" "$ETH_PORT" & PIDS+=($!)
fi
sleep 2
R=(--rpc "http://127.0.0.1:$LEZ_PORT" --eth-rpc "http://127.0.0.1:$ETH_PORT")
fail=0
echo "##### inbound (scripts/verify-testnet.sh)"
scripts/verify-testnet.sh "${R[@]}" || fail=1
echo "##### outbound (scripts/verify-outbound.sh)"
scripts/verify-outbound.sh "${R[@]}" ${EXTRA[@]+"${EXTRA[@]}"} || fail=1
if [ $MODE = record ]; then
  if [ $fail = 0 ]; then
    for f in lez sepolia; do sort -u "$T/$f.jsonl" | gzip -9n > "$A/$f.jsonl.gz"; done
    date -u +%Y-%m-%dT%H:%M:%SZ > "$A/RECORDED"
    echo "$PORTAGE_CONTROL_ROOT" > "$A/CONTROL_ROOT"
    echo "# recorded: $(zcat < $A/lez.jsonl.gz | wc -l | tr -d ' ') LEZ and $(zcat < $A/sepolia.jsonl.gz | wc -l | tr -d ' ') Sepolia answers"
  else
    echo "# not recorded: the live re-check failed" >&2
  fi
fi
echo "##### $( [ $fail = 0 ] && echo PASS || echo FAIL ) ($MODE)"
exit $fail

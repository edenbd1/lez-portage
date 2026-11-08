#!/usr/bin/env bash
# The v0.4 spike on a local LEZ v0.4.0-rc1 standalone sequencer (mock Bedrock)
# and a local anvil chain:
#   1. a sequencer home from LEZ's networks/local.json, its genesis supplying the
#      driver's fee payer (seq_drive pubkey), 2 s blocks, no gossip;
#   2. seq_drive deploy: token, sextant_attest, wrap_bridge through the program loader;
#   3. anvil: WrapVault with this deployment's pairing, two deposits committed to
#      alice (scripts/anvil_deposits.py, committee release verifier: no release here);
#   4. seq_drive run: Record, Init, Register, alice's holding, both claims, a replay
#      (reverted), four burns (two applied, two reverted), every block saved.
# Needs SEQ (the sequencer_service binary of logos-execution-zone v0.4.0-rc1, built
# with --features standalone) and LEZ (that checkout, for networks/local.json).
#   SEQ=... LEZ=... v04/scripts/e2e-seq.sh [RUN]
# Nothing here reaches a public network.
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${SEQ:?set SEQ to the v0.4.0-rc1 sequencer_service binary (standalone)}"
: "${LEZ:?set LEZ to the logos-execution-zone v0.4.0-rc1 checkout}"
RUN="${1:-${TMPDIR:-/tmp}/portage-v04-e2e}"
PORT="${PORT:-3077}"; METRICS_PORT="${METRICS_PORT:-9477}"; ANVIL_PORT="${ANVIL_PORT:-8577}"
rm -rf "$RUN"; mkdir -p "$RUN"
DRIVE=(cargo run -q --release -j "${JOBS:-4}" --manifest-path v04/executor-tests/Cargo.toml --bin seq_drive --)
PIDS=()
cleanup() { for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT

PAYER=$("${DRIVE[@]}" pubkey)
"$SEQ" setup "$RUN/seq" --config "$LEZ/lez/sequencer/service/networks/local.json" \
  --funding-public-key 2e03b2eff5a45478e7e79668d2a146cf2c5c7925bce927f2b1c67f2ab4fc0d26 --create-channel > "$RUN/setup.log"
python3 - "$RUN/seq/sequencer_config.json" "$PAYER" "$METRICS_PORT" <<'PY'
import json, sys
p, payer, metrics = sys.argv[1:]
c = json.load(open(p))
c["block_create_timeout"] = "2s"
c["metrics_listen_address"] = f"127.0.0.1:{metrics}"
c["gossip"] = None
c["genesis"]["actions"][0]["supply_account"]["recipient"] = {"identified": {"key": payer}}
json.dump(c, open(p, "w"), indent=1)
PY
RUST_LOG=info,risc0_zkvm=warn,kameo=warn "$SEQ" start "$RUN/seq" --rpc-listen-address "127.0.0.1:$PORT" > "$RUN/sequencer.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 60); do
  curl -sf -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"checkHealth","params":[]}' "http://127.0.0.1:$PORT" > /dev/null && break
  sleep 1
done
echo "# sequencer up on $PORT"
"${DRIVE[@]}" deploy "http://127.0.0.1:$PORT" "$RUN"

anvil --port "$ANVIL_PORT" --silent > "$RUN/anvil.log" 2>&1 &
PIDS+=($!)
sleep 2
python3 scripts/anvil_deposits.py --rpc "http://127.0.0.1:$ANVIL_PORT" --plan "$RUN/plan.json" --out "$RUN/anvil" --release-verifier committee

"${DRIVE[@]}" run "http://127.0.0.1:$PORT" "$RUN"
echo "# done: $RUN/manifest.tsv, $RUN/blocks/"

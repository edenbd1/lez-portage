#!/usr/bin/env bash
# A separate, single-node local Bedrock devnet for the Bedrock-anchored release:
# LEZ v0.3.1's own devnet configuration (bedrock/node-config.yaml and
# deployment-settings.yaml of the LEZ checkout: one validator holding all the
# stake, security parameter k = 5, active slot coefficient 1/2, 1 s slots,
# proofs of leadership in dev mode), run by a logos-blockchain-node 0.3.1
# binary, in its own directory, with its API on 127.0.0.1:$PORT and its
# network and blend ports moved off the defaults, so that it shares nothing
# with any other node on the machine (no peers, no keys, no state).
#
#   BEDROCK_BIN=<logos-blockchain-node 0.3.1> LEZ_SRC=<LEZ at 1a003bfb> DIR=<a new directory> scripts/bedrock-devnet.sh
#
# Returns once the node has passed WARMUP_EPOCHS (3) epoch transitions, about
# 5 minutes (see below why). Prints the node's pid; stop it with `kill <pid>`. A LEZ sequencer built
# without `standalone` and pointed at http://127.0.0.1:$PORT creates its
# channel there and inscribes each block (scripts/e2e-local.sh, BEDROCK_NODE).
set -euo pipefail
: "${BEDROCK_BIN:?a logos-blockchain-node 0.3.1 binary}"
: "${LEZ_SRC:?a LEZ checkout at 1a003bfb}"
: "${DIR:?a directory for the devnet}"
PORT="${PORT:-18480}"; SWARM_PORT="${SWARM_PORT:-3700}"; BLEND_PORT="${BLEND_PORT:-3710}"
[ -e "$DIR/state" ] && { echo "$DIR already holds a devnet's state: choose a new DIR"; exit 2; }
(exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null && { echo "port $PORT is in use"; exit 2; }
mkdir -p "$DIR"
cp "$BEDROCK_BIN" "$DIR/logos-blockchain-node"
cp "$LEZ_SRC/bedrock/deployment-settings.yaml" "$LEZ_SRC/bedrock/kzgrs_test_params" "$DIR/"
python3 - "$LEZ_SRC/bedrock/node-config.yaml" "$DIR/node-config.yaml" "$PORT" "$SWARM_PORT" "$BLEND_PORT" <<'PY'
import sys
src, dst, port, swarm, blend = sys.argv[1:]
t = open(src).read()
assert "listen_address: 0.0.0.0:18080" in t and "  core:\n    zk:" in t
t = t.replace("listen_address: 0.0.0.0:18080", f"listen_address: 127.0.0.1:{port}")
t = t.replace("  core:\n    zk:", f"  core:\n    backend:\n      listening_address: /ip4/127.0.0.1/udp/{blend}/quic-v1\n    zk:", 1)
t = f"network:\n  backend:\n    swarm:\n      host: 127.0.0.1\n      port: {swarm}\n" + t
open(dst, "w").write(t)
PY
cd "$DIR"
"./logos-blockchain-node" node-config.yaml --deployment deployment-settings.yaml --check-config > /dev/null
POL_PROOF_DEV_MODE=true nohup ./logos-blockchain-node node-config.yaml --deployment deployment-settings.yaml > node.out 2>&1 &
echo $! > node.pid
for _ in $(seq 60); do curl -s -m 2 "127.0.0.1:$PORT/cryptarchia/info" | grep -q '"tip"' && break; sleep 1; done
curl -s -m 2 "127.0.0.1:$PORT/cryptarchia/info" | grep -q '"tip"' || { echo "the devnet node did not answer"; tail -20 node.out; exit 1; }
# The devnet's genesis is slot 0 of a fixed past time, so the node starts by
# skipping some 90,000 epochs at once. A channel that a sequencer opens inside
# that first epoch, and fills with the large program-deployment blocks, does not
# survive the first epoch transition: from it on, the node drops every one of the
# sequencer's inscriptions from its mempool (each chains on the dropped one), and
# scripts/e2e-bedrock.sh later finds nothing to anchor. So hand the node over
# only after WARMUP_EPOCHS (3) epoch transitions: 100 slots each (k = 5, f = 1/2,
# 3 + 3 + 4 stabilization periods), 1 s slots, about 5 minutes.
WARMUP_EPOCHS="${WARMUP_EPOCHS:-3}"
slot() { curl -s -m 2 "127.0.0.1:$PORT/cryptarchia/info" | python3 -c 'import json,sys;print(json.load(sys.stdin)["cryptarchia_info"]["slot"])' 2>/dev/null; }
if [ "$WARMUP_EPOCHS" -gt 0 ]; then
  # The tip is at slot 0 until the node proposes its first block (after the skip).
  for _ in $(seq 60); do s0="$(slot)"; [ -n "$s0" ] && [ "$s0" -gt 0 ] && break; sleep 1; done
  [ -n "$s0" ] && [ "$s0" -gt 0 ] || { echo "the devnet node made no block"; tail -20 node.out; exit 1; }
  target=$(( (s0 / 100 + WARMUP_EPOCHS) * 100 ))
  echo "waiting for $WARMUP_EPOCHS epoch transitions (slot $s0 to $target, about $(( target - s0 )) s) before a sequencer may use the node"
  for _ in $(seq $(( (target - s0) / 5 + 60 ))); do
    s="$(slot)"; [ -n "$s" ] && [ "$s" -ge "$target" ] && break
    kill -0 "$(cat node.pid)" 2>/dev/null || { echo "the devnet node exited"; tail -20 node.out; exit 1; }
    sleep 5
  done
  s="$(slot)"; [ -n "$s" ] && [ "$s" -ge "$target" ] || { echo "the devnet node did not reach slot $target (at ${s:-?})"; tail -20 node.out; exit 1; }
fi
echo "devnet node pid $(cat node.pid), API http://127.0.0.1:$PORT, $("./logos-blockchain-node" --version | head -1)"

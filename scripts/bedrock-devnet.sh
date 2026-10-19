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
# Prints the node's pid; stop it with `kill <pid>`. A LEZ sequencer built
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
echo "devnet node pid $(cat node.pid), API http://127.0.0.1:$PORT, $("./logos-blockchain-node" --version | head -1)"

#!/usr/bin/env bash
# The v0.4 port on a local LEZ v0.4.0-rc3 standalone sequencer (mock Bedrock)
# and a local anvil chain, driven end to end by the v0.4 client `wrap`
# (v04/executor-tests/src/bin/wrap.rs), every transaction pre-checked on rc3's
# state machine and its verdict read back from the block's `reverted` list:
#   1. a sequencer home from LEZ's networks/local.json, its genesis funding the
#      client's fee payer (`wrap pubkey`), 2 s blocks, no gossip;
#   2. `wrap deploy`: token, sextant_lc, wrap_bridge through the program loader;
#      `wrap plan`: the pairing and four deposits (alice: 1 ETH twice; bob: 100
#      and 1,000 USDT, the ERC-20);
#   3. anvil: WrapVault with this deployment's pairing, the USDT-shaped ERC-20,
#      the deposits (scripts/anvil_deposits.py, committee release verifier);
#   4. Sextant's Init on a devnet beacon header committing to the anvil block,
#      the bridge's Init, ETH and USDT registered, holdings, the four claims
#      (each attested by Sextant), refused claims (replays, a root the vault
#      never had, redirected, the wrong token), burns of ETH and USDT (applied,
#      and refused: not a denomination, more than held, the wrong token), the
#      release openings of the applied burns, every block saved.
# The sequencer runs with RISC0_DEV_MODE=0.
# Needs SEQ (the sequencer_service binary of logos-execution-zone v0.4.0-rc3, built
# with --features standalone) and LEZ (that checkout, for networks/local.json).
#   SEQ=... LEZ=... v04/scripts/e2e-seq.sh [RUN]
# Then: python3 scripts/v04-run-json.py RUN vectors/lez-blocks-v04
# Nothing here reaches a public network.
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${SEQ:?set SEQ to the v0.4.0-rc3 sequencer_service binary (standalone)}"
: "${LEZ:?set LEZ to the logos-execution-zone v0.4.0-rc3 checkout}"
RUN="${1:-${TMPDIR:-/tmp}/portage-v04-e2e}"
PORT="${PORT:-3077}"; METRICS_PORT="${METRICS_PORT:-9477}"; ANVIL_PORT="${ANVIL_PORT:-8577}"
export RISC0_DEV_MODE=0
rm -rf "$RUN"; mkdir -p "$RUN"
cargo build -q --release -j "${JOBS:-4}" --manifest-path v04/executor-tests/Cargo.toml --bin wrap
WRAP=v04/executor-tests/target/release/wrap
export WRAP_RPC="http://127.0.0.1:$PORT" WRAP_RUN="$RUN"
PIDS=()
cleanup() { for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT

PAYER=$("$WRAP" pubkey)
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
"$WRAP" deploy
"$WRAP" plan

anvil --port "$ANVIL_PORT" --silent > "$RUN/anvil.log" 2>&1 &
PIDS+=($!)
sleep 2
python3 scripts/anvil_deposits.py --rpc "http://127.0.0.1:$ANVIL_PORT" --plan "$RUN/plan.json" --out "$RUN/anvil" --release-verifier committee

FIRST=$(( $("$WRAP" tip) + 1 ))
"$WRAP" init-sextant
"$WRAP" init-bridge
"$WRAP" register eth
"$WRAP" register usdt
"$WRAP" open-holding alice eth
"$WRAP" open-holding bob usdt
# Claims, relayed by the payer: the four deposits, then refusals sent anyway (--force).
"$WRAP" claim 0
"$WRAP" claim 2
"$WRAP" --force --label "Claim deposit 0 again (replay)" claim 0
"$WRAP" --force --label "Claim deposit 2 again under an earlier root (replay)" claim 2 --root-after 3
"$WRAP" --force claim --phantom
"$WRAP" --force --label "Claim deposit 3 redirected to alice" claim 3 --redirect-to alice
"$WRAP" --force --label "Claim deposit 3 as ETH (the wrong token)" claim 3 --as-token eth
"$WRAP" claim 1
"$WRAP" claim 3
"$WRAP" --force --label "claim --private (refused by the client on v0.4)" claim 1 --private 2> "$RUN/private-claim.txt" && exit 1 || true
# Burns: ETH by alice, USDT by bob; refusals sent anyway.
"$WRAP" burn alice eth 100000000000000000
"$WRAP" burn bob usdt 100000000
"$WRAP" --force --label "bob Burn 12345 USDT units (not a denomination)" burn bob usdt 12345
"$WRAP" --force --label "alice Burn 100 USDT from her wrapped ETH (the wrong token)" burn alice usdt 100000000
"$WRAP" burn bob usdt 1000000000
"$WRAP" --force --label "bob Burn 1000 USDT (more than bob holds)" burn bob usdt 1000000000
"$WRAP" burn alice eth 1000000000000000000
for w in alice bob; do for t in eth usdt; do "$WRAP" balance $w $t; done; done | tee "$RUN/balances.jsonl"
{ "$WRAP" release-proof alice; "$WRAP" release-proof bob; "$WRAP" release-proof bob --n 1; } > "$RUN/release-openings.jsonl"
"$WRAP" finish --from "$FIRST"
echo "# done: $RUN/manifest.tsv, $RUN/blocks/"

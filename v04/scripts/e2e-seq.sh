#!/usr/bin/env bash
# The v0.4 port on a local LEZ v0.4.0-rc3 standalone sequencer (mock Bedrock)
# and a local anvil chain, driven end to end by the v0.4 client `wrap`
# (v04/executor-tests/src/bin/wrap.rs), every transaction pre-checked on rc3's
# state machine and its verdict read back from the block's `reverted` list:
#   1. a sequencer home from LEZ's networks/local.json, its genesis funding the
#      client's fee payer (`wrap pubkey`), 2 s blocks, no gossip;
#   2. `wrap deploy`: token, ata, sextant_lc, wrap_bridge through the program
#      loader; `wrap plan`: the pairing and four deposits (alice: 1 ETH twice and
#      1,000 USDT, the ERC-20; bob: 100 USDT);
#   3. anvil: WrapVault with this deployment's pairing, the USDT-shaped ERC-20,
#      the deposits (scripts/anvil_deposits.py, committee release verifier);
#   4. Sextant's Init on a devnet beacon header committing to the anvil block,
#      the bridge's Init, ETH and USDT registered, bob's ATA holding opened by
#      the payer, the four claims (each attested by Sextant; alice's two
#      holdings, wrapped ETH and wrapped USDT, created by her claims), refused claims (replays, a root the vault
#      never had, redirected, the wrong token), burns of ETH and USDT (applied,
#      and refused: not a denomination, more than held, the wrong token), the
#      release openings of the applied burns, every block saved;
#   5. anvil: every applied burn released from WrapVault through the committee
#      verifier (each member reads the burn tree's root from this sequencer),
#      and the release refusals (not final, replay, redirected, another amount,
#      a burn that never happened, one signature), releases.tsv;
#   and between them the private claim of carol's deposit (4) into her private
#   account: one privacy-preserving transaction, its fee paid in-proof by one
#   of networks/local.json's genesis-funded private accounts (private-claim.json:
#   proving time, peak memory, size, fee, carol's decrypted holding).
# The sequencer and the prover run with RISC0_DEV_MODE=0 unless set.
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
# RISC0_DEV_MODE=1 (sequencer and prover) checks the flow with fake receipts in minutes.
export RISC0_DEV_MODE="${RISC0_DEV_MODE:-0}"
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
"$WRAP" open-holding bob usdt
# Claims, relayed by the payer: the four deposits, then refusals sent anyway (--force).
"$WRAP" claim 0
"$WRAP" claim 2
"$WRAP" --force --label "Claim deposit 0 again (replay)" claim 0
"$WRAP" --force --label "Claim deposit 2 again under an earlier root (replay)" claim 2 --root-after 3
"$WRAP" --force claim --phantom
"$WRAP" --force --label "Claim deposit 3 redirected to bob" claim 3 --redirect-to bob
"$WRAP" --force --label "Claim deposit 3 as ETH (the wrong token)" claim 3 --as-token eth
"$WRAP" claim 1
"$WRAP" claim 3
# The private claim: carol's deposit (4) into her private account, one
# privacy-preserving transaction proven here (real proving unless
# RISC0_DEV_MODE=1), its fee paid in-proof by a private payer funded at genesis.
/usr/bin/time -l "$WRAP" claim 4 --private 2> "$RUN/private-claim-time.txt" | tee "$RUN/private-claim.txt"
"$WRAP" --label "claim deposit 1 --private (refused: committed to a public account)" claim 1 --private 2> "$RUN/private-claim-refused.txt" && exit 1 || true
# Burns: ETH and USDT by alice (one account, two wrapped tokens), USDT by bob; refusals sent anyway.
"$WRAP" burn alice eth 100000000000000000
"$WRAP" burn bob usdt 100000000
"$WRAP" --force --label "bob Burn 12345 USDT units (not a denomination)" burn bob usdt 12345
"$WRAP" --force --label "bob Burn 0.1 ETH (bob holds no wrapped ETH)" burn bob eth 100000000000000000
"$WRAP" burn alice usdt 1000000000
"$WRAP" --force --label "bob Burn 1000 USDT (more than bob holds)" burn bob usdt 1000000000
"$WRAP" burn alice eth 1000000000000000000
# The private burn: 0.1 of carol's private wrapped ETH, a public root call to
# the burn tree naming a fresh alias of her, her holding updated privately
# (real proving unless RISC0_DEV_MODE=1), the fee paid in-proof by the other
# genesis-funded private payer.
/usr/bin/time -l "$WRAP" burn carol eth 100000000000000000 --private 2> "$RUN/private-burn-time.txt" | tee "$RUN/private-burn.txt"
for w in alice bob; do for t in eth usdt; do "$WRAP" balance $w $t; done; done | tee "$RUN/balances.jsonl"
python3 -c 'import json,sys;print(json.dumps({"owner":"carol","token":"eth","private_balance":json.load(open(sys.argv[1]))["carol_holding_after"]["balance"]}))' "$RUN/private-burn.json" | tee -a "$RUN/balances.jsonl"
{ "$WRAP" release-proof alice; "$WRAP" release-proof alice --n 1; "$WRAP" release-proof alice --n 2; "$WRAP" release-proof bob; "$WRAP" release-proof carol; } > "$RUN/release-openings.jsonl"
"$WRAP" finish --from "$FIRST"

# 5. The releases on anvil, against the LEZ burn tree, through CommitteeBurnVerifier
# (anvil accounts 5, 6, 7; 2 of 3; each member reads the burn tree's root from
# this sequencer itself and signs the root it read). Every applied burn is
# released once (ETH and USDT to alice, USDT to bob by a relayer); the refusals are
# mined too (scripts/anvil_release.py), each verdict equal to its eth_call.
ANVIL="http://127.0.0.1:$ANVIL_PORT"
DEP="$RUN/anvil/deployment.json"
REL="$RUN/releases.tsv"
dep() { python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$DEP" "$1"; }
VAULT=$(dep vault); USDT=$(dep usdt)
echo "# caveat-release the committee attests the burn tree's root as each member reads it from the local rc3 sequencer; 'final' is that standalone sequencer's tip, $(dep finality_depth) blocks above the block read: there is no Bedrock in this run" > "$REL"
"$WRAP" release-proof alice > "$RUN/release-alice-0.json"          # 0.1 ETH
"$WRAP" release-proof alice --n 1 > "$RUN/release-alice-1.json"    # 1,000 USDT
"$WRAP" release-proof alice --n 2 > "$RUN/release-alice-2.json"    # 1 ETH
"$WRAP" release-proof bob > "$RUN/release-bob-0.json"              # 100 USDT
"$WRAP" release-proof carol > "$RUN/release-carol-0.json"          # 0.1 ETH, carol's private burn
rel() { # label expected proof -- args
  local label="$1" expect="$2" proof="$3"; shift 4
  local out
  out=$(python3 scripts/anvil_release.py --anvil "$ANVIL" --lez "$WRAP_RPC" --deployment "$DEP" --out "$REL" \
        --label "$label" --expect "$expect" --proof "$RUN/release-$proof.json" "$@" 2>&1 | tail -1) || true
  python3 - "$out" <<'PY' || { echo "release $label: $out"; exit 1; }
import json, sys
r = json.loads(sys.argv[1])
print(f"release {r['label']}: {r['verdict']} ({r['precheck']}), anvil block {r['block']}, gas {r['gas_used']}, "
      f"LEZ root {r['root_seen'][:10]} read at block {r['lez_block']}, final {r['lez_final_block']}")
sys.exit(0 if r["ok"] else 1)
PY
}
erc() { cast call "$USDT" 'balanceOf(address)(uint256)' "$1" --rpc-url "$ANVIL" | cut -d' ' -f1; }
V_ETH0=$(cast balance "$VAULT" --rpc-url "$ANVIL"); V_USDT0=$(erc "$VAULT"); B_USDT0=$(erc "$(dep bob)"); A_USDT0=$(erc "$(dep alice)")
rel release-alice-0.1eth-not-final NotFinal alice-0 --
rel release-alice-0.1eth applied alice-0 -- --wait-final
rel release-alice-0.1eth-replay AlreadyReleased alice-0 -- --wait-final
rel release-bob-100usdt-redirected-to-relayer RootNotAttested bob-0 -- --sender relayer --recipient-as-sender --wait-final
rel release-bob-100usdt-as-1000usdt RootNotAttested bob-0 -- --sender relayer --amount 1000000000 --wait-final
rel release-forged-burn-committee RootNotAttested bob-0 -- --sender outsider --forge --wait-final
rel release-forged-burn-outsiders RootNotAttested bob-0 -- --sender outsider --forge --outsiders --wait-final
rel release-bob-100usdt-one-member RootNotAttested bob-0 -- --sender relayer --members 1 --wait-final
rel release-bob-100usdt-by-relayer applied bob-0 -- --sender relayer --wait-final
rel release-alice-1000usdt applied alice-1 -- --wait-final
rel release-alice-1eth applied alice-2 -- --wait-final
rel release-carol-0.1eth-private-burn-by-relayer applied carol-0 -- --sender relayer --wait-final
V_ETH1=$(cast balance "$VAULT" --rpc-url "$ANVIL"); V_USDT1=$(erc "$VAULT"); B_USDT1=$(erc "$(dep bob)"); A_USDT1=$(erc "$(dep alice)")
{
  echo "# eth-final vault ETH $V_ETH0 -> $V_ETH1 (released $(python3 -c "print($V_ETH0 - $V_ETH1)") wei)"
  echo "# eth-final vault USDT $V_USDT0 -> $V_USDT1 (released $(python3 -c "print($V_USDT0 - $V_USDT1)") units); alice USDT +$(python3 -c "print($A_USDT1 - $A_USDT0)"), bob USDT +$(python3 -c "print($B_USDT1 - $B_USDT0)")"
  for f in alice-0 alice-1 alice-2 bob-0 carol-0; do
    nf=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["nullifier"])' "$RUN/release-$f.json")
    echo "# eth-final nullifier $nf $(cast call "$VAULT" 'nullifierSpent(bytes32)(bool)' "$nf" --rpc-url "$ANVIL") $f"
  done
} >> "$REL"
[ "$(python3 -c "print($V_ETH0 - $V_ETH1)")" = 1200000000000000000 ] || { echo "the vault did not pay out exactly 1.2 ETH"; exit 1; }
[ "$(python3 -c "print($V_USDT0 - $V_USDT1)")" = 1100000000 ] && [ "$(python3 -c "print($A_USDT1 - $A_USDT0)")" = 1000000000 ] && [ "$(python3 -c "print($B_USDT1 - $B_USDT0)")" = 100000000 ] \
  || { echo "the vault did not pay exactly 1,000 USDT to alice and 100 USDT to bob"; exit 1; }
[ "$(grep -c '^# eth-final nullifier .* true ' "$REL")" = 5 ] || { echo "not every released burn's nullifier is spent"; exit 1; }
grep -c '^# eth ' "$REL" | xargs echo "# releases:"
echo "# done: $RUN/manifest.tsv, $RUN/releases.tsv, $RUN/blocks/"

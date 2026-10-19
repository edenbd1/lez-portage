#!/usr/bin/env bash
# The Groth16 fixture of vault/test/ZkBurnVerifier.t.sol, made by the real
# prover: the test's burn tree (scripts/zk_fixture_input.py), proven by the
# committed guest artifacts/zk/burn_membership.bin to a Groth16 receipt
# (scripts/zk-prove-measured.sh: real proving, the Groth16 step in RISC
# Zero's x86_64 Docker image). Writes vault/test/fixtures/zk_release.json (the
# prover's public record: journal, seal, image id, timings; not the opening)
# and the measurement to docs/raw/zk-prove-fixture-<UTC>.txt.
set -euo pipefail
cd "$(dirname "$0")/.."
IN="$(mktemp /tmp/rfp022-zk-input.XXXXXX)"
python3 scripts/zk_fixture_input.py > "$IN"
mkdir -p vault/test/fixtures
scripts/zk-prove-measured.sh vault/test/fixtures/zk_release.json "docs/raw/zk-prove-fixture-$(date -u +%Y%m%dT%H%MZ).txt" -- zk-prove "$IN"
rm -f "$IN"
python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));print("fixture", d["image_id"], d["fields"]["root"], d["seal_bytes"], "bytes", d["stats"])' vault/test/fixtures/zk_release.json

#!/usr/bin/env bash
# Measure, in r0vm 3.0.5, one Bedrock Proof of Leadership verification and the
# per-note ledger update a Cryptarchia light client needs (docs/POL-MEASURED.md).
#   scripts/pol-measure.sh            build, generate the jobs, execute them all
#   scripts/pol-measure.sh --prove    also prove pol-bn-1 (one accelerated PoL
#                                     verification) and notes-bigint2, timed
# The host tool builds its jobs with the Bedrock node's own code at 3926a532
# and checks the measured implementations against it natively first; the
# guest is built in RISC Zero's pinned builder image (r0.1.91.1).
# Outputs: evidence/pol-measure/{vectors.json,<job>.json,run.txt}.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=evidence/pol-measure
JOBS="$(mktemp -d /tmp/pol-measure.XXXXXX)"
HEAVY="${HEAVY:-}"; [ -x "$HEAVY" ] || HEAVY=""
mkdir -p "$OUT"
H=tools/pol-measure/host/target/release/pol-measure
G=tools/pol-measure/guest/target/riscv32im-risc0-zkvm-elf/docker/pol_measure.bin
${HEAVY:+"$HEAVY"} cargo build --release -j4 --manifest-path tools/pol-measure/host/Cargo.toml
${HEAVY:+"$HEAVY"} env RISC0_DOCKER_CONTAINER_TAG=r0.1.91.1 cargo risczero build --manifest-path tools/pol-measure/guest/Cargo.toml
"$H" gen vectors/bedrock/testnet-lez-36401.json "$JOBS"
cp "$JOBS/vectors.json" "$OUT/vectors.json"
unset RISC0_DEV_MODE
{
  echo "# $(date -u +%FT%TZ) r0vm $(r0vm --version | awk '{print $2}'), guest sha256 $(shasum -a 256 "$G" | cut -d' ' -f1)"
  if [ "$(uname)" = Darwin ]; then
    echo "# host $(sysctl -n machdep.cpu.brand_string) hw.memsize=$(sysctl -n hw.memsize) hw.ncpu=$(sysctl -n hw.ncpu)"
  else
    echo "# host $(uname -m) $(nproc) cpus"
  fi
  echo "# commit $(git rev-parse --short HEAD)$(git diff --quiet -- tools/pol-measure || echo ' (uncommitted tools/pol-measure changes)')"
  echo "# load before: $(uptime | sed 's/.*load/load/')"
} > "$OUT/run.txt"
for j in pol-ark-1 pol-bn-1 pol-ark pol-bn notes-ark notes-bn notes-bigint2; do
  ${HEAVY:+"$HEAVY"} "$H" run "$G" "$JOBS/$j.job" > "$OUT/$j.json"
  echo "$j: $(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["user_cycles"], "user cycles,", d["segments"], "segments")' "$OUT/$j.json")" | tee -a "$OUT/run.txt"
done
if [ "${1:-}" = --prove ]; then
  for j in pol-bn-1 notes-bigint2; do
    echo "# prove $j, load before: $(uptime | sed 's/.*load/load/')" >> "$OUT/run.txt"
    ${HEAVY:+"$HEAVY"} /usr/bin/time -l "$H" run "$G" "$JOBS/$j.job" --prove > "$OUT/$j.prove.json" 2>> "$OUT/run.txt"
    echo "$j proven: $(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["total_cycles"], "total cycles,", round(d["run"]["prove_seconds"],1), "s")' "$OUT/$j.prove.json")" | tee -a "$OUT/run.txt"
  done
fi
echo "# load after: $(uptime | sed 's/.*load/load/')" >> "$OUT/run.txt"
rm -rf "$JOBS"

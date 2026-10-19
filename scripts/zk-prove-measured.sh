#!/usr/bin/env bash
# One zero-knowledge release proof, measured: a `wrap` command that proves
# (`wrap zk-prove INPUT`, or `wrap release-proof OWNER --zk ...`: the guest
# zk/burn_membership, a succinct STARK through r0vm, then its Groth16
# compression in RISC Zero's x86_64 Docker image, emulated on Apple Silicon
# with DOCKER_DEFAULT_PLATFORM=linux/amd64), under $HEAVY
# and /usr/bin/time (the host's peak resident set: wrap and r0vm), with the
# Groth16 container's memory sampled by `docker stats` every ~2 s.
#   scripts/zk-prove-measured.sh OUT.json RAW.txt -- wrap-args...
# OUT.json is the command's last JSON line (journal, seal, timings); RAW.txt
# the machine, load, time(1) output and container samples.
set -uo pipefail
cd "$(dirname "$0")/.."
OUT="${1:?OUT.json}"; RAW="${2:?RAW.txt}"; shift 2; [ "${1:-}" = -- ] && shift
CLI="${CLI:-cli/target/release/wrap}"
HEAVY="${HEAVY:-}"; [ -x "$HEAVY" ] || HEAVY=""
unset RISC0_DEV_MODE
export DOCKER_DEFAULT_PLATFORM="${DOCKER_DEFAULT_PLATFORM:-linux/amd64}"
export RISC0_WORK_DIR="${RISC0_WORK_DIR:-$(mktemp -d /tmp/rfp022-groth16.XXXXXX)}"
mkdir -p "$(dirname "$RAW")" "$(dirname "$OUT")"
{
  echo "# $(date -u +%FT%TZ) wrap $*, real proving (RISC0_DEV_MODE unset), r0vm $(r0vm --version | awk '{print $2}')"
  if [ "$(uname)" = Darwin ]; then
    echo "# host $(sysctl -n machdep.cpu.brand_string) hw.memsize=$(sysctl -n hw.memsize) hw.ncpu=$(sysctl -n hw.ncpu)"
  else
    echo "# host $(uname -m) $(nproc) cpus $(awk '/MemTotal/ {print $2 " kB"}' /proc/meminfo)"
  fi
  echo "# docker server $(docker info --format '{{.ServerVersion}} {{.OSType}}/{{.Architecture}} cpus={{.NCPU}} mem={{.MemTotal}}' 2>/dev/null); DOCKER_DEFAULT_PLATFORM=$DOCKER_DEFAULT_PLATFORM"
  echo "# groth16 image $(docker image inspect risczero/risc0-groth16-prover:v2025-04-03.1 --format '{{.Id}} {{.Os}}/{{.Architecture}} {{.Size}}' 2>/dev/null)"
  echo "# commit $(git rev-parse --short HEAD)$(git diff --quiet || echo ' (uncommitted changes)'); guest sha256 $(shasum -a 256 artifacts/zk/burn_membership.bin | cut -d' ' -f1)"
  echo "# load before: $(uptime | sed 's/.*load/load/')"
} > "$RAW"
( peak=0
  while sleep 1; do
    s="$(docker stats --no-stream --format '{{.Name}} {{.MemUsage}} {{.CPUPerc}}' 2>/dev/null | head -5)"
    c="$(docker ps --filter ancestor=risczero/risc0-groth16-prover:v2025-04-03.1 --format '{{.Names}}' | head -1)"
    [ -n "$c" ] && echo "# docker $(date -u +%H:%M:%S) $(echo "$s" | grep "^$c ")"
  done ) > "$RAW.docker" &
SAMPLER=$!
TIME=(/usr/bin/time -l); [ "$(uname)" = Darwin ] || TIME=(/usr/bin/time -v)
${HEAVY:+"$HEAVY"} "${TIME[@]}" "$CLI" "$@" > "$OUT.tmp" 2>> "$RAW"; rc=$?
kill "$SAMPLER" 2>/dev/null; wait "$SAMPLER" 2>/dev/null
cat "$RAW.docker" >> "$RAW"; rm -f "$RAW.docker"
echo "# exit $rc; load after: $(uptime | sed 's/.*load/load/')" >> "$RAW"
grep '^{' "$OUT.tmp" | tail -1 > "$OUT"; rm -f "$OUT.tmp"
python3 - "$RAW" >> "$RAW" <<'PY'
import re, sys
peak = 0.0
for l in open(sys.argv[1]):
    m = re.match(r"# docker \S+ \S+ ([\d.]+)(GiB|MiB|KiB|B) /", l)
    if m:
        v = float(m.group(1)) * {"GiB": 2**30, "MiB": 2**20, "KiB": 2**10, "B": 1}[m.group(2)]
        peak = max(peak, v)
print(f"# groth16 container peak memory (docker stats samples) {peak / 2**30:.2f} GiB")
PY
sed "s|$HOME|~|g" "$RAW" > "$RAW.tmp" && mv "$RAW.tmp" "$RAW"
grep -E 'real|maximum resident|Maximum resident|peak memory|exit' "$RAW"
exit $rc

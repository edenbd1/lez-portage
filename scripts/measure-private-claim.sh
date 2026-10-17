#!/usr/bin/env bash
# The private claim with real proving (executor-tests --bin measure_private),
# under /usr/bin/time -l for the peak resident set, with the machine and its
# load recorded around it. One run per call; the raw output goes to
#   docs/raw/private-claim-<UTC timestamp>.txt
#   scripts/measure-private-claim.sh            (RISC0_DEV_MODE must be unset)
set -euo pipefail
cd "$(dirname "$0")/.."
unset RISC0_DEV_MODE
cargo build -q --release --manifest-path executor-tests/Cargo.toml --bin measure_private
OUT="docs/raw/private-claim-$(date -u +%Y%m%dT%H%MZ).txt"
provers() { pgrep -f 'r0vm --port' | wc -l | tr -d ' '; }
{
  echo "# $(date -u +%FT%TZ) executor-tests --bin measure_private, real proving (RISC0_DEV_MODE unset), r0vm $(r0vm --version | awk '{print $2}')"
  if [ "$(uname)" = Darwin ]; then
    echo "# $(sysctl -n hw.model) $(sysctl -n machdep.cpu.brand_string) hw.memsize=$(sysctl -n hw.memsize) hw.ncpu=$(sysctl -n hw.ncpu)"
  else
    echo "# $(uname -m) $(nproc) cpus $(awk '/MemTotal/ {print $2 " kB"}' /proc/meminfo)"
  fi
  echo "# commit $(git rev-parse --short HEAD)$(git diff --quiet || echo ' (uncommitted changes)'); wrap_bridge.bin sha256 $(shasum -a 256 artifacts/programs/v0.3/wrap_bridge.bin | cut -d' ' -f1)"
  echo "# load before: $(uptime | sed 's/.*load/load/'); other provers running: $(provers)"
} > "$OUT"
( while sleep 30; do echo "# load during: $(date -u +%H:%M:%S) $(uptime | sed 's/.*load average[s]*: //'); r0vm processes incl. this run's: $(provers)"; done ) > "$OUT.load" &
LOADER=$!
TIME=(/usr/bin/time -l); [ "$(uname)" = Darwin ] || TIME=(/usr/bin/time -v)
"${TIME[@]}" executor-tests/target/release/measure_private >> "$OUT" 2>&1 || echo "# exit $?" >> "$OUT"
kill "$LOADER" 2>/dev/null || true
cat "$OUT.load" >> "$OUT"; rm -f "$OUT.load"
echo "# load after: $(uptime | sed 's/.*load/load/')" >> "$OUT"
sed "s|$HOME|~|g" "$OUT" > "$OUT.tmp" && mv "$OUT.tmp" "$OUT"
grep -E "prove_ms|tx_bytes|maximum resident|Maximum resident|carol reads|absent" "$OUT"
echo "$OUT"

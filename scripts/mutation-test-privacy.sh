#!/usr/bin/env bash
# Mutation test of scripts/privacy-check.py: weaken its detector in ways a real
# bug could, and require the controls to catch every one (the check must FAIL).
set -uo pipefail
cd "$(dirname "$0")/.."
caught=0; total=0
for m in blind hex prefix64k first-only skip-own; do
  total=$((total + 1))
  out=$(MUTANT=$m python3 - <<'PY' 2>&1
import importlib.util, os, sys
spec = importlib.util.spec_from_file_location("pc", "scripts/privacy-check.py")
pc = importlib.util.module_from_spec(spec); spec.loader.exec_module(pc)
real = pc.leaks
m = os.environ["MUTANT"]
pc.leaks = {
    "blind":      lambda tx, s: [],                                                    # finds nothing
    "hex":        lambda tx, s: [n for n, v in s.items() if v.hex().encode() in tx],   # searches hex text
    "prefix64k":  lambda tx, s: real(tx[:65536], s),                                   # reads only the head
    "first-only": lambda tx, s: real(tx, dict(list(s.items())[:1])),                   # checks one value
    "skip-own":   lambda tx, s: [n for n in real(tx, s) if not n.startswith(("carol", "alice"))],
}[m]
sys.argv = ["privacy-check.py"]
pc.main()
PY
)
  if echo "$out" | grep -q "== FAIL"; then caught=$((caught + 1)); echo "  caught  $m: $(echo "$out" | grep -c FAIL) failing line(s)"
  else echo "  MISSED  $m"; fi
done
echo "$caught of $total mutants caught"
[ "$caught" = "$total" ]

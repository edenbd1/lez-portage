#!/usr/bin/env bash
# The verifier is only evidence if it fails when the record is wrong. Each
# mutation below corrupts one thing in a copy of a manifest;
# scripts/verify-onchain.sh must exit non-zero on every one, with a FAIL line,
# and zero on the manifest as recorded.
#
#   scripts/mutation-test-verifier.sh [--manifest FILE] [--rpc URL] [--eth-rpc URL]
set -uo pipefail
cd "$(dirname "$0")/.."
MANIFEST="evidence/local/e2e.tsv"; RPC="http://127.0.0.1:3051"; ETH_RPC=""
while [ $# -gt 0 ]; do
  case "$1" in
    --manifest) MANIFEST="$2"; shift 2 ;;
    --rpc) RPC="$2"; shift 2 ;;
    --eth-rpc) ETH_RPC="$2"; shift 2 ;;
    *) echo "usage: $0 [--manifest FILE] [--rpc URL] [--eth-rpc URL]"; exit 2 ;;
  esac
done
ETH=(); [ -n "$ETH_RPC" ] && ETH=(--eth-rpc "$ETH_RPC")
T="$(mktemp -d)"
fake="$(python3 -c 'import secrets;print(secrets.token_hex(32))')"
mutate() { # mutation
  python3 - "$MANIFEST" "$T/$1.tsv" "$1" "$fake" <<'PY'
import sys
src, dst, how, fake = sys.argv[1:5]
lines = open(src).read().splitlines()
def cols(i): return lines[i].split("\t")
rows = [i for i, l in enumerate(lines) if l and not l.startswith("#")]
public = [i for i in rows if cols(i)[6] != "private"]
private = [i for i in rows if cols(i)[6] == "private"]
eth = [i for i, l in enumerate(lines) if l.startswith("# eth ")]
def ecols(i): return lines[i].split(" ")
def eput(i, c): lines[i] = " ".join(c)
def put(i, c): lines[i] = "\t".join(c)
def comment(prefix):
    return next(i for i, l in enumerate(lines) if l.startswith(prefix))
if how == "fake-hash":               # an applied transaction's hash replaced by one never sent
    i = next(i for i in public if cols(i)[1] == "applied"); c = cols(i); c[4] = fake; put(i, c)
elif how == "refusal-as-applied":    # a refused transaction claimed as applied
    i = next(i for i in public if cols(i)[1] == "refused"); c = cols(i); c[1] = "applied"; c[2] = "applied"; c[6] = "gas=" + c[7]; put(i, c)
elif how == "applied-as-refused":    # an applied transaction claimed as refused
    i = next(i for i in public if cols(i)[1] == "applied"); c = cols(i); c[1] = "refused"; c[2] = "reverted"; put(i, c)
elif how == "wrong-gas":             # an applied transaction's pre-checked gas off by one
    i = next(i for i in public if cols(i)[1] == "applied"); c = cols(i); c[6] = "gas=%d" % (int(c[6][4:]) + 1); put(i, c)
elif how == "wrong-block":           # a transaction recorded in another block
    i = public[0]; c = cols(i); c[3] = str(int(c[3]) + 1); put(i, c)
elif how == "swapped-hashes":        # two transactions' hashes swapped (each exists, neither is its own)
    a, b = public[0], public[1]; ca, cb = cols(a), cols(b); ca[4], cb[4] = cb[4], ca[4]; put(a, ca); put(b, cb)
elif how == "private-fake-hash":     # the private claim's hash replaced by one never sent
    i = private[0]; c = cols(i); c[4] = fake; put(i, c)
elif how == "private-as-public":     # the private claim recorded as a public transaction
    i = private[0]; c = cols(i); c[6] = "gas=1"; put(i, c)
elif how in ("private-destination", "private-burn-holder"):  # an account the transaction does publish (the bridge config, a row of it)
    cfg = next(l.split()[2] for l in lines if l.startswith("# final ") and l.endswith(" config"))
    i = comment("# private claim-carol-private " if how == "private-destination" else "# private burn-carol-private ")
    p = lines[i].split(); p[p.index("hides") + 1] = cfg; lines[i] = " ".join(p)
elif how in ("private-size", "private-burn-size"):  # a private transaction's size off by one
    i = comment("# private claim-carol-private " if how == "private-size" else "# private burn-carol-private ")
    p = lines[i].split(); k = p.index("tx_bytes") + 1; p[k] = str(int(p[k]) + 1); lines[i] = " ".join(p)
elif how == "private-burn-fake-hash":  # the private burn's hash replaced by one never sent
    i = next(i for i in private if cols(i)[0] == "burn-carol-private"); c = cols(i); c[4] = fake; put(i, c)
elif how == "eth-fake-hash":         # an applied release's hash replaced by one never sent
    i = next(i for i in eth if ecols(i)[3] == "applied"); c = ecols(i); c[6] = "0x" + fake; eput(i, c)
elif how == "eth-refusal-as-applied":  # a refused release recorded as applied
    i = next(i for i in eth if ecols(i)[3] != "applied"); c = ecols(i); c[3] = "applied"; c[4] = "applied"; c[7] = "-"; eput(i, c)
elif how == "eth-applied-as-refused":  # an applied release recorded as refused
    i = next(i for i in eth if ecols(i)[3] == "applied"); c = ecols(i); c[3] = "RootNotAttested"; c[4] = "reverted"; c[7] = "0xfb0612b1"; eput(i, c)
elif how == "eth-other-error":       # a refusal recorded with another error than the one the chain gives
    i = next(i for i in eth if ecols(i)[3] == "RootNotAttested"); c = ecols(i); c[3] = "NotFinal"; c[7] = "0xd7c25c79"; eput(i, c)
elif how == "eth-wrong-block":       # a release recorded in another block
    i = eth[0]; c = ecols(i); c[5] = str(int(c[5]) + 1); eput(i, c)
elif how == "eth-wrong-nullifier":   # an applied release recorded with another nullifier
    i = next(i for i in eth if ecols(i)[3] == "applied"); c = ecols(i); c[9] = "0x" + fake; eput(i, c)
elif how == "eth-wrong-root":        # the root the releases were checked against is not the LEZ burn tree's
    i = comment("# release-root "); p = lines[i].split(); p[2] = "0x" + fake; lines[i] = " ".join(p)
elif how == "eth-unreleased":        # a released burn's nullifier recorded as unspent
    i = comment("# eth-final nullifier "); p = lines[i].split(); p[4] = "false"; lines[i] = " ".join(p)
elif how == "eth-control-spent":     # a random nullifier recorded as spent
    i = comment("# eth-final unspent "); p = lines[i].split(); p[4] = "true"; lines[i] = " ".join(p)
elif how == "eth-balance":           # the vault's final balance off by one wei
    i = comment("# eth-final balance "); p = lines[i].split(); p[4] = str(int(p[4]) + 1); lines[i] = " ".join(p)
elif how == "public-as-private":     # a public transaction recorded as the private claim
    i = next(i for i in public if cols(i)[1] == "applied"); c = cols(i); c[6] = "private"; put(i, c)
elif how == "final-state":           # a nullifier account's recorded final bytes altered
    i = [i for i, l in enumerate(lines) if l.startswith("# final ") and " nullifier_" in l][-1]; p = lines[i].split(); p[5] = "0" * 64; lines[i] = " ".join(p)
elif how == "final-unclaimed":       # the unclaimed deposit's nullifier claimed as spent
    i = next(i for i, l in enumerate(lines) if l.startswith("# final ") and l.endswith(" nullifier_2")); p = lines[i].split()
    p[4] = "1"; p[5] = "4bf5122f344554c53bde2ebb8cd2b7e3d1600ad631c385a5d7cce23c7785459a"; lines[i] = " ".join(p)
elif how == "shared-block-gas":      # a transaction in a shared block recorded with another pre-checked gas
    blk, *members = next(l.split()[2:] for l in lines if l.startswith("# shared-block "))
    i = next(i for i in public if cols(i)[0] in members and cols(i)[1] == "applied"); c = cols(i); c[6] = "gas=%d" % (int(c[6][4:]) + 1); put(i, c)
elif how == "shared-block-member":   # a shared block recorded without one of its transactions
    i = comment("# shared-block "); p = lines[i].split(); lines[i] = " ".join(p[:-1])
elif how == "image-id":              # another ImageID claimed for a deployed program
    i = next(i for i, l in enumerate(lines) if l.startswith("# program ") and " wrap_bridge" in l); p = lines[i].split(); p[3] = "0" * 64; lines[i] = " ".join(p)
elif how == "deploy-hash":           # a deployment transaction never sent
    i = comment("# deploy "); p = lines[i].split(); p[3] = fake; lines[i] = " ".join(p)
elif how == "zk-image-id":          # the committed guest recorded with another image id
    i = comment("# binary burn_membership "); p = lines[i].split(); k = p.index("image_id") + 1; p[k] = "0" * 64; lines[i] = " ".join(p)
elif how == "zk-verifier":           # the zk verifier recorded as RISC Zero's verifier (the vault does not use it)
    i = comment("# anvil zk-verifier "); p = lines[i].split(); p[3] = p[5]; lines[i] = " ".join(p)
elif how == "zk-leaf-sent":          # a released burn's leaf recorded as a word the release does send (its nullifier)
    i = comment("# release-proof carol "); p = lines[i].split(); p[p.index("leaf") + 1] = p[p.index("nullifier") + 1]; lines[i] = " ".join(p)
elif how == "zk-path-sent":          # a path node recorded as a word the release does send (its root)
    i = comment("# release-path alice "); p = lines[i].split()
    root = next(l.split()[2] for l in lines if l.startswith("# release-root "))
    p[3] = root if p[3] == "-" else p[3] + "," + root; lines[i] = " ".join(p)
open(dst, "w").write("\n".join(lines) + "\n")
PY
}
fail=0; caught=0; total=0
./scripts/verify-onchain.sh --manifest "$MANIFEST" --rpc "$RPC" ${ETH[@]+"${ETH[@]}"} > "$T/clean.out" 2>&1 \
  && echo "  ok   the recorded manifest verifies ($(tail -1 "$T/clean.out"))" || { echo "  FAIL the recorded manifest does not verify"; tail -5 "$T/clean.out"; fail=1; }
MUTS=(fake-hash refusal-as-applied applied-as-refused wrong-gas wrong-block swapped-hashes private-fake-hash private-as-public
      private-destination private-size public-as-private final-state image-id deploy-hash)
# Those that need a line only some runs record (the local run's unclaimed deposit and
# private burn; a block the public testnet packed two of the run's transactions into).
grep -q ' nullifier_2$' "$MANIFEST" && MUTS+=(final-unclaimed)
grep -q '^burn-carol-private'$'\t' "$MANIFEST" && MUTS+=(private-burn-fake-hash private-burn-holder private-burn-size)
grep -q '^# shared-block ' "$MANIFEST" && MUTS+=(shared-block-gas shared-block-member)
[ -n "$ETH_RPC" ] && MUTS+=(eth-fake-hash eth-refusal-as-applied eth-applied-as-refused eth-other-error eth-wrong-block eth-wrong-nullifier
                            eth-wrong-root eth-unreleased eth-control-spent eth-balance)
grep -q '^# anvil zk-verifier ' "$MANIFEST" && MUTS+=(zk-image-id zk-verifier zk-leaf-sent zk-path-sent)
for m in "${MUTS[@]}"; do
  total=$((total + 1))
  mutate "$m"
  if ./scripts/verify-onchain.sh --manifest "$T/$m.tsv" --rpc "$RPC" ${ETH[@]+"${ETH[@]}"} > "$T/$m.out" 2>&1; then
    echo "  FAIL mutation $m was NOT caught"; fail=1
  elif grep -q '^  FAIL' "$T/$m.out"; then
    caught=$((caught + 1))
    echo "  ok   mutation $m caught: $(grep -m1 '^  FAIL' "$T/$m.out" | sed 's/^ *FAIL *//' | cut -c1-120)"
  else
    echo "  FAIL mutation $m: the verifier exited non-zero without a FAIL line ($(tail -1 "$T/$m.out" | cut -c1-120))"; fail=1
  fi
done
echo "$caught of $total mutations caught"
exit $fail

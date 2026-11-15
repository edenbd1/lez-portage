#!/usr/bin/env bash
# The outbound verifier is only evidence if it fails when the record is wrong.
# Each mutation below corrupts one claim in a copy of the outbound manifest
# (and, for the Bedrock ones, of the saved Bedrock blocks);
# scripts/verify-outbound.sh must exit non-zero on every one, with a FAIL
# line, and zero on the record as kept.
#
#   scripts/mutation-test-outbound.sh [--manifest FILE] [--rpc URL] [--eth-rpc URL]
set -uo pipefail
cd "$(dirname "$0")/.."
MANIFEST="evidence/testnet-outbound/run.tsv"; RPC="https://testnet.lez.logos.co"; ETH_RPC="https://rpc.sepolia.ethpandaops.io"
while [ $# -gt 0 ]; do
  case "$1" in
    --manifest) MANIFEST="$2"; shift 2 ;;
    --rpc) RPC="$2"; shift 2 ;;
    --eth-rpc) ETH_RPC="$2"; shift 2 ;;
    *) echo "usage: $0 [--manifest FILE] [--rpc URL] [--eth-rpc URL]"; exit 2 ;;
  esac
done
SRC="$(dirname "$MANIFEST")"
T="$(mktemp -d)"
fake="$(python3 -c 'import secrets;print(secrets.token_hex(32))')"
mutate() { # mutation -> $T/<m>/run.tsv (with a copy of the Bedrock blocks)
  mkdir -p "$T/$1"; [ -d "$SRC/bedrock" ] && cp -R "$SRC/bedrock" "$T/$1/bedrock"
  python3 - "$MANIFEST" "$T/$1/run.tsv" "$1" "$fake" "$T/$1/bedrock" <<'PY'
import json, os, sys
src, dst, how, fake, bdir = sys.argv[1:6]
lines = open(src).read().splitlines()
def idx(prefix, cond=lambda p: True):
    return next(i for i, l in enumerate(lines) if l.startswith(prefix) and cond(l.split()))
def edit(i, key=None, pos=None, val=None, f=None):
    p = lines[i].split(); k = p.index(key) + 1 if key else pos
    p[k] = f(p[k]) if f else val; lines[i] = " ".join(p)
applied = lambda p: p[3] == "applied"
if how == "release-fake-hash":        # the applied release's hash replaced by one never sent
    edit(idx("# eth ", applied), pos=6, val="0x" + fake)
elif how == "release-wrong-nullifier":
    edit(idx("# eth ", applied), pos=9, val="0x" + fake)
elif how == "release-wrong-root":
    edit(idx("# eth ", applied), pos=10, val="0x" + fake)
elif how == "release-refusal-as-applied":  # a refused attempt recorded as applied
    edit(idx("# eth ", lambda p: p[3] != "applied"), pos=3, val="applied"); edit(idx("# eth ", lambda p: p[3] == "applied" and p[4] == "reverted"), pos=4, val="applied")
elif how == "release-other-error":     # a refusal recorded with another error
    i = idx("# eth ", lambda p: p[3] == "NotFinal"); edit(i, pos=3, val="RootNotAttested"); edit(i, pos=7, val="0xfb0612b1")
elif how == "burn-tx-other":           # the release said to pay out another LEZ transaction (one of the inbound run's)
    edit(idx("# release-proof "), key="burn_tx", val="834a346002e0410e5248a45e4ac79ce92bda88e327c3d8c7f1611b6ee5881752")
elif how == "burn-block":
    edit(idx("# release-proof "), key="burn_block", f=lambda v: str(int(v) + 1))
elif how == "burn-private":            # a public burn recorded as private
    edit(idx("# release-proof "), key="private", f=lambda v: "false" if v == "true" else "true")
elif how == "recipient":               # another recipient claimed
    edit(idx("# release-proof "), key="recipient", val="0x" + fake[:40])
elif how == "amount":
    edit(idx("# release-proof "), key="amount", f=lambda v: str(int(v) * 10))
elif how == "leaf":
    edit(idx("# release-proof "), key="leaf", val="0x" + fake)
elif how == "attested-final":          # the attestation said to name another final block
    edit(idx("# eth-attestation ", lambda p: p[2] == next(l.split()[2] for l in lines if l.startswith("# eth ") and l.split()[3] == "applied")),
         key="lez_final_block", f=lambda v: str(int(v) + 1))
elif how == "verifier":                # another verifier claimed for the vault
    edit(idx("# eth-vault "), pos=4, val="0x" + fake[:40])
elif how == "committee-member":        # an outsider claimed as a member
    edit(idx("# eth-vault "), pos=6, f=lambda v: ",".join(["0x" + fake[:40]] + v.split(",")[1:]))
elif how == "threshold":
    edit(idx("# eth-vault "), pos=8, val="1")
elif how == "finality-depth":
    edit(idx("# eth-vault "), pos=10, val="3")
elif how == "pairing":
    edit(idx("# network "), key="pairing", val="0x" + fake)
elif how == "release-root":
    edit(idx("# release-root "), pos=2, val="0x" + fake)
elif how == "unreleased":              # the released nullifier recorded as unspent
    edit(idx("# eth-final nullifier "), pos=4, val="false")
elif how == "control-spent":
    edit(idx("# eth-final unspent "), pos=4, val="true")
elif how == "recipient-balance":
    edit(idx("# eth-final balance ", lambda p: p[-1].endswith("-recipient")), pos=4, f=lambda v: str(int(v) + 1))
elif how == "burn-gas":                # the public burn's pre-checked gas off by one
    i = next(i for i, l in enumerate(lines) if l.startswith("burn-alice\t")); c = lines[i].split("\t"); c[6] = "gas=%d" % (int(c[6][4:]) + 1); lines[i] = "\t".join(c)
elif how == "bedrock-other-block":     # the members' Bedrock note of an applied release names another (saved) block
    released = {l.split()[2] for l in lines if l.startswith("# eth ") and l.split()[3] == "applied"}
    i = idx("# eth-attestation ", lambda p: p[2] in released and any(":bedrock=lez" in x for x in p))
    p = lines[i].split(); k = next(j for j, x in enumerate(p) if ":bedrock=lez" in x)
    d = next(os.path.join(bdir, x) for x in sorted(os.listdir(bdir)))
    walk = json.load(open(os.path.join(d, "walk.json")))
    other = walk["chain_top_down"][0]
    pre, rest = p[k].split("@", 1); p[k] = pre + "@" + other + ":" + rest.split(":", 1)[1]; lines[i] = " ".join(p)
elif how == "bedrock-inscription":     # a saved Bedrock block's inscription altered by one byte
    for d in os.listdir(bdir):
        walk = json.load(open(os.path.join(bdir, d, "walk.json")))
        f = os.path.join(bdir, d, walk["inscription_block"] + ".json"); b = json.load(open(f))
        for t in b["transactions"]:
            op = t["mantle_tx"]["ops"][0]
            if op["opcode"] == 17 and op["payload"]["channel_id"] == "01" * 32:
                s = op["payload"]["inscription"]; op["payload"]["inscription"] = s[:200] + ("0" if s[200] != "0" else "1") + s[201:]
        json.dump(b, open(f, "w"))
elif how in ("bedrock-inscription-last-content", "bedrock-status-invalid"):
    # The comparison skips only the last byte (the node's Bedrock status, outside the hash):
    # the byte before it is block content and must still count, and the status itself must be
    # a real one (0, 1 or 2).
    for d in os.listdir(bdir):
        walk = json.load(open(os.path.join(bdir, d, "walk.json")))
        f = os.path.join(bdir, d, walk["inscription_block"] + ".json"); b = json.load(open(f))
        for t in b["transactions"]:
            op = t["mantle_tx"]["ops"][0]
            if op["opcode"] == 17 and op["payload"]["channel_id"] == "01" * 32:
                raw = bytearray(bytes.fromhex(op["payload"]["inscription"]))
                if how == "bedrock-status-invalid": raw[-1] = 7
                else: raw[-2] ^= 1
                op["payload"]["inscription"] = raw.hex()
        json.dump(b, open(f, "w"))
elif how == "bedrock-broken-walk":     # a saved block's parent link broken
    for d in os.listdir(bdir):
        walk = json.load(open(os.path.join(bdir, d, "walk.json")))
        f = os.path.join(bdir, d, walk["chain_top_down"][0] + ".json"); b = json.load(open(f))
        b["header"]["parent_block"] = fake; json.dump(b, open(f, "w"))
open(dst, "w").write("\n".join(lines) + "\n")
PY
}
fail=0; caught=0; total=0
./scripts/verify-outbound.sh --manifest "$MANIFEST" --rpc "$RPC" --eth-rpc "$ETH_RPC" > "$T/clean.out" 2>&1 \
  && echo "  ok   the recorded manifest verifies ($(grep -E 'checks pass' "$T/clean.out" | tr '\n' ';'))" || { echo "  FAIL the recorded manifest does not verify"; grep FAIL "$T/clean.out" | head; fail=1; }
MUTS=(release-fake-hash release-wrong-nullifier release-wrong-root release-refusal-as-applied release-other-error burn-tx-other burn-block
      burn-private recipient amount leaf attested-final verifier committee-member threshold finality-depth pairing release-root unreleased
      control-spent recipient-balance burn-gas)
[ -d "$SRC/bedrock" ] && MUTS+=(bedrock-other-block bedrock-inscription bedrock-inscription-last-content bedrock-status-invalid bedrock-broken-walk)
for m in "${MUTS[@]}"; do
  total=$((total + 1))
  mutate "$m" || { echo "  FAIL mutation $m could not be applied"; fail=1; continue; }
  if ./scripts/verify-outbound.sh --manifest "$T/$m/run.tsv" --rpc "$RPC" --eth-rpc "$ETH_RPC" > "$T/$m.out" 2>&1; then
    echo "  FAIL mutation $m was NOT caught"; fail=1
  elif grep -q '^  FAIL' "$T/$m.out"; then
    caught=$((caught + 1))
    echo "  ok   mutation $m caught: $(grep -m1 '^  FAIL' "$T/$m.out" | sed 's/^ *FAIL *//' | cut -c1-130)"
  else
    echo "  FAIL mutation $m: the verifier exited non-zero without a FAIL line ($(tail -1 "$T/$m.out" | cut -c1-120))"; fail=1
  fi
done
echo "$caught of $total mutations caught"
rm -rf "$T"
exit $fail

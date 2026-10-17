#!/usr/bin/env bash
# Re-check a recorded run of the bridge against a LEZ v0.3 sequencer, from its
# manifest, with nothing but JSON-RPC (no wallet, no build).
#
#   scripts/verify-onchain.sh --manifest evidence/local/e2e.tsv --rpc http://127.0.0.1:3051
#
# A manifest is what scripts/e2e-local.sh writes: one line per transaction
#   label expected verdict block tx attempts precheck gas_limit charged_gas
# and comment lines, of which these are checked:
#   # program <header> <image-id> <name>   the program_loader header names this ImageID
#   # deploy <what> <hash>                 the deployment transaction resolves
#   # final <account> <program> <len> <sha256> <name>
#                                          the account's shard of that program is these bytes now
#   # private-destination <account>        the private claim's destination
#   # private-claim ... tx_bytes <n> ...   the private claim's size
#
# What is checked, for every transaction line:
# - it resolves by hash (getTransaction) to the recorded block, and the hash is
#   SHA-256 of the returned bytes after the variant tag: the hash names exactly
#   those bytes;
# - a public one (precheck `gas=N` or a refusal code): what the chain charged it.
#   On LEZ v0.3 a public transaction whose program refuses is still included,
#   changes nothing, and is charged its whole gas limit; an accepted one is
#   charged the cycles it used. The gas limit is read from the transaction's own
#   bytes, the charged gas from the `Distribute` summary of the block's fee
#   transaction (gas_used_exec, gas_used_stor). When gas_used_stor is exactly
#   this transaction's size, it is the block's only charged transaction:
#   `applied` needs gas_used_exec equal to the pre-checked gas and below the
#   limit, `refused` needs it equal to the limit;
# - the private one (precheck `private`): it is a privacy-preserving
#   transaction (variant tag 1) carrying no signature, of the recorded size,
#   and the private destination's id occurs nowhere in its bytes;
# then every `final` line, and a control: a random hash does not resolve.
#
# Needs python3. Exits non-zero on any mismatch.
set -uo pipefail
cd "$(dirname "$0")/.."
MANIFEST="evidence/local/e2e.tsv"
RPC="http://127.0.0.1:3051"
while [ $# -gt 0 ]; do
  case "$1" in
    --manifest) MANIFEST="${2:?}"; shift 2 ;;
    --rpc) RPC="${2:?}"; shift 2 ;;
    *) echo "usage: $0 [--manifest FILE] [--rpc URL]" >&2; exit 2 ;;
  esac
done
[ -f "$MANIFEST" ] || { echo "no manifest at $MANIFEST"; exit 3; }

python3 - "$MANIFEST" "$RPC" <<'PY'
import base64, hashlib, json, secrets, struct, sys, urllib.request

manifest, rpc = sys.argv[1], sys.argv[2]

def call(method, param):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": [param]}).encode()
    req = urllib.request.Request(rpc, body, {"Content-Type": "application/json"})
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                return json.load(r)
        except Exception:
            if attempt == 2:
                raise

FEE = "2TsGdKDWTY92X4vSBZzjzemZpThhyqNEdZJj3xBrQ9AZ"   # fee_core::fee_account_id()
LOADER = "JAQ5MVHbCkSYRzXunsrNuM2m1LS859PGveHfoYPAmcvZ"  # [0xFE; 32], program_loader
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
def unb58(s):
    n = 0
    for c in s:
        n = n * 58 + B58.index(c)
    return n.to_bytes(32, "big")

def u32(b, o):
    return struct.unpack_from("<I", b, o)[0]

def gas_limit_of(tx):
    # borsh LeeTransaction::Public(PublicTransaction { message: Message { program_account_id,
    # shard_selectors, nonces, instruction_data, fee: Option<FeeDeclaration { payer, gas_limit, .. }> }, .. })
    if tx[0] != 0:
        raise ValueError("not a public transaction")
    o = 1 + 32
    o += 4 + 64 * u32(tx, o)
    o += 4 + 16 * u32(tx, o)
    o += 4 + u32(tx, o)
    if tx[o] != 1:
        raise ValueError("a fee-exempt transaction")
    return struct.unpack_from("<Q", tx, o + 1 + 32)[0]

def fee_summary(block):
    raw = base64.b64decode(call("getBlock", block)["result"])
    i = raw.find(unb58(FEE) + struct.pack("<I", 4))
    assert i >= 0, f"block {block} has no fee transaction"
    o = i + 32 + 4 + 4 * 64
    assert u32(raw, o) == 0 and u32(raw, o + 4) == 65 and raw[o + 8] == 0, "unexpected fee instruction"
    return struct.unpack_from("<QQ", raw, o + 9)

def shard(account, program):
    res = call("getAccount", account).get("result") or {}
    return bytes(((res.get("data") or {}).get("shards") or {}).get(program, []))

ok = bad = 0
def report(good, text):
    global ok, bad
    if good:
        ok += 1; print(f"  ok   {text}")
    else:
        bad += 1; print(f"  FAIL {text}")

lines = open(manifest).read().splitlines()
private_dest = next((l.split()[2] for l in lines if l.startswith("# private-destination ")), None)
private_size = None
for l in lines:
    if l.startswith("# private-claim ") and " tx_bytes " in l:
        p = l.split(); v = p[p.index("tx_bytes") + 1]
        private_size = int(v) if v.isdigit() else None

print(f"wrap_bridge run against {rpc}, from {manifest}")
applied = refused = private = 0
for line in lines:
    if line.startswith("# program "):
        _, _, header, image, name = line.split()[:5]
        h = shard(header, LOADER)
        got = h[:32].hex() if len(h) >= 32 else ""
        report(got == image, f"program {name}: header {header[:12]}… names ImageID {image[:16]}…" + ("" if got == image else f" (chain has {got or 'nothing'})"))
    elif line.startswith("# deploy "):
        _, _, what, tx = line.split()[:4]
        res = call("getTransaction", tx).get("result")
        report(isinstance(res, list) and len(res) > 1, f"deploy {what}: {tx[:16]}… resolves" + (f" (block {res[1]})" if isinstance(res, list) and len(res) > 1 else " NOT FOUND"))
    elif line.startswith("# final "):
        _, _, account, program, length, want, name = line.split()[:7]
        b = shard(account, program)
        same = hashlib.sha256(b).hexdigest() == want and len(b) == int(length)
        report(same, f"final state of {name} ({account[:12]}…): {length} bytes as the run left them" if same
               else f"final state of {name} ({account[:12]}…): recorded {length} bytes {want[:16]}…, chain has {len(b)} bytes {hashlib.sha256(b).hexdigest()[:16]}…")
    elif line and not line.startswith("#"):
        cols = line.split("\t") + [""] * 9
        label, expected, verdict, block, tx, attempts, pre = cols[:7]
        if tx in ("", "-"):
            report(False, f"{label}: no transaction recorded ({verdict})")
            continue
        res = call("getTransaction", tx).get("result")
        if not (isinstance(res, list) and len(res) > 1):
            report(False, f"{label}: {tx[:16]}… NOT FOUND on chain")
            continue
        found = str(res[1] if not isinstance(res[1], dict) else res[1].get("height"))
        mine = base64.b64decode(res[0])
        if hashlib.sha256(mine[1:]).hexdigest() != tx:
            report(False, f"{label}: the bytes getTransaction returns do not hash to {tx[:16]}…")
            continue
        if found != block:
            report(False, f"{label}: {tx[:16]}… is in block {found}, recorded in {block}")
            continue
        if pre == "private":
            private += 1
            sigs = None
            good = mine[0] == 1 and expected == "applied"
            why = []
            if mine[0] != 1:
                why.append("not a privacy-preserving transaction")
            if private_size is not None and len(mine) != private_size:
                good = False; why.append(f"{len(mine)} bytes, recorded {private_size}")
            if private_dest is None:
                good = False; why.append("no private destination recorded")
            elif unb58(private_dest) in mine:
                good = False; why.append("the private destination's id occurs in the transaction")
            report(good, f"{label}: privacy-preserving, {tx[:16]}… in block {found}, {len(mine)} bytes, the destination absent from its bytes" if good
                   else f"{label}: " + "; ".join(why or ["expected applied"]))
            continue
        try:
            limit = gas_limit_of(mine)
        except ValueError as e:
            report(False, f"{label}: {e}")
            continue
        exec_gas, stor_gas = fee_summary(int(found))
        if stor_gas != len(mine):
            report(False, f"{label}: block {found} holds other charged transactions; its charge is not separable")
            continue
        if expected == "applied":
            applied += 1
            want = int(pre.split("=")[1]) if pre.startswith("gas=") else -1
            report(exec_gas == want and exec_gas < limit,
                   f"{label}: applied, {tx[:16]}… in block {found}, charged {exec_gas} gas = the pre-checked gas, below its limit {limit}" if exec_gas == want and exec_gas < limit
                   else f"{label}: expected applied, charged {exec_gas} gas (pre-checked {want}, limit {limit})")
        else:
            refused += 1
            report(exec_gas == limit,
                   f"{label}: refused ({pre}), {tx[:16]}… in block {found}, charged its whole limit {limit}" if exec_gas == limit
                   else f"{label}: expected refused, charged {exec_gas} gas against a limit of {limit}")

ctl = secrets.token_hex(32)
res = call("getTransaction", ctl)
report("result" in res and res["result"] is None, f"control: a random hash ({ctl[:12]}…) does not resolve, and the endpoint answers")
print(f"{ok} checks pass, {bad} fail ({applied} applied and {refused} refused public transactions, {private} privacy-preserving, in the manifest)")
sys.exit(1 if bad else 0)
PY

#!/usr/bin/env bash
# Re-check a recorded run of the bridge against a LEZ v0.3 sequencer, from its
# manifest, with nothing but JSON-RPC (no wallet, no build).
#
#   scripts/verify-onchain.sh --manifest evidence/local/e2e.tsv --rpc http://127.0.0.1:3051 [--eth-rpc http://127.0.0.1:8551]
#
# A manifest is what scripts/e2e-local.sh writes: one line per transaction
#   label expected verdict block tx attempts precheck gas_limit charged_gas
# and comment lines, of which these are checked:
#   # program <header> <image-id> <name>   the program_loader header names this ImageID
#   # deploy <what> <hash>                 the deployment transaction resolves
#   # shared-block <block> <label> <label> ...
#                                          these transactions of the run share one block: their
#                                          charges are checked together (below)
#   # final <account> <program> <len> <sha256> <name>
#                                          the account's shard of that program is these bytes now
#   # private <label> hides <account> tx_bytes <n>
#                                          the private transaction <label>: its size, and an
#                                          account id that must occur nowhere in its bytes
#   # eth <label> <expected> <verdict> <block> <tx> <selector|-> <gas_used> <nullifier> <root>
#                                          a release attempt on the Ethereum side (--eth-rpc)
#   # eth-final nullifier|unspent <nf> <true|false> <name>, # eth-final balance <addr> <wei> <name>
#   # release-root <root> <burns-account> <program>
#                                          the root the releases were checked against is the
#                                          LEZ burn tree's root now
#   # anvil zk-verifier <verifier> risc0 <groth16-verifier> image_id <id>
#                                          zero-knowledge releases (ZkBurnVerifier); with it:
#   # binary burn_membership ... image_id <id>, # release-proof <owner> ... leaf <leaf> ...,
#   # release-path <owner> <nodes>        the guest committed, and each released burn's opening
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
# A block that holds several of the run's own transactions (a `shared-block`
# line; the public testnet sometimes packs two, or moves one into the next
# block when it rewrites recent blocks) is checked as a group: its
# gas_used_stor must be exactly the sum of their sizes, so it holds no other
# charged transaction, and its gas_used_exec exactly the sum of what each
# verdict implies (an applied one its pre-checked gas, a refused one its
# whole limit).
# With --eth-rpc, every `eth` line: its receipt is in the recorded block with
# the recorded gas; an applied release has status 1 and a Release event of the
# vault naming the recorded nullifier; a refused one has status 0, and the
# same call replayed (eth_call at the block before) reverts with the recorded
# selector, which is the expected error's; every applied release's root is
# the `release-root`, which is the LEZ burn tree's root on the sequencer now;
# every `eth-final` line holds; and a random transaction hash does not
# resolve on anvil either. With an `anvil zk-verifier` line: the vault's
# verifier is it, pinned to the committed guest's image id (`binary
# burn_membership`) over the recorded RISC Zero verifier; for every applied
# release, the seal and journal are read back from the transaction's input:
# sha256(journal) is the vault's releaseDigest of the release's arguments,
# its root and nullifier are the recorded ones, RISC Zero's verifier accepts
# the seal for that image id and digest (eth_call) and refuses it with one
# byte flipped, and neither the burn's leaf nor any node of its path occurs
# in the input.
#
# Needs python3. Exits non-zero on any mismatch.
set -uo pipefail
cd "$(dirname "$0")/.."
MANIFEST="evidence/local/e2e.tsv"
RPC="http://127.0.0.1:3051"
ETH_RPC=""
while [ $# -gt 0 ]; do
  case "$1" in
    --manifest) MANIFEST="${2:?}"; shift 2 ;;
    --rpc) RPC="${2:?}"; shift 2 ;;
    --eth-rpc) ETH_RPC="${2:?}"; shift 2 ;;
    *) echo "usage: $0 [--manifest FILE] [--rpc URL] [--eth-rpc URL]" >&2; exit 2 ;;
  esac
done
[ -f "$MANIFEST" ] || { echo "no manifest at $MANIFEST"; exit 3; }

python3 - "$MANIFEST" "$RPC" "$ETH_RPC" <<'PY'
import base64, hashlib, json, secrets, struct, sys, urllib.request

manifest, rpc, eth_rpc = sys.argv[1], sys.argv[2], sys.argv[3]

def call(method, param, url=None, many=False):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": param if many else [param]}).encode()
    req = urllib.request.Request(url or rpc, body, {"Content-Type": "application/json"})
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
# label -> (the account id it must not carry, its size)
private_info = {}
for l in lines:
    if l.startswith("# private ") and " hides " in l:
        p = l.split(); v = p[p.index("tx_bytes") + 1]
        private_info[p[2]] = (p[p.index("hides") + 1], int(v) if v.isdigit() else None)

shared = {}
for l in lines:
    if l.startswith("# shared-block "):
        p = l.split()
        shared[p[2]] = p[3:]
groups = {}  # block -> {label: (expected charge, size)}

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
            good = mine[0] == 1 and expected == "applied"
            why = []
            if mine[0] != 1:
                why.append("not a privacy-preserving transaction")
            hidden, size = private_info.get(label, (None, None))
            if size is None or len(mine) != size:
                good = False; why.append(f"{len(mine)} bytes, recorded {size}")
            if hidden is None:
                good = False; why.append("no hidden account recorded")
            elif unb58(hidden) in mine:
                good = False; why.append(f"the private account's id {hidden[:12]}… occurs in the transaction")
            report(good, f"{label}: privacy-preserving, {tx[:16]}… in block {found}, {len(mine)} bytes, Private/{hidden[:12]}… absent from its bytes" if good
                   else f"{label}: " + "; ".join(why or ["expected applied"]))
            continue
        try:
            limit = gas_limit_of(mine)
        except ValueError as e:
            report(False, f"{label}: {e}")
            continue
        exec_gas, stor_gas = fee_summary(int(found))
        if label in shared.get(found, []):
            want = (int(pre.split("=")[1]) if pre.startswith("gas=") else -1) if expected == "applied" else limit
            groups.setdefault(found, {})[label] = (want, len(mine), expected, limit)
            continue
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

for blk, members in shared.items():
    got = groups.get(blk, {})
    if sorted(got) != sorted(members):
        report(False, f"shared block {blk}: recorded {members}, resolved there {sorted(got)}")
        continue
    exec_gas, stor_gas = fee_summary(int(blk))
    want_exec = sum(v[0] for v in got.values()); want_stor = sum(v[1] for v in got.values())
    for v in got.values():
        if v[2] == "applied":
            applied += 1
        else:
            refused += 1
    good = exec_gas == want_exec and stor_gas == want_stor and all(v[0] < v[3] for v in got.values() if v[2] == "applied")
    report(good, f"shared block {blk}: {', '.join(f'{k} ({v[2]}, {v[0]})' for k, v in got.items())}; charged {exec_gas} gas = their sum, storage {stor_gas} = their sizes, no other charged transaction" if good
           else f"shared block {blk}: charged {exec_gas} exec / {stor_gas} storage, the members imply {want_exec} / {want_stor}")

ctl = secrets.token_hex(32)
res = call("getTransaction", ctl)
report("result" in res and res["result"] is None, f"control: a random hash ({ctl[:12]}…) does not resolve, and the endpoint answers")

# ---- the Ethereum side: the releases ----
ERRORS = {"NotThisRelease": "0x97ed60d9", "BadOpening": "0xfbafac0f", "NotFinal": "0xd7c25c79", "RootNotAttested": "0xfb0612b1",
          "AlreadyReleased": "0x63b4904e", "VerificationFailed": "0x439cc0cd", "IsFrozen": "0x41dbb51e", "InvalidAmount": "0x2c5211c6",
          "UnsupportedToken": "0x6a172882", "CapExceeded": "0xa4875a49", "TransferFailed": "0x90b8ec18"}
NULLIFIER_SPENT = "0x38c86911"
eth_lines = [l.split() for l in lines if l.startswith("# eth ")]
eth_applied = eth_refused = 0
if eth_lines and not eth_rpc:
    report(False, "the manifest records Ethereum releases: pass --eth-rpc")
elif eth_rpc:
    def eth(method, *params):
        r = call(method, list(params), url=eth_rpc, many=True)
        return r
    vault = next((l.split()[3] for l in lines if l.startswith("# anvil vault ")), None)
    root_line = next((l.split() for l in lines if l.startswith("# release-root ")), None)
    release_root = root_line[2] if root_line else None
    if root_line:
        _, _, r, burns, program = root_line[:5]
        b = shard(burns, program)
        got = "0x" + b[-32:].hex() if len(b) == 812 else None
        report(got == r, f"release-root {r[:18]}… is the LEZ burn tree's root on the sequencer now ({burns[:12]}…)" if got == r
               else f"release-root {r[:18]}… is not the LEZ burn tree's root (the sequencer has {got})")
    for p in eth_lines:
        label, expected, verdict, block, tx, sel, gas, nf, root = p[2:11]
        rc = eth("eth_getTransactionReceipt", tx).get("result")
        if not rc:
            report(False, f"eth {label}: {tx[:16]}… NOT FOUND on anvil"); continue
        status, b, g = int(rc["status"], 16), int(rc["blockNumber"], 16), int(rc["gasUsed"], 16)
        why = []
        if str(b) != block: why.append(f"in block {b}, recorded {block}")
        if str(g) != gas: why.append(f"used {g} gas, recorded {gas}")
        if (rc.get("to") or "").lower() != (vault or "").lower(): why.append("not a call to the vault")
        if expected == "applied":
            eth_applied += 1
            logs = [x for x in rc["logs"] if x["address"].lower() == (vault or "").lower() and len(x["topics"]) == 3]
            if status != 1 or verdict != "applied": why.append(f"status {status}, recorded {verdict}")
            if not any(x["topics"][1] == nf for x in logs): why.append("no Release event of the vault names the recorded nullifier")
            if release_root is None or root != release_root: why.append("its root is not the release-root")
            report(not why, f"eth {label}: applied, {tx[:16]}… in block {b}, {g} gas, Release of nullifier {nf[:14]}… under the LEZ burn root" if not why
                   else f"eth {label}: " + "; ".join(why))
        else:
            eth_refused += 1
            if status != 0 or verdict != "reverted": why.append(f"status {status}, recorded {verdict}")
            if ERRORS.get(expected) != sel: why.append(f"recorded selector {sel} is not {expected}'s")
            t = eth("eth_getTransactionByHash", tx).get("result") or {}
            rep = eth("eth_call", {"from": t.get("from"), "to": t.get("to"), "data": t.get("input"), "gas": t.get("gas"), "value": t.get("value")}, hex(b - 1))
            got = ((rep.get("error") or {}).get("data") or "")[:10]
            if got != sel: why.append(f"replayed at block {b - 1} it reverts with {got or 'nothing'}, recorded {sel}")
            report(not why, f"eth {label}: refused {expected} ({sel}), {tx[:16]}… in block {b}, replayed: the same revert" if not why
                   else f"eth {label}: " + "; ".join(why))
    zkl = next((l.split() for l in lines if l.startswith("# anvil zk-verifier ")), None)
    if zkl:
        zv, r0, image = zkl[3], zkl[5], zkl[7]
        ebytes = lambda h: bytes.fromhex(h[2:] if h.startswith("0x") else h)
        def ecall(to, data, block="latest"):
            return eth("eth_call", {"to": to, "data": data}, block)
        def word_of(res): return (res.get("result") or "0x")[-64:].lower()
        SEL = {"IMAGE_ID()": "0x549206ed", "RISC0()": "0x11efe2db", "VERIFIER()": "0x08c84e70",
               "releaseDigest": "0x02826c0e", "verify": "0xab750e75"}
        committed = next((l.split() for l in lines if l.startswith("# binary burn_membership ")), None)
        cimg = "0x" + committed[committed.index("image_id") + 1] if committed else None
        report(cimg == image, f"zk: the zk verifier's recorded image id is the committed guest's ({image[:18]}…)" if cimg == image
               else f"zk: recorded image id {image[:18]}… is not the committed guest's ({cimg})")
        got = "0x" + word_of(ecall(zv, SEL["IMAGE_ID()"]))
        report(got == image.lower(), f"zk: ZkBurnVerifier.IMAGE_ID() = {got[:18]}…, the recorded image id" if got == image.lower()
               else f"zk: ZkBurnVerifier.IMAGE_ID() = {got[:18]}…, recorded {image[:18]}…")
        got = "0x" + word_of(ecall(zv, SEL["RISC0()"]))[-40:]
        report(got == r0.lower(), f"zk: ZkBurnVerifier.RISC0() is the recorded RiscZeroGroth16Verifier {r0[:12]}…" if got == r0.lower()
               else f"zk: ZkBurnVerifier.RISC0() = {got}, recorded {r0}")
        got = "0x" + word_of(ecall(vault, SEL["VERIFIER()"]))[-40:]
        report(got == zv.lower(), f"zk: the vault's VERIFIER() is the zk verifier {zv[:12]}…" if got == zv.lower()
               else f"zk: the vault's VERIFIER() = {got}, recorded zk verifier {zv}")
        openings = {}
        for l in lines:
            q = l.split()
            if l.startswith("# release-proof "):
                openings.setdefault(q[2], {})["leaf"] = q[q.index("leaf") + 1]
            elif l.startswith("# release-path "):
                openings.setdefault(q[2], {})["path"] = [] if q[3] == "-" else q[3].split(",")
        def u(b, o): return int.from_bytes(b[o:o + 32], "big")
        for p in eth_lines:
            label, expected, verdict, block, tx, _sel, gas, nf, root = p[2:11]
            if expected != "applied":
                continue
            t = eth("eth_getTransactionByHash", tx).get("result") or {}
            inp = ebytes(t.get("input") or "0x")
            why = []
            try:
                a = inp[4:]
                args = [a[32 * i:32 * (i + 1)] for i in range(7)]
                po = u(a, 7 * 32); proof = a[po + 32:po + 32 + u(a, po)]
                to = u(proof, 0)
                tup = proof[to:]
                def dyn(k):
                    o = u(tup, 32 * k); return tup[o + 32:o + 32 + u(tup, o)]
                seal, journal = dyn(0), dyn(1)
            except Exception as e:
                report(False, f"zk {label}: the input does not decode as a zero-knowledge release ({e})"); continue
            jd = hashlib.sha256(journal).hexdigest()
            rd = word_of(ecall(vault, SEL["releaseDigest"] + b"".join(args).hex(), hex(int(block) - 1)))
            if jd != rd: why.append(f"sha256(journal) {jd[:12]}… is not the vault's releaseDigest {rd[:12]}…")
            if len(journal) != 352 or "0x" + journal[288:320].hex() != root or "0x" + journal[320:352].hex() != nf:
                why.append("the journal's root or nullifier is not the recorded one")
            def verify_call(sl):
                data = (SEL["verify"] + (96).to_bytes(32, "big").hex() + image[2:].rjust(64, "0") + jd
                        + len(sl).to_bytes(32, "big").hex() + sl.hex() + "00" * ((32 - len(sl) % 32) % 32))
                return eth("eth_call", {"to": r0, "data": data, "gas": hex(5_000_000)}, "latest")
            r1 = verify_call(seal)
            if "error" in r1: why.append(f"RISC Zero's verifier refuses the seal: {r1['error'].get('message')}")
            flipped = bytearray(seal); flipped[40] ^= 1
            r2 = verify_call(bytes(flipped))
            if "error" not in r2: why.append("RISC Zero's verifier accepts the seal with a byte flipped")
            owner = label.replace("release-", "")
            o = openings.get(owner)
            hexin = inp.hex()
            if not o or "leaf" not in o: why.append(f"no recorded opening for {owner}")
            else:
                if o["leaf"][2:].lower() in hexin: why.append("the burn leaf occurs in the input")
                sent = [n for n in o.get("path", []) if n[2:].lower() in hexin]
                if sent: why.append(f"{len(sent)} path node(s) occur in the input")
            report(not why, f"zk {label}: seal ({len(seal)} B) and journal read back from {tx[:16]}…; sha256(journal) = the vault's digest; "
                            f"RISC Zero's verifier accepts it for {image[:12]}… and refuses it altered; leaf and {len((o or {}).get('path', []))} path node(s) absent"
                   if not why else f"zk {label}: " + "; ".join(why))
    for l in lines:
        if l.startswith("# eth-final nullifier ") or l.startswith("# eth-final unspent "):
            _, _, _, nf, want, name = l.split()[:6]
            r = eth("eth_call", {"to": vault, "data": NULLIFIER_SPENT + nf[2:]}, "latest").get("result") or ""
            got = "true" if r and int(r, 16) == 1 else "false"
            report(got == want, f"eth final: nullifierSpent({nf[:14]}…) = {got} ({name})" if got == want else f"eth final: nullifierSpent({nf[:14]}…) = {got}, recorded {want} ({name})")
        elif l.startswith("# eth-final balance "):
            _, _, _, addr, want, name = l.split()[:6]
            got = str(int(eth("eth_getBalance", addr, "latest").get("result") or "0x0", 16))
            report(got == want, f"eth final: {name} holds {got} wei" if got == want else f"eth final: {name} holds {got} wei, recorded {want}")
    ctl = "0x" + secrets.token_hex(32)
    r = eth("eth_getTransactionReceipt", ctl)
    report("result" in r and r["result"] is None, f"control: a random transaction hash ({ctl[:12]}…) does not resolve on anvil, and the endpoint answers")
print(f"{ok} checks pass, {bad} fail ({applied} applied and {refused} refused public transactions, {private} privacy-preserving, "
      f"{eth_applied} applied and {eth_refused} refused releases, in the manifest)")
sys.exit(1 if bad else 0)
PY

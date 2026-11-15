#!/usr/bin/env python3
"""Re-check the public outbound run (evidence/testnet-outbound/) with public
JSON-RPC only: the Sepolia vault's release, read back from its calldata, against
the LEZ testnet burn it pays out.

    verify-outbound.py --manifest FILE --rpc LEZ_URL --eth-rpc SEPOLIA_URL [--bedrock-dir DIR] [--bedrock-tools BIN]

scripts/verify-onchain.sh checks the LEZ transactions and the Ethereum receipts
(status, gas, the Release event, each refusal's revert replayed). This adds,
for the committee verifier the deployed vault uses:

- the deployment: the vault's VERIFIER() and PAIRING(), the verifier's members(),
  THRESHOLD() and FINALITY_DEPTH() are the recorded ones;
- for every applied release, decoded from the transaction's own input: the
  opening (token, amount, recipient, fee, secret, index, path) recomputed here
  with SHA-256 (ethCommitment, burnLeaf, rootFromPath, burnNullifier) gives the
  root and nullifier the vault was called with, and the recorded ones;
- the LEZ burn it pays out: the recorded burn transaction is on the LEZ
  testnet in the recorded block, and carries the burn's Ethereum commitment
  (a public Burn's instruction) or its leaf (a private BurnFrom's Append);
- the attestation: every signature recovers (EIP-191 over attestationDigest,
  recomputed here) to a distinct member, at least THRESHOLD of them;
  lezFinalBlock >= lezBlock + FINALITY_DEPTH; burn block <= lezBlock <=
  lezFinalBlock <= the LEZ tip now; and the LEZ blocks from the burn's to the
  final one are one hash-linked chain (each block's prev field is the hash
  field of the one before);
- the payout: the recipient's balance went up by amount - fee in that block,
  and the vault's down by amount;
- the burn tree: the released openings, by index, rebuild the LEZ burn tree's
  root now and its leaf count (every leaf it holds is a released burn);
- with --bedrock-dir (the members' saved Bedrock blocks): each attestation's
  final LEZ block, read from the public LEZ RPC, is byte for byte the
  inscription on the LEZ channel opening a transaction of the recorded Bedrock
  block, whose parent links lead down from the node's tip at signing, at
  least k = 30 blocks above it; with
  --bedrock-tools, that tool re-derives every header id, body root and
  transaction hash with the node's own types and checks the inscription's
  Ed25519 signature and its depth (`bedrock-tools anchor`).

Exits non-zero on any mismatch.
"""
import argparse
import base64
import hashlib
import json
import os
import secrets
import subprocess
import sys
import time
import urllib.request

from eth_abi import decode as abi_decode, encode as abi_encode
from eth_account import Account
from Crypto.Hash import keccak as _keccak

ATTEST_TAG = b"rfp022/burn-root-attestation/v1".ljust(32, b"\0")
ETH_COMMIT_TAG = b"rfp022/eth-commit/v1".ljust(32, b"\0")
RELEASE_EVENT = None  # topic0 computed below
LEZ_CHANNEL = "01" * 32
BEDROCK_K = 30   # Bedrock's security parameter in the node's deployment settings

ok_n = bad_n = 0


def report(good, text):
    global ok_n, bad_n
    if good:
        ok_n += 1; print(f"  ok   {text}")
    else:
        bad_n += 1; print(f"  FAIL {text}")


def keccak(b):
    return _keccak.new(digest_bits=256, data=b).digest()


def call(url, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    for i in range(5):   # public endpoints drop connections now and then: back off and retry
        try:
            req = urllib.request.Request(url, body, {"Content-Type": "application/json", "User-Agent": "portage-verify"})
            with urllib.request.urlopen(req, timeout=30) as r:
                return json.load(r)
        except Exception:  # noqa: BLE001
            if i == 4:
                raise
            time.sleep(2 ** i)


def sel(sig):
    return keccak(sig.encode())[:4].hex()


def eth_call(url, to, sig, types, args, out_types, block="latest"):
    data = "0x" + sel(sig) + abi_encode(types, args).hex()
    r = call(url, "eth_call", [{"to": to, "data": data}, block])
    if "error" in r:
        raise RuntimeError(f"{sig}: {r['error']}")
    return abi_decode(out_types, bytes.fromhex(r["result"][2:]))


def le(v, n):
    return v.to_bytes(n, "little")


def sha(b):
    return hashlib.sha256(b).digest()


def root_from_path(leaf, index, path):
    node = leaf
    for level in range(24):
        node = sha(path[level] + node) if (index >> level) & 1 else sha(node + path[level])
    return node


def tree_root(leaves):
    """The root of the depth-24 tree holding `leaves` from index 0, zeros elsewhere."""
    level, zero = list(leaves), b"\0" * 32
    for _ in range(24):
        if len(level) % 2:
            level.append(zero)
        level = [sha(level[i] + level[i + 1]) for i in range(0, len(level), 2)] or [sha(zero + zero)]
        zero = sha(zero + zero)
    return level[0] if leaves else zero


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", default="evidence/testnet-outbound/run.tsv")
    ap.add_argument("--rpc", default="https://testnet.lez.logos.co")
    ap.add_argument("--eth-rpc", default="https://rpc.sepolia.ethpandaops.io")
    ap.add_argument("--bedrock-dir")
    ap.add_argument("--bedrock-tools")
    a = ap.parse_args()
    lines = open(a.manifest).read().splitlines()
    lez = lambda m, p: call(a.rpc, m, p).get("result")
    eth = lambda m, p: call(a.eth_rpc, m, p).get("result")

    def line(prefix):
        return next((l.split() for l in lines if l.startswith(prefix)), None)

    ev = line("# eth-vault ")
    if not ev:
        report(False, "no '# eth-vault' line"); return finish()
    vault, verifier = ev[2], ev[4]
    committee = ev[6].split(",")
    threshold, depth = int(ev[8]), int(ev[10])
    net = line("# network ")
    pairing = net[net.index("pairing") + 1] if net else None

    # ---- the deployment as Sepolia has it now ----
    got = eth_call(a.eth_rpc, vault, "VERIFIER()", [], [], ["address"])[0]
    report(got.lower() == verifier.lower(), f"the vault {vault[:10]}…'s VERIFIER() is the recorded CommitteeBurnVerifier {verifier[:10]}…" if got.lower() == verifier.lower()
           else f"the vault's VERIFIER() is {got}, recorded {verifier}")
    got = "0x" + eth_call(a.eth_rpc, vault, "PAIRING()", [], [], ["bytes32"])[0].hex()
    report(got == pairing, f"the vault's PAIRING() is the inbound run's pairing {str(pairing)[:18]}…" if got == pairing else f"PAIRING() {got}, recorded {pairing}")
    got = [x.lower() for x in eth_call(a.eth_rpc, verifier, "members()", [], [], ["address[]"])[0]]
    report(got == [c.lower() for c in committee], f"the verifier's members() are the recorded {len(committee)}" if got == [c.lower() for c in committee]
           else f"members() {got}, recorded {committee}")
    got = eth_call(a.eth_rpc, verifier, "THRESHOLD()", [], [], ["uint256"])[0]
    report(got == threshold, f"THRESHOLD() = {got}" if got == threshold else f"THRESHOLD() = {got}, recorded {threshold}")
    got = eth_call(a.eth_rpc, verifier, "FINALITY_DEPTH()", [], [], ["uint64"])[0]
    report(got == depth, f"FINALITY_DEPTH() = {got} LEZ blocks" if got == depth else f"FINALITY_DEPTH() = {got}, recorded {depth}")

    proofs = {}
    for l in lines:
        if l.startswith("# release-proof "):
            p = l.split(); kv = {p[i]: p[i + 1] for i in range(3, len(p) - 1, 2)}
            proofs[p[2]] = kv
    atts = {}
    for l in lines:
        if l.startswith("# eth-attestation "):
            p = l.split(); atts[p[2]] = p
    tip = int(lez("getLastBlockId", []))
    released = {}   # index -> leaf
    lez_block_cache = {}

    def lez_block(n):
        if n not in lez_block_cache:
            r = lez("getBlock", [n])
            lez_block_cache[n] = base64.b64decode(r) if r else None
        return lez_block_cache[n]

    release_sig = sel("release(address,uint256,address,address,uint256,bytes32,bytes32,bytes)")
    RELEASE_T = "(address,uint256,address,address,uint256,bytes32,uint64,bytes32[24],uint64,uint64,bytes[])"
    for l in lines:
        if not l.startswith("# eth "):
            continue
        p = l.split()
        label, expected, verdict, block, txh, _sel, _gas, nf, root = p[2:11]
        if expected != "applied":
            continue
        owner = label.replace("release-", "")
        pr = proofs.get(owner)
        if not pr:
            report(False, f"{label}: no release-proof line for {owner}"); continue
        t = eth("eth_getTransactionByHash", [txh]) or {}
        inp = bytes.fromhex((t.get("input") or "0x")[2:])
        why = []
        if inp[:4].hex() != release_sig:
            report(False, f"{label}: {txh[:14]}… is not a call to release()"); continue
        (token, amount, recipient, relayer, fee, root_arg, nf_arg, proof) = abi_decode(
            ["address", "uint256", "address", "address", "uint256", "bytes32", "bytes32", "bytes"], inp[4:])
        (r_token, r_amount, r_recipient, r_relayer, r_fee, secret, index, path, lez_b, lez_f, sigs) = abi_decode([RELEASE_T], proof)[0]
        if (r_token, r_amount, r_recipient, r_relayer, r_fee) != (token, amount, recipient, relayer, fee):
            why.append("the proof's release arguments differ from the call's")
        ec = sha(abi_encode(["bytes32", "address", "uint256", "bytes32"], [ETH_COMMIT_TAG, recipient, fee, secret]))
        leaf = sha(b"rfp022/burn/v1" + bytes.fromhex(pairing[2:]) + bytes.fromhex(token[2:]) + le(amount, 16) + ec)
        rt = root_from_path(leaf, index, list(path))
        nfc = sha(b"rfp022/burn-nullifier/v1" + bytes.fromhex(pairing[2:]) + secret + le(index, 8))
        if rt != root_arg or "0x" + rt.hex() != root:
            why.append(f"the opening leads to root 0x{rt.hex()[:12]}…, the call names 0x{root_arg.hex()[:12]}…, recorded {root[:14]}…")
        if nfc != nf_arg or "0x" + nfc.hex() != nf:
            why.append("the opening's nullifier is not the call's or the recorded one")
        for k, v in (("leaf", "0x" + leaf.hex()), ("eth_commitment", "0x" + ec.hex()), ("recipient", recipient), ("amount", str(amount)),
                     ("fee", str(fee)), ("index", str(index)), ("root", root), ("nullifier", nf)):
            if pr.get(k, "").lower() != v.lower():
                why.append(f"recorded {k} {pr.get(k)} is not the release's {v}")
        report(not why, f"{label}: the opening in {txh[:14]}…'s input gives leaf 0x{leaf.hex()[:12]}…, root and nullifier = the call's and the recorded ones; "
                        f"{amount} wei to {recipient[:10]}…, fee {fee}" if not why else f"{label}: " + "; ".join(why))
        released[index] = leaf

        # The LEZ burn it pays out.
        why = []
        res = lez("getTransaction", [pr["burn_tx"]])
        if not (isinstance(res, list) and len(res) > 1):
            report(False, f"{label}: LEZ burn {pr['burn_tx'][:14]}… NOT FOUND on the LEZ testnet"); continue
        btx = base64.b64decode(res[0]); bblk = int(res[1] if not isinstance(res[1], dict) else res[1].get("height"))
        if hashlib.sha256(btx[1:]).hexdigest() != pr["burn_tx"]:
            why.append("its bytes do not hash to its id")
        if str(bblk) != pr.get("burn_block"):
            why.append(f"in LEZ block {bblk}, recorded {pr.get('burn_block')}")
        private = pr.get("private") == "true"
        needle, what = (leaf, "leaf (its Append)") if private else (ec, "Ethereum commitment (its instruction)")
        if (btx[0] == 1) != private:
            why.append(f"a {'privacy-preserving' if btx[0] == 1 else 'public'} transaction, recorded private={private}")
        if needle not in btx:
            why.append(f"it does not carry the release's {what}")
        report(not why, f"{label}: pays out LEZ burn {pr['burn_tx'][:14]}… ({'privacy-preserving' if private else 'public'}, block {bblk}), which carries the release's {what}"
               if not why else f"{label}: LEZ burn: " + "; ".join(why))
        burn_ok = not why

        # The attestation.
        why = []
        digest = keccak(abi_encode(["bytes32", "uint256", "address", "bytes32", "bytes32", "uint256", "uint256"],
                                   [ATTEST_TAG, 11155111, vault, bytes.fromhex(pairing[2:]), rt, lez_b, lez_f]))
        signed = keccak(b"\x19Ethereum Signed Message:\n32" + digest)
        seen = []
        for s in sigs:
            try:
                who = Account._recover_hash(signed, signature=s)
            except Exception:  # noqa: BLE001
                continue
            if int.from_bytes(s[32:64], "big") > 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0:
                continue
            if who.lower() in [c.lower() for c in committee] and who.lower() not in seen:
                seen.append(who.lower())
        if len(seen) < threshold:
            why.append(f"{len(seen)} distinct members signed, threshold {threshold}")
        if lez_f < lez_b + depth:
            why.append(f"final block {lez_f} is not {depth} above {lez_b}")
        if not (bblk <= lez_b <= lez_f <= tip):
            why.append(f"not burn block {bblk} <= lezBlock {lez_b} <= final {lez_f} <= LEZ tip {tip}")
        at = atts.get(label)
        if not at or at[at.index("lez_block") + 1] != str(lez_b) or at[at.index("lez_final_block") + 1] != str(lez_f):
            why.append("the recorded attestation blocks are not the release's")
        report(not why, f"{label}: {len(seen)} of {len(committee)} members signed root 0x{rt.hex()[:12]}… at LEZ block {lez_b}, final {lez_f} "
                        f"({lez_f - lez_b} >= {depth} deep), burn in block {bblk}, LEZ tip now {tip}" if not why else f"{label}: attestation: " + "; ".join(why))

        # The LEZ blocks from the burn's to the final one: one hash-linked chain (not walked from a
        # burn that is not the recorded one: that is already a failure, and the walk could be long).
        why = [] if burn_ok else ["not walked: the LEZ burn is not the recorded one"]
        prev_hash = None
        for n in (range(bblk, lez_f + 1) if burn_ok else ()):
            raw = lez_block(n)
            if raw is None or int.from_bytes(raw[:8], "little") != n:
                why.append(f"block {n} missing"); break
            if prev_hash is not None and raw[8:40] != prev_hash:
                why.append(f"block {n}'s prev is not block {n - 1}'s hash"); break
            prev_hash = raw[40:72]
        report(not why, f"{label}: LEZ blocks {bblk}..{lez_f} are one hash-linked chain ({lez_f - bblk + 1} blocks)" if not why else f"{label}: " + "; ".join(why))

        # The payout.
        rc = eth("eth_getTransactionReceipt", [txh]) or {}
        b = int(rc.get("blockNumber", "0x0"), 16)
        bal = lambda addr, blk: int(eth("eth_getBalance", [addr, hex(blk)]) or "0x0", 16)
        dr = bal(recipient, b) - bal(recipient, b - 1)
        dv = bal(vault, b - 1) - bal(vault, b)
        good = dr == amount - fee and dv == amount
        report(good, f"{label}: in Sepolia block {b} the recipient {recipient[:10]}… received {dr} wei (amount - fee) and the vault paid out {dv}" if good
               else f"{label}: the recipient received {dr} wei and the vault paid {dv} in block {b}, expected {amount - fee} and {amount}")

        # Bedrock, from the members' saved blocks.
        if a.bedrock_dir and at:
            notes = [x for x in at if x.startswith("m") and ":bedrock=" in x]
            for note in notes:
                bnote = note.split(":bedrock=")[1]
                if not bnote.startswith("lez"):
                    report(False, f"{label}: member {note.split(':')[0]} signed without a Bedrock check ({bnote})"); continue
                lez_n = int(bnote.split("@")[0][3:]); bid = bnote.split("@")[1].split(":")[0]
                mi = note.split(":")[0][1:]
                top16 = next((x[3:] for x in bnote.split(":") if x.startswith("top") and not x.startswith("top_")), "")
                depth_noted = next((int(x[5:]) for x in bnote.split(":") if x.startswith("depth")), -1)
                d = os.path.join(a.bedrock_dir, f"lez{lez_n}-top{top16}")
                why = []
                if lez_n != lez_f:
                    why.append(f"checked LEZ block {lez_n}, the release names {lez_f} final")
                try:
                    walk = json.load(open(os.path.join(d, "walk.json")))
                    blk = json.load(open(os.path.join(d, f"{bid}.json")))
                except OSError as e:
                    report(False, f"{label}: member {mi}: no saved Bedrock blocks ({e})"); continue
                ins = [op["payload"]["inscription"] for t_ in blk["transactions"] for op in t_["mantle_tx"]["ops"][:1]
                       if op["opcode"] == 17 and op["payload"]["channel_id"] == LEZ_CHANNEL]
                if (lez_block(lez_n) or b"").hex() not in [i.lower() for i in ins]:
                    why.append(f"Bedrock block {bid[:12]}… does not inscribe LEZ block {lez_n} as the LEZ RPC returns it")
                chain = walk["chain_top_down"]
                if chain[0] != walk["top"] or chain[-1] != bid:
                    why.append("the saved walk does not go from the node's tip to the inscription")
                if depth_noted != len(chain) - 1 or depth_noted < BEDROCK_K:
                    why.append(f"recorded depth {depth_noted}, the saved walk has {len(chain) - 1} blocks above the inscription, k = {BEDROCK_K}")
                for hi, lo in zip(chain, chain[1:]):
                    bj = json.load(open(os.path.join(d, f"{hi}.json")))
                    if bj["header"]["id"] != hi or bj["header"]["parent_block"] != lo:
                        why.append(f"block {hi[:12]}…'s parent is not {lo[:12]}…"); break
                report(not why, f"{label}: member {mi}: LEZ block {lez_n} is, byte for byte, the inscription in Bedrock block {bid[:12]}…, "
                                f"{len(chain) - 1} parent links (>= k = {BEDROCK_K}) below the node's tip {walk['top'][:12]}… when the member signed" if not why else f"{label}: member {mi}: " + "; ".join(why))
                if a.bedrock_tools and os.path.exists(os.path.join(d, "anchor.json")):
                    an = json.load(open(os.path.join(d, "anchor.json")))
                    r = subprocess.run([a.bedrock_tools, "anchor", d, "--channel", an["channel_id"], "--signer", an["signer"],
                                        "--lez-hash", (lez_block(lez_n) or b"\0" * 72)[40:72].hex(), "--depth", str(an["top"]["depth"]),
                                        "--out", os.devnull], capture_output=True, text=True)
                    good = r.returncode == 0 and an["top"]["id"] == walk["top"] and an["bedrock_block"]["id"] == bid
                    report(good, f"{label}: member {mi}: bedrock-tools anchor: LEZ block {lez_n}'s inscription in {bid[:12]}… (tx {an['bedrock_block']['tx_index']}, "
                                 f"path {an['bedrock_block']['path_len']}), signed by {an['signer'][:12]}…, {an['top']['depth']} headers above it"
                           if good else f"{label}: member {mi}: bedrock-tools anchor refuses: {(r.stderr or r.stdout).strip()[-200:]}")
                if a.bedrock_tools:
                    r = subprocess.run([a.bedrock_tools, "check", d], capture_output=True, text=True)
                    report(r.returncode == 0, f"{label}: member {mi}: bedrock-tools check: the node's own types agree on every saved block's id, body root and transactions"
                           if r.returncode == 0 else f"{label}: member {mi}: bedrock-tools check failed: {(r.stderr or r.stdout).strip()[-200:]}")

    # The burn tree: every leaf it holds is a released burn.
    rr = line("# release-root ")
    if rr:
        burns, prog = rr[3], rr[4]
        res = lez("getAccount", [burns]) or {}
        d = bytes(((res.get("data") or {}).get("shards") or {}).get(prog, []))
        n = int.from_bytes(d[:8], "little") if d else -1
        root_now = d[-32:] if d else b""
        rebuilt = tree_root([released[i] for i in sorted(released)]) if sorted(released) == list(range(len(released))) else None
        good = rebuilt == root_now and n == len(released) and "0x" + root_now.hex() == rr[2]
        report(good, f"the LEZ burn tree now holds {n} leaves with root 0x{root_now.hex()[:12]}…: exactly the {len(released)} released burns, rebuilt here" if good
               else f"the LEZ burn tree holds {n} leaves, root 0x{root_now.hex()[:12]}… (recorded {rr[2][:14]}…); the released openings rebuild "
                    f"{('0x' + rebuilt.hex()[:12]) if rebuilt else 'no contiguous tree'} from {len(released)}")
    else:
        report(False, "no release-root line")
    ctl = "0x" + secrets.token_hex(32)
    report(eth("eth_getTransactionByHash", [ctl]) is None, f"control: a random hash ({ctl[:12]}…) is no Sepolia transaction")
    return finish()


def finish():
    print(f"{ok_n} checks pass, {bad_n} fail")
    sys.exit(1 if bad_n else 0)


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as e:  # noqa: BLE001 - an endpoint that stops answering is a failure, said as one
        report(False, f"stopped: {type(e).__name__}: {str(e)[:200]}")
        finish()

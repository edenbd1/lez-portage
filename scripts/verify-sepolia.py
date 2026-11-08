#!/usr/bin/env python3
"""Re-check the Sepolia half of a public inbound run, and what ties it to LEZ,
from its manifest, with nothing but JSON-RPC.

    python3 scripts/verify-sepolia.py --manifest evidence/testnet/run.tsv \\
        [--eth-rpc https://ethereum-sepolia-rpc.publicnode.com] [--rpc https://testnet.lez.logos.co]

Checks:
- the vault has code, and its PAIRING and VERIFIER are the recorded ones;
- every recorded deposit: its receipt has status 1, is in the recorded block,
  sends exactly the amount to the vault, and carries the vault's Deposit event
  with the recorded index, leaf, amount and root; its block is at or before the
  checkpoint's execution block;
- the checkpoint's execution block is on Sepolia's chain now with the recorded
  hash and state root, and the committed Sextant bootstrap commits to that
  block hash (evidence/testnet/sepolia/bootstrap-*.json);
- the committed storage proof the claims used (sepolia/proofs.json):
  its first account node hashes to that state root, it is the vault's
  knownRoot[root] slot, and its value is 1; knownRoot[root] is true on Sepolia
  now, and false for a random root (a control);
- on LEZ: Sextant's header record for the checkpoint slot holds that state
  root, and the public claim's transaction carries the root it proved and the
  vault's address.
"""
import argparse
import base64
import glob
import json
import os
import secrets
import sys
import urllib.request

from Crypto.Hash import keccak as _k


def keccak(b):
    h = _k.new(digest_bits=256)
    h.update(b)
    return h.digest()


def call(url, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    for attempt in range(4):
        try:
            req = urllib.request.Request(url, body, {"Content-Type": "application/json", "user-agent": "portage-verify"})
            with urllib.request.urlopen(req, timeout=40) as r:
                return json.load(r).get("result")
        except Exception:
            if attempt == 3:
                raise


ok = bad = 0


def report(good, text):
    global ok, bad
    if good:
        ok += 1
        print(f"  ok   {text}")
    else:
        bad += 1
        print(f"  FAIL {text}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", default="evidence/testnet/run.tsv")
    ap.add_argument("--eth-rpc", default="https://ethereum-sepolia-rpc.publicnode.com")
    ap.add_argument("--rpc", default="https://testnet.lez.logos.co")
    a = ap.parse_args()
    lines = open(a.manifest).read().splitlines()
    sep = os.path.join(os.path.dirname(a.manifest), "sepolia")
    E = lambda m, p: call(a.eth_rpc, m, p)
    vault = verifier = pairing = None
    deposits, ckpt, sextant, claim_tx = [], None, None, None
    for l in lines:
        w = l.split()
        if l.startswith("# sepolia vault "):
            vault, verifier, pairing = w[3], w[5], w[7]
        elif l.startswith("# sepolia checkpoint-block "):
            ckpt = (int(w[3]), w[4], w[6])
        elif l.startswith("# sepolia deposit "):
            deposits.append({"index": int(w[3]), "amount": int(w[5]), "leaf": w[7], "root": w[9], "tx": w[11], "block": int(w[13])})
        elif l.startswith("# sextant instance "):
            sextant = {"name": w[3], "instance": w[4], "header": w[6], "slot": int(w[8])}
        elif l.startswith("claim-alice\t"):
            claim_tx = l.split("\t")[4]
    assert vault and ckpt and deposits and sextant and claim_tx, "the manifest lacks a sepolia, sextant or claim line"
    print(f"== Sepolia: vault {vault}, {len(deposits)} deposits, checkpoint execution block {ckpt[0]}")
    report(len(E("eth_getCode", [vault, "latest"]) or "0x") > 2, f"the vault {vault} has code")
    word = lambda sel: E("eth_call", [{"to": vault, "data": "0x" + keccak(sel.encode()).hex()[:8]}, "latest"])
    report(word("PAIRING()") == pairing, f"its PAIRING is {pairing}")
    report(int(word("VERIFIER()"), 16) == int(verifier, 16), f"its VERIFIER is {verifier}")
    topic = "0x" + keccak(b"Deposit(uint256,bytes32,bytes32,address,uint256,bytes32)").hex()
    for d in deposits:
        r = E("eth_getTransactionReceipt", [d["tx"]]) or {}
        t = E("eth_getTransactionByHash", [d["tx"]]) or {}
        logs = [x for x in r.get("logs", []) if x["address"].lower() == vault.lower() and x["topics"][0] == topic]
        good = r.get("status") == "0x1" and int(r.get("blockNumber", "0x0"), 16) == d["block"] and (t.get("to") or "").lower() == vault.lower() \
            and int(t.get("value", "0x0"), 16) == d["amount"] and len(logs) == 1
        if good:
            data = logs[0]["data"][2:]
            ws = ["0x" + data[i:i + 64] for i in range(0, len(data), 64)]
            good = int(logs[0]["topics"][1], 16) == d["index"] and ws[0] == d["leaf"] and int(ws[2], 16) == d["amount"] and ws[3] == d["root"]
        report(good, f"deposit {d['index']}: {d['tx']} in block {d['block']}, {d['amount']} wei into the vault, its Deposit event (leaf {d['leaf'][:18]}..., root {d['root'][:18]}...)")
        report(d["block"] <= ckpt[0], f"deposit {d['index']} is at or before the checkpoint's execution block")
    b = E("eth_getBlockByNumber", [hex(ckpt[0]), False]) or {}
    report(b.get("hash") == ckpt[1] and b.get("stateRoot") == ckpt[2], f"Sepolia block {ckpt[0]} is {ckpt[1][:18]}... with state root {ckpt[2][:18]}...")
    boots = glob.glob(os.path.join(sep, "bootstrap-*.json"))
    hdr = json.load(open(boots[0]))["data"]["header"] if len(boots) == 1 else {}
    bh = hdr.get("execution_block_hash") or (hdr.get("execution") or {}).get("block_hash")
    report(bh == ckpt[1] and int(hdr["beacon"]["slot"]) == sextant["slot"], f"the committed bootstrap (slot {sextant['slot']}) commits to that execution block")
    last = deposits[-1]["root"]
    proofs = {p["root"]: p for p in json.load(open(os.path.join(sep, "proofs.json")))}
    p = proofs.get(last, {}).get("proof", {}).get("result", {})
    slot = "0x" + keccak(bytes.fromhex(last[2:]) + bytes(32)).hex()
    good = bool(p) and "0x" + keccak(bytes.fromhex(p["accountProof"][0][2:])).hex() == ckpt[2] and p["address"].lower() == vault.lower() \
        and int(p["storageProof"][0]["key"], 16) == int(slot, 16) and int(p["storageProof"][0]["value"], 16) == 1
    report(good, f"the committed proof of knownRoot[{last[:18]}...] is under that state root, the vault's slot {slot[:18]}..., value 1")
    kr = lambda root: E("eth_call", [{"to": vault, "data": "0x" + keccak(b"knownRoot(bytes32)").hex()[:8] + root[2:]}, "latest"])
    report(int(kr(last), 16) == 1, "knownRoot[root] is true on Sepolia now")
    report(int(kr("0x" + secrets.token_hex(32)), 16) == 0, "control: knownRoot of a random root is false")
    print(f"== LEZ: Sextant instance {sextant['name']}, header record {sextant['header']}")
    acc = call(a.rpc, "getAccount", [sextant["header"]]) or {}
    shards = (acc.get("data") or {}).get("shards") or {}
    rec = b"".join(bytes(v) for v in shards.values())
    report(bytes.fromhex(ckpt[2][2:]) in rec, "Sextant's header record for the checkpoint slot holds the checkpoint block's state root")
    tx = call(a.rpc, "getTransaction", [claim_tx])
    raw = b""
    if isinstance(tx, list) and tx:
        raw = base64.b64decode(tx[0]) if isinstance(tx[0], str) else bytes(tx[0])
    elif isinstance(tx, str):
        raw = base64.b64decode(tx)
    report(bytes.fromhex(last[2:]) in raw and bytes.fromhex(vault[2:]) in raw, f"the public claim {claim_tx[:16]}... carries that root and the vault's address")
    print(f"== {ok} checks pass, {bad} fail")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""The privacy check, with the controls that show it can fail.

A privacy-preserving transaction must not carry what it hides: the private
account (the claimant, the burner), the deposit leaf it claims, the Ethereum
recipients. The check is a byte search of the transaction as the chain serves
it. A byte search that never finds anything proves nothing, so the same
detector is run on things that DO leak, and must flag every one of them:

  1. the public transactions of the same runs (alice's claim and burn: same
     program, privacy off), which carry alice's account and the claimed leaf;
  2. a leaky build of each private transaction: its real bytes with one hidden
     value written into them, at the start, the middle and the end.

    scripts/privacy-check.py                  real build: every private tx clean, every control flagged (exit 0)
    scripts/privacy-check.py --build leaky    the leaky build as the build under test: must FAIL (exit 1)
    scripts/privacy-check.py --rpc URL        read the transactions from a live node instead of the archive

Offline by default: the transactions come from evidence/archive/lez.jsonl.gz,
and each one is checked against its own hash (sha256 of bytes[1:]) first.
"""
import argparse, base64, gzip, hashlib, json, sys, urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFESTS = [ROOT / "evidence/testnet/run.tsv", ROOT / "evidence/testnet-outbound/run.tsv"]
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def unb58(s):
    n = 0
    for c in s:
        n = n * 58 + B58.index(c)
    return n.to_bytes(32, "big")


def manifest():
    rows, accounts, private, leaves, recipients = {}, {}, {}, {}, {}
    for f in MANIFESTS:
        for l in f.read_text().splitlines():
            p = l.split()
            if l.startswith("# accounts "):
                accounts.update(dict(zip(p[2::2], p[3::2])))
            elif l.startswith("# private-account "):
                accounts[p[3]] = p[2]
            elif l.startswith("# private ") and " hides " in l:
                private[p[2]] = p[p.index("hides") + 1]
            elif l.startswith("# sepolia deposit "):
                leaves["deposit %s leaf" % p[3]] = bytes.fromhex(p[p.index("leaf") + 1][2:])
            elif l.startswith("# recipients "):
                recipients.update({"%s's Ethereum recipient" % k: bytes.fromhex(v[2:]) for k, v in zip(p[2::2], p[3::2])})
            elif l and not l.startswith("#"):
                c = l.split("\t")
                rows[c[0]] = (c[4], c[6] if len(c) > 6 else "")
    return rows, accounts, private, leaves, recipients


def fetch(rpc):
    if rpc is None:
        txs = {}
        with gzip.open(ROOT / "evidence/archive/lez.jsonl.gz", "rt") as f:
            for line in f:
                e = json.loads(line)
                r = e["res"].get("result")
                if e["req"].get("method") == "getTransaction" and isinstance(r, list):
                    txs[e["req"]["params"][0]] = base64.b64decode(r[0])
        return txs.get

    def get(h):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getTransaction", "params": [h]}).encode()
        req = urllib.request.Request(rpc, body, {"Content-Type": "application/json"})
        r = json.load(urllib.request.urlopen(req, timeout=60)).get("result")
        return base64.b64decode(r[0]) if isinstance(r, list) else None
    return get


def leaks(tx, secrets):
    """The detector: which of the named byte strings occur in the transaction."""
    return [name for name, value in secrets.items() if value in tx]


def leaky(tx, value):
    """A leaky build of a transaction: its bytes with a hidden value written in."""
    m = len(tx) // 2
    return [tx[:1] + value + tx[1:], tx[:m] + value + tx[m:], tx + value]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--build", choices=["real", "leaky"], default="real")
    ap.add_argument("--rpc")
    a = ap.parse_args()
    rows, accounts, private, leaves, recipients = manifest()
    get = fetch(a.rpc)
    ok = bad = 0

    def report(good, text):
        nonlocal ok, bad
        ok, bad = ok + good, bad + (not good)
        print(("  ok   " if good else "  FAIL ") + text)

    def load(label):
        h = rows[label][0]
        tx = get(h)
        if tx is None or hashlib.sha256(tx[1:]).hexdigest() != h:
            report(False, f"{label}: {h[:16]}… not found, or its bytes do not hash to it")
            return None
        return tx

    print(f"privacy check, build under test: {a.build}, transactions from {a.rpc or 'evidence/archive'}")
    # What a private transaction must not carry: every user account of the runs,
    # every deposit leaf, every Ethereum recipient.
    # (the admin and freezer roles act in public by design and are not among them)
    hidden = {f"{n}'s account {v[:8]}…": unb58(v) for n, v in accounts.items() if n not in ("admin", "freezer")}
    hidden.update(leaves)
    hidden.update(recipients)
    for label, account in sorted(private.items()):
        tx = load(label)
        if tx is None:
            continue
        if tx[0] != 1:
            report(False, f"{label}: not a privacy-preserving transaction")
            continue
        own = next(k for k, v in hidden.items() if v == unb58(account))
        builds = [tx] if a.build == "real" else [t for v in hidden.values() for t in leaky(tx, v)]
        found = sorted({n for t in builds for n in leaks(t, hidden)})
        report(not found, f"{label}: privacy-preserving, {len(tx):,} bytes; none of the {len(hidden)} hidden values "
               f"occurs in it (its own: {own})" if not found else f"{label}: carries {', '.join(found)}")
    # Controls: the detector must see what is really there.
    print("controls: the same detector on transactions that do carry these values")
    for label, must in (("claim-alice", ["alice's account", "deposit 1 leaf"]), ("burn-alice", ["alice's account"])):
        tx = load(label)
        if tx is None:
            continue
        found = leaks(tx, hidden)
        seen = all(any(f.startswith(m) for f in found) for m in must)
        report(seen, f"{label} (public, same program): the detector finds {', '.join(found) or 'nothing'}"
               + ("" if seen else f"; it must find {', '.join(must)}"))
    for label in sorted(private):
        tx = load(label)
        if tx is None:
            continue
        variants = [(n, t) for n, v in hidden.items() for t in leaky(tx, v)]
        caught = sum(n in leaks(t, hidden) for n, t in variants)
        report(caught == len(variants), f"{label}, leaky build: {caught} of {len(variants)} planted leaks found "
               f"({len(hidden)} values x start, middle, end)")
    print(f"{ok} checks pass, {bad} fail")
    print("== PASS" if bad == 0 else "== FAIL")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()

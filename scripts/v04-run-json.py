#!/usr/bin/env python3
"""Turn a v04/scripts/e2e-seq.sh run directory into vectors/lez-blocks-v04:
run.json (programs, config, burns, pairing, block range, applied burn leaves,
the burn tree root, every transaction's label, verdict, block, index and hash)
and the blocks themselves.

    python3 scripts/v04-run-json.py RUN vectors/lez-blocks-v04
"""
import json, os, shutil, sys

run, out = sys.argv[1], sys.argv[2]
progs = json.load(open(os.path.join(run, "programs.json")))
txs, meta = [], {}
for line in open(os.path.join(run, "manifest.tsv")):
    line = line.rstrip("\n")
    if line.startswith("# label") or not line:
        continue
    if line.startswith("# burn leaves"):
        meta["leaves"] = line.split(") ", 1)[1].split()
        continue
    if line.startswith("# burn tree root"):
        meta["root"] = line.split()[4]
        continue
    if line.startswith("# "):
        f = line[2:].split(" ")
        meta[f[0]] = f[1:]
        continue
    label, expected, verdict, block, tx, h = line.split("\t")
    txs.append({"label": label, "verdict": verdict, "block": int(block), "tx": int(tx), "hash": h})
first, last = [int(x) for x in meta["blocks"][0].split("..=")]
cfg = meta["config"]
os.makedirs(out, exist_ok=True)
for f in os.listdir(out):
    if f.endswith(".borsh"):
        os.remove(os.path.join(out, f))
for b in range(first, last + 1):
    shutil.copy(os.path.join(run, "blocks", f"{b}.borsh"), os.path.join(out, f"{b}.borsh"))
leaves = meta["leaves"]
doc = {
    "programs": progs,
    "config": cfg[0],
    "burns": cfg[2],
    "pairing": cfg[4],
    "first": first,
    "last": last,
    "applied_burn_leaves": leaves,
    "root": meta["root"],
    "transactions": txs,
}
json.dump(doc, open(os.path.join(out, "run.json"), "w"), indent=1)
print(f"{out}: blocks {first}..={last}, {len(txs)} transactions, {len(leaves)} applied burns")

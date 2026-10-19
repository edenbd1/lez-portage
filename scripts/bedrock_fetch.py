#!/usr/bin/env python3
"""Save a stretch of a Bedrock node's canonical chain, read-only.

  bedrock_fetch.py --node URL --out DIR [--count N] [--top ID] [--lib]
                   [--until-lez-hash HEX --channel HEX]

Walks parent links back from the node's tip (or from its LIB with --lib, or
from --top ID) for N blocks, with GET /cryptarchia/blocks/:id only, and writes
DIR/<id>.json (the API's block) and DIR/chain.json (the ids, oldest first, each
the parent of the next), plus DIR/info.json (the node's /cryptarchia/info when
the walk started). With --until-lez-hash, the walk stops at the block whose
transaction inscribes, on --channel, the LEZ block with that hash (bytes 40..72
of the inscription), --count then bounding it. Nothing is posted to the node.
"""
import argparse, json, os, sys, time, urllib.request


def get(node, path, tries=5):
    for i in range(tries):
        try:
            with urllib.request.urlopen(node.rstrip("/") + path, timeout=30) as r:
                return r.read()
        except Exception as e:  # noqa: BLE001
            if i == tries - 1:
                raise
            print(f"GET {path}: {e}, retrying", file=sys.stderr)
            time.sleep(1 + i)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--node", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--count", type=int, default=40)
    ap.add_argument("--top")
    ap.add_argument("--lib", action="store_true")
    ap.add_argument("--until-lez-hash")
    ap.add_argument("--channel")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    info = json.loads(get(a.node, "/cryptarchia/info"))
    json.dump(info, open(os.path.join(a.out, "info.json"), "w"), indent=1)
    ci = info["cryptarchia_info"]
    h = a.top or (ci["lib"] if a.lib else ci["tip"])
    ids, found = [], None
    for _ in range(a.count):
        raw = get(a.node, f"/cryptarchia/blocks/{h}")
        b = json.loads(raw)
        if b is None or b["header"]["id"] != h:
            sys.exit(f"block {h}: not returned as itself")
        open(os.path.join(a.out, f"{h}.json"), "wb").write(raw)
        ids.append(h)
        if a.until_lez_hash and any(
                op["opcode"] == 17 and op["payload"]["channel_id"] == a.channel
                and op["payload"]["inscription"][80:144] == (a.until_lez_hash[2:] if a.until_lez_hash.startswith("0x") else a.until_lez_hash)
                for t in b["transactions"] for op in t["mantle_tx"]["ops"][:1]):
            found = h
            break
        if b["header"]["slot"] == 0:
            break
        h = b["header"]["parent_block"]
    if a.until_lez_hash and not found:
        sys.exit(f"no inscription of LEZ block {a.until_lez_hash} on {a.channel} in the {len(ids)} blocks below {ids[0]}")
    ids.reverse()
    json.dump(ids, open(os.path.join(a.out, "chain.json"), "w"), indent=0)
    print(json.dumps({"blocks": len(ids), "top": ids[-1], "bottom": ids[0], "tip": ci["tip"], "lib": ci["lib"],
                      "lib_slot": ci["lib_slot"], "height": ci.get("height"), "found": found}))


if __name__ == "__main__":
    main()

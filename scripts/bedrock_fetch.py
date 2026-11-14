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

If that inscription is not found (within --count blocks, or before a block the
node has no body for), it exits 1 with one line saying so, the newest LEZ block
it did see inscribed on the channel, and the likely cause: the sequencer's
inscriptions are not landing.
"""
import argparse, json, os, sys, time, urllib.error, urllib.request


class Missing(Exception):
    pass


def get(node, path, tries=5):
    for i in range(tries):
        try:
            with urllib.request.urlopen(node.rstrip("/") + path, timeout=30) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code == 404:
                raise Missing(path) from None
            if i == tries - 1:
                raise
            print(f"GET {path}: {e}, retrying", file=sys.stderr)
            time.sleep(1 + i)
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
    want = a.until_lez_hash and (a.until_lez_hash[2:] if a.until_lez_hash.startswith("0x") else a.until_lez_hash).lower()
    ids, found, seen, newest, stop = [], None, 0, None, None
    for _ in range(a.count):
        try:
            raw = get(a.node, f"/cryptarchia/blocks/{h}")
        except Missing:
            stop = f"the node has no body for block {h} (HTTP 404)"
            break
        b = json.loads(raw)
        if b is None or b["header"]["id"] != h:
            sys.exit(f"block {h}: not returned as itself")
        open(os.path.join(a.out, f"{h}.json"), "wb").write(raw)
        ids.append(h)
        ins = [op["payload"]["inscription"] for t in b["transactions"] for op in t["mantle_tx"]["ops"][:1]
               if op["opcode"] == 17 and op["payload"]["channel_id"] == a.channel]
        seen += len(ins)
        if ins and newest is None:
            newest = int.from_bytes(bytes.fromhex(ins[0][:16]), "little")  # the LEZ header's block id
        if want and any(i[80:144].lower() == want for i in ins):
            found = h
            break
        if b["header"]["slot"] == 0:
            stop = "genesis"
            break
        h = b["header"]["parent_block"]
    if want and not found:
        sys.exit(f"LEZ block {a.until_lez_hash} is not inscribed on channel {a.channel} in the {len(ids)} Bedrock blocks walked down "
                 f"from the {'top' if a.top else 'LIB' if a.lib else 'tip'} {ids[0] if ids else h}"
                 f"{' (the walk stopped: ' + stop + ')' if stop else ''}; inscriptions on that channel among them: {seen}"
                 f"{f', the newest of LEZ block {newest}' if newest is not None else ''}. The sequencer's inscriptions are "
                 f"not landing (on a fresh devnet: see scripts/bedrock-devnet.sh, WARMUP_EPOCHS), or the block is newer "
                 f"than the node's tip.")
    if stop and stop != "genesis" and not want:
        sys.exit(f"the walk stopped after {len(ids)} blocks: {stop}")
    ids.reverse()
    json.dump(ids, open(os.path.join(a.out, "chain.json"), "w"), indent=0)
    print(json.dumps({"blocks": len(ids), "top": ids[-1], "bottom": ids[0], "tip": ci["tip"], "lib": ci["lib"],
                      "lib_slot": ci["lib_slot"], "height": ci.get("height"), "found": found}))


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""The outbound half of scripts/testnet-outbound.sh: attest a LEZ testnet burn
tree's root with the committee of the Sepolia vault's CommitteeBurnVerifier,
and release (or attempt to) from that vault.

    sepolia_release.py --lez URL --run DIR --proof PROOF.json --owner NAME --out MANIFEST
                       [--bedrock-node URL --bedrock-evidence DIR] [--refusals]
    sepolia_release.py --final --lez URL --run DIR --out MANIFEST

`PROOF.json` is `wrap release-proof <owner>`: the burn's opening (token,
amount, recipient, fee, secret, index, path), the root it leads to, its
nullifier, the LEZ burn transaction and the burn-tree account.

**What each member checks before it signs** (three keys, 2 of 3 needed; one
operator holds all three, which the manifest says):
1. It reads the burn tree's root from the public LEZ testnet itself
   (getAccount of the burn-tree account, between two equal reads of the tip),
   and signs **the root it read**, at the block it read it, never the one the
   release names.
2. It waits until the LEZ tip is `FINALITY_DEPTH` (30) blocks above that
   block, and names the tip it then reads as final.
3. With `--bedrock-node`: it reads the LEZ block it names as final
   (getBlock) and refuses to sign unless that block, byte for byte, is the
   inscription opening a transaction on the LEZ channel of a Bedrock block on
   the node's canonical chain with at least BEDROCK_DEPTH (30, Bedrock's
   security parameter k in the node's deployment settings) blocks above it.
   The Bedrock blocks from the node's tip down to that inscription are saved
   to `--bedrock-evidence`; with BEDROCK_TOOLS, bedrock-tools re-derives them
   with the node's own types and checks the inscription's Merkle path,
   signature and depth. Without a node it signs on LEZ depth alone, and the
   manifest says so.
It signs with its own key (EIP-191, eth_account): the verifier's
`attestationDigest(vault, pairing, root, lezBlock, lezFinalBlock)`, read with
eth_call and recomputed here.

**The release** is sent by the relayer (the deployer key), fee 0, to the
recipient the burn committed to, with an explicit gas limit after an eth_call
pre-check; a refusal must pre-check as its expected error and is mined too.
With `--refusals`: `NotFinal` (the members sign at depth 0), `RootNotAttested`
(one member), then the release, then `AlreadyReleased` (the replay).

Keys are read from files and never printed: $SEPOLIA_KEY_FILE (member 0 and
the relayer) and RUN/keys/committee.json (members 1 and 2).
"""
import argparse
import hashlib
import json
import os
import secrets
import subprocess
import sys
import time
import urllib.request

from eth_abi import encode as abi_encode
from eth_account import Account
from eth_account.messages import encode_defunct
from Crypto.Hash import keccak as _keccak

ETH_RPC = os.environ.get("SEPOLIA_RPC", "https://ethereum-sepolia-rpc.publicnode.com")
CHAIN_ID = 11155111
ATTEST_TAG = b"rfp022/burn-root-attestation/v1".ljust(32, b"\0")
ERRORS = ["NotThisRelease()", "BadOpening()", "NotFinal()", "RootNotAttested()", "AlreadyReleased()", "VerificationFailed()",
          "IsFrozen()", "InvalidAmount()", "UnsupportedToken()", "CapExceeded()", "TransferFailed()"]
RELEASE_T = "(address,uint256,address,address,uint256,bytes32,uint64,bytes32[24],uint64,uint64,bytes[])"
LEZ_CHANNEL = os.environ.get("LEZ_CHANNEL", "01" * 32)

def same_lez_block(inscription_hex, raw):
    """True when a Bedrock inscription is the LEZ block `raw`, byte for byte, except the last byte.

    The last byte of a Borsh-encoded LEZ block is its `bedrock_status` (Pending 0, Safe 1,
    Finalized 2: lez/common/src/block.rs, the last field of `Block`). It is the node's own view
    of the block's progress on Bedrock, not part of what the block hash covers, and it changes
    after the block is inscribed: a block inscribed while Pending is later returned Finalized.
    Every other byte, the header with its hash and the body, must match."""
    try:
        ins = bytes.fromhex(inscription_hex)
    except ValueError:
        return False
    return (raw is not None and len(ins) == len(raw) and len(raw) > 0
            and ins[:-1] == raw[:-1] and ins[-1] in (0, 1, 2) and raw[-1] in (0, 1, 2))



def keccak(b):
    return _keccak.new(digest_bits=256, data=b).digest()


def rpc(url, method, params, tries=6):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    for i in range(tries):
        try:
            req = urllib.request.Request(url, body, {"Content-Type": "application/json", "User-Agent": "portage-outbound"})
            with urllib.request.urlopen(req, timeout=30) as r:
                return json.load(r)
        except Exception as e:  # noqa: BLE001
            if i == tries - 1:
                raise
            print(f"{method}: {e}, retrying", file=sys.stderr)
            time.sleep(2 + 2 * i)


def ok(url, method, params):
    r = rpc(url, method, params)
    if "error" in r:
        sys.exit(f"{method}: {r['error']}")
    return r["result"]


def cast(*args):
    r = subprocess.run(["cast", *args], capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"cast {args[0]}: {r.stderr.strip()}")
    return r.stdout.strip()


def ecall(to, sig, *args, block="latest"):
    return cast("call", to, sig, *[str(x) for x in args], "--rpc-url", ETH_RPC, "--block", str(block))


def lez_tip(lez):
    return int(ok(lez, "getLastBlockId", []))


def lez_block_bytes(lez, n):
    import base64
    res = ok(lez, "getBlock", [n])
    if not res:
        sys.exit(f"no LEZ block {n}")
    return base64.b64decode(res)


def lez_burn_root(lez, burns, program):
    """The burn tree's root at the tip, and that tip (two equal reads around the account)."""
    for _ in range(40):
        t0 = lez_tip(lez)
        res = ok(lez, "getAccount", [burns]) or {}
        data = bytes(((res.get("data") or {}).get("shards") or {}).get(program, []))
        if lez_tip(lez) == t0:
            if len(data) != 8 + 4 + 24 * 32 + 32:
                sys.exit(f"the burn tree account holds {len(data)} bytes, not a frontier")
            return "0x" + data[-32:].hex(), int.from_bytes(data[:8], "little"), t0
        time.sleep(2)
    sys.exit("the LEZ tip kept moving while reading the burn tree")


def bedrock_get(node, path):
    with urllib.request.urlopen(node.rstrip("/") + path, timeout=30) as r:
        return r.read()


def bedrock_inscribed(node, lez_raw, evidence, label, max_walk=400):
    """The Bedrock block on the node's canonical chain whose first-op inscription on the LEZ
    channel is exactly `lez_raw`, walking parent links down from the node's tip, with the
    number of blocks above it; or None. The walk (tip first) is saved under `evidence`."""
    info = json.loads(bedrock_get(node, "/cryptarchia/info"))
    ci = info["cryptarchia_info"]
    if ci.get("state") != "Online":
        return None, ci, f"node not Online ({ci.get('state')})", []
    h, walked, newest = ci["tip"], [], None
    for _ in range(max_walk):
        raw = bedrock_get(node, f"/cryptarchia/blocks/{h}")
        b = json.loads(raw)
        if b is None or b["header"]["id"] != h:
            return None, ci, f"block {h} not returned as itself", walked
        walked.append((h, raw))
        ins = [op["payload"]["inscription"] for t in b["transactions"] for op in t["mantle_tx"]["ops"][:1]
               if op["opcode"] == 17 and op["payload"]["channel_id"] == LEZ_CHANNEL]
        if ins and newest is None:
            newest = int.from_bytes(bytes.fromhex(ins[0][:16]), "little")
        if any(same_lez_block(i, lez_raw) for i in ins):
            if evidence:
                d = os.path.join(evidence, f"{label}-top{ci['tip'][:16]}")
                os.makedirs(d, exist_ok=True)
                for hid, r in walked:
                    open(os.path.join(d, f"{hid}.json"), "wb").write(r)
                json.dump({"info": info, "top": ci["tip"], "inscription_block": h, "chain_top_down": [x[0] for x in walked]},
                          open(os.path.join(d, "walk.json"), "w"), indent=1)
                json.dump([x[0] for x in reversed(walked)], open(os.path.join(d, "chain.json"), "w"), indent=0)
            return {"block": h, "slot": b["header"]["slot"], "depth": len(walked) - 1}, ci, None, walked
        if b["header"]["slot"] == 0:
            break
        h = b["header"]["parent_block"]
    return None, ci, f"not inscribed within {len(walked)} blocks below the tip (newest LEZ block seen inscribed: {newest})", walked


def load_keys(run):
    members = [Account.from_key(open(os.environ["SEPOLIA_KEY_FILE"]).read().strip())]
    for m in json.load(open(os.path.join(run, "keys", "committee.json"))):
        members.append(Account.from_key(m["key"]))
    return members


def attestation_digest(verifier, vault, pairing, root, lez_block, final):
    local = keccak(abi_encode(["bytes32", "uint256", "address", "bytes32", "bytes32", "uint256", "uint256"],
                              [ATTEST_TAG, CHAIN_ID, vault, bytes.fromhex(pairing[2:]), bytes.fromhex(root[2:]), lez_block, final]))
    onchain = ecall(verifier, "attestationDigest(address,bytes32,bytes32,uint64,uint64)(bytes32)", vault, pairing, root, lez_block, final)
    if onchain.lower() != "0x" + local.hex():
        sys.exit(f"attestationDigest: the verifier says {onchain}, recomputed 0x{local.hex()}")
    return local


def send_release(relayer, vault, data, gas_limit, lockfile):
    """Sign locally and send, one relayer transaction at a time across processes; returns (hash, receipt)."""
    import fcntl
    with open(lockfile, "w") as lk:
        fcntl.flock(lk, fcntl.LOCK_EX)
        return _send_release(relayer, vault, data, gas_limit)


def _send_release(relayer, vault, data, gas_limit):
    nonce = int(ok(ETH_RPC, "eth_getTransactionCount", [relayer.address, "pending"]), 16)
    base = int(ok(ETH_RPC, "eth_getBlockByNumber", ["latest", False])["baseFeePerGas"], 16)
    tip = max(int(ok(ETH_RPC, "eth_maxPriorityFeePerGas", []), 16), 1_000_000)
    tx = {"chainId": CHAIN_ID, "nonce": nonce, "to": vault, "value": 0, "data": data, "gas": gas_limit,
          "maxFeePerGas": 2 * base + tip, "maxPriorityFeePerGas": tip, "type": 2}
    signed = relayer.sign_transaction(tx)
    raw = "0x" + (signed.raw_transaction if hasattr(signed, "raw_transaction") else signed.rawTransaction).hex().removeprefix("0x")
    h = ok(ETH_RPC, "eth_sendRawTransaction", [raw])
    for _ in range(180):
        rc = rpc(ETH_RPC, "eth_getTransactionReceipt", [h]).get("result")
        if rc:
            # wait one more block so that the next attempt sees this one's state
            while int(ok(ETH_RPC, "eth_blockNumber", []), 16) <= int(rc["blockNumber"], 16):
                time.sleep(3)
            return h, rc
        time.sleep(4)
    sys.exit(f"release {h}: no receipt after 12 minutes")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lez", required=True)
    ap.add_argument("--run", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--proof")
    ap.add_argument("--owner")
    ap.add_argument("--bedrock-node")
    ap.add_argument("--bedrock-evidence")
    ap.add_argument("--refusals", action="store_true")
    ap.add_argument("--final", action="store_true")
    ap.add_argument("--gas-limit", type=int, default=250_000)
    a = ap.parse_args()
    dep = json.load(open(os.path.join(a.run, "state", "anvil", "deployment.json")))
    vault, verifier = dep["vault"], dep["verifier"]
    class Appender:   # reopened on every write, so the manifest may be rewritten in place while a release waits
        def write(self, text):
            with open(a.out, "a") as f:
                f.write(text)
        def flush(self):
            pass
    out = Appender()
    if a.final:
        return final(a, dep, out)

    pr = json.load(open(a.proof))
    members = load_keys(a.run)
    relayer = members[0]
    onchain_members = [x.lower() for x in cast("call", verifier, "members()(address[])", "--rpc-url", ETH_RPC).strip("[]").split(", ")]
    if [m.address.lower() for m in members] != onchain_members:
        sys.exit("the keys are not the verifier's committee, in order")
    depth = int(ecall(verifier, "FINALITY_DEPTH()(uint64)"))
    pairing = ecall(vault, "PAIRING()(bytes32)")
    token, amount, recipient, fee, secret, index, path = pr["token"], pr["amount"], pr["recipient"], pr["fee"], pr["secret"], pr["index"], pr["path"]
    assert recipient.lower() == dep[a.owner].lower(), "the burn's recipient is not the run's recipient"

    # The root and nullifier the opening leads to, as the verifier computes them.
    ec = ecall(verifier, "ethCommitment(address,uint256,bytes32)(bytes32)", recipient, fee, secret)
    leaf = ecall(verifier, "burnLeaf(bytes32,address,uint256,bytes32)(bytes32)", pairing, token, amount, ec)
    root = ecall(verifier, "rootFromPath(bytes32,uint64,bytes32[24])(bytes32)", leaf, index, "[" + ",".join(path) + "]")
    nullifier = ecall(verifier, "burnNullifier(bytes32,bytes32,uint64)(bytes32)", pairing, secret, index)
    if (leaf, root, nullifier) != (pr["leaf"], pr["root"], pr["nullifier"]):
        sys.exit("the verifier's leaf, root or nullifier differ from the client's")
    burn_raw = None
    res = ok(a.lez, "getTransaction", [pr["burn_tx"]])
    if isinstance(res, list):
        import base64
        burn_raw = base64.b64decode(res[0])
    if not any(l.startswith(f"# release-proof {a.owner} ") for l in open(a.out).read().splitlines()):
      out.write(f"# release-proof {a.owner} burn_tx {pr['burn_tx']} burn_block {pr['burn_block']} private {str(pr['private']).lower()} "
              f"token {token} amount {amount} recipient {recipient} fee {fee} index {index} leaf {leaf} eth_commitment {ec} "
              f"nullifier {nullifier} root {root} commitment_in_burn_tx {str(bool(burn_raw) and bytes.fromhex(ec[2:]) in burn_raw).lower()}\n")
    out.flush()

    def read(i):
        """Member i's own read of the burn tree: (root, leaves, the LEZ block it is the state at)."""
        root_seen, leaves_seen, at = lez_burn_root(a.lez, pr["burns_account"], pr["bridge_program"])
        print(f"  member {i}: burn tree root {root_seen[:18]}… ({leaves_seen} leaves) at LEZ block {at}", file=sys.stderr)
        return root_seen, leaves_seen, at

    def bedrock_check(i, final_):
        """Member i's check that LEZ block `final_` is inscribed on its Bedrock node's canonical chain
        with at least BEDROCK_DEPTH (k = 30, the node's security parameter) blocks above it."""
        if not a.bedrock_node:
            return "unchecked(no-node)"
        k = int(os.environ.get("BEDROCK_DEPTH", "30"))
        raw = lez_block_bytes(a.lez, final_)
        while True:
            found, ci, why, _ = bedrock_inscribed(a.bedrock_node, raw, None, "")
            if found and found["depth"] >= k:
                found, ci, why, _ = bedrock_inscribed(a.bedrock_node, raw, a.bedrock_evidence, f"lez{final_}")
            if found and found["depth"] >= k:
                note = (f"lez{final_}@{found['block']}:slot{found['slot']}:depth{found['depth']}:top{ci['tip'][:16]}:"
                        f"top_slot{ci['slot']}:lib_slot{ci['lib_slot']}")
                tools = os.environ.get("BEDROCK_TOOLS")
                if tools and a.bedrock_evidence:
                    # The node's own types re-derive every saved block, and lez-bedrock checks the
                    # inscription's Merkle path, its Ed25519 signature and the headers above it.
                    d = os.path.join(a.bedrock_evidence, f"lez{final_}-top{ci['tip'][:16]}")
                    r = subprocess.run([tools, "check", d], capture_output=True, text=True)
                    if r.returncode != 0:
                        sys.exit(f"member {i}: bedrock-tools check refuses the saved blocks: {r.stderr.strip()[-300:]}")
                    blk = json.load(open(os.path.join(d, f"{found['block']}.json")))
                    signer = next(op["payload"]["signer"] for t in blk["transactions"] for op in t["mantle_tx"]["ops"][:1]
                                  if op["opcode"] == 17 and op["payload"]["channel_id"] == LEZ_CHANNEL and same_lez_block(op["payload"]["inscription"], raw))
                    r = subprocess.run([tools, "anchor", d, "--channel", LEZ_CHANNEL, "--signer", signer, "--lez-hash", raw[40:72].hex(),
                                        "--depth", str(found["depth"]), "--out", os.path.join(d, "anchor.json")], capture_output=True, text=True)
                    if r.returncode != 0:
                        sys.exit(f"member {i}: bedrock-tools anchor refuses: {r.stderr.strip()[-300:]}")
                    note += f":signer{signer[:16]}:anchor-ok"
                return note
            print(f"  member {i}: LEZ block {final_} not yet {k} blocks deep on Bedrock "
                  f"({why or 'inscribed ' + str(found['depth']) + ' deep'}); waiting", file=sys.stderr)
            time.sleep(120)

    def attempt(label, expect, signers, lez_block, final_, notes_of, gas_limit=a.gas_limit):
        if any(l.startswith(f"# eth {label} ") for l in open(a.out).read().splitlines()):
            print(json.dumps({"label": label, "skipped": "already in the manifest"}))   # a resumed run
            return True
        d = attestation_digest(verifier, vault, pairing, root, lez_block, final_)
        sigs = ["0x" + members[i].sign_message(encode_defunct(primitive=d)).signature.hex().removeprefix("0x") for i in signers]
        notes = [f"m{i}:{members[i].address}:{notes_of.get(i, '-')}" for i in signers]
        release = f"({token},{amount},{recipient},{relayer.address},{fee},{secret},{index},[{','.join(path)}],{lez_block},{final_},[{','.join(sigs)}])"
        proof = cast("abi-encode", f"f({RELEASE_T})", release)
        data = cast("calldata", "release(address,uint256,address,address,uint256,bytes32,bytes32,bytes)",
                    token, amount, recipient, relayer.address, fee, root, nullifier, proof)
        if expect == "applied":
            est = rpc(ETH_RPC, "eth_estimateGas", [{"from": relayer.address, "to": vault, "data": data}, "latest"])
            if "result" in est:
                gas_limit = int(int(est["result"], 16) * 1.25)
        call = {"from": relayer.address, "to": vault, "data": data, "gas": hex(gas_limit)}
        sim = rpc(ETH_RPC, "eth_call", [call, "latest"])
        selectors = {cast("sig", e): e.split("(")[0] for e in ERRORS}
        if "error" in sim:
            err = sim["error"].get("data") or ""
            if isinstance(err, dict):
                err = err.get("data") or ""
            pre = selectors.get(err[:10], f"unknown:{err[:10] or sim['error'].get('message')}")
        else:
            pre = "applied"
        if pre != expect:
            print(json.dumps({"label": label, "expect": expect, "precheck": pre, "sent": False}))
            out.write(f"# eth-precheck {label} expected {expect} got {pre}: not sent\n")
            out.flush()
            return False
        if os.environ.get("RELEASE_DRY"):
            print(json.dumps({"label": label, "expect": expect, "precheck": pre, "sent": False, "dry": True}))
            return True
        h, rc = send_release(relayer, vault, data, gas_limit, os.path.join(a.run, "relayer.lock"))
        status = int(rc["status"], 16)
        verdict = "applied" if status == 1 else "reverted"
        sel = "-" if status == 1 else next((k for k, v in selectors.items() if v == pre), "-")
        out.write(f"# eth {label} {expect} {verdict} {int(rc['blockNumber'], 16)} {h} {sel} {int(rc['gasUsed'], 16)} {nullifier} {root}\n")
        out.write(f"# eth-attestation {label} lez_block {lez_block} lez_final_block {final_} signers {len(sigs)} {' '.join(notes)} "
                  f"relayer {relayer.address} gas_limit {gas_limit}\n")
        out.flush()
        good = (expect == "applied" and verdict == "applied") or (expect != "applied" and verdict == "reverted")
        print(json.dumps({"label": label, "expect": expect, "precheck": pre, "verdict": verdict, "tx": h, "block": int(rc["blockNumber"], 16),
                          "gas_used": int(rc["gasUsed"], 16), "lez_block": lez_block, "lez_final_block": final_, "root": root,
                          "nullifier": nullifier, "ok": good}))
        return good

    good = True
    # Member 0 proposes: the root it read and the block it read it at.
    root0, leaves0, blk = read(0)
    if root0 != root:
        sys.exit(f"member 0 read root {root0}, the opening leads to {root}: no member signs this release")
    if a.refusals:
        # Signed at depth 0 (the block read is named final): the verifier refuses it.
        good &= attempt(f"release-{a.owner}-not-final", "NotFinal", [0, 1], blk, blk, {0: f"read{blk}:depth0", 1: f"read{blk}:depth0"})
    if os.environ.get("RELEASE_DRY"):
        blk -= depth   # dry run: pretend the read is `depth` blocks old, to exercise the rest without waiting
    while lez_tip(a.lez) < blk + depth:
        print(f"  LEZ tip {lez_tip(a.lez)}, waiting for {blk + depth} ({depth} blocks above {blk})", file=sys.stderr)
        time.sleep(60)
    fin = lez_tip(a.lez)
    # Every member checks the proposal itself: the root it reads now is the proposed one (the tree is
    # append-only, so an unchanged root and leaf count mean no leaf was added since `blk`), the tip
    # it sees is at least the final block, and (with a node) the final block is under the Bedrock LIB.
    notes = {}
    for i in range(len(members)):
        r_i, n_i, at_i = read(i)
        if (r_i, n_i) != (root0, leaves0) or at_i < fin:
            sys.exit(f"member {i} reads root {r_i} ({n_i} leaves) at {at_i}: not the proposal ({root0}, {leaves0}, final {fin}); it does not sign")
        notes[i] = f"read{at_i}:root{r_i}:leaves{n_i}:bedrock={bedrock_check(i, fin)}"
    if a.refusals:
        good &= attempt(f"release-{a.owner}-one-member", "RootNotAttested", [0], blk, fin, notes)
    good &= attempt(f"release-{a.owner}", "applied", [0, 2], blk, fin, notes)
    if a.refusals:
        good &= attempt(f"release-{a.owner}-replay", "AlreadyReleased", [0, 2], blk, fin, notes)
    sys.exit(0 if good else 1)


def final(a, dep, out):
    vault = dep["vault"]
    lines = open(a.out).read().splitlines()
    rp = [l.split() for l in lines if l.startswith("# release-proof ")]
    if not rp:
        sys.exit("no release-proof lines")
    burns = next(l.split()[2] for l in lines if l.startswith("# burns-account "))
    prog = next(l.split()[2] for l in lines if l.startswith("# program ") and l.split()[4] == "wrap_bridge")
    roots = {p[p.index("root") + 1] for p in rp}
    root_now, leaves_now, tip = lez_burn_root(a.lez, burns, prog)
    out.write(f"# release-root {root_now} {burns} {prog}\n")
    for p in rp:
        nf = p[p.index("nullifier") + 1]
        got = int(ecall(vault, "nullifierSpent(bytes32)(bool)", nf) == "true")
        out.write(f"# eth-final nullifier {nf} {'true' if got else 'false'} {p[2]}\n")
        out.write(f"# eth-final balance {p[p.index('recipient') + 1]} {int(ok(ETH_RPC, 'eth_getBalance', [p[p.index('recipient') + 1], 'latest']), 16)} {p[2]}-recipient\n")
    ctl = "0x" + secrets.token_hex(32)
    out.write(f"# eth-final unspent {ctl} {'true' if ecall(vault, 'nullifierSpent(bytes32)(bool)', ctl) == 'true' else 'false'} control\n")
    out.write(f"# eth-final balance {vault} {int(ok(ETH_RPC, 'eth_getBalance', [vault, 'latest']), 16)} vault\n")
    out.write(f"# lez-final tip {tip} burn_tree_leaves {leaves_now}\n")
    # The LEZ accounts the burns wrote, as they are now (scripts/verify-onchain.sh checks `final` lines).
    ids = json.loads(subprocess.run([os.environ.get("CLI", "cli/target/release/wrap"), "ids"], capture_output=True, text=True, check=True).stdout)
    progs = json.load(open(os.path.join(a.run, "state", "programs.json")))
    for name, p_ in [("burns", progs["wrap_bridge"]), ("eth_entry", progs["wrap_bridge"]), ("eth_definition", progs["token"]), ("alice_eth_ata", progs["token"])]:
        res = ok(a.lez, "getAccount", [ids[name]]) or {}
        b = bytes(((res.get("data") or {}).get("shards") or {}).get(p_, []))
        out.write(f"# final {ids[name]} {p_} {len(b)} {hashlib.sha256(b).hexdigest()} {name}\n")
    out.flush()
    print(json.dumps({"release_root": root_now, "leaves": leaves_now, "roots_released_under": sorted(roots)}))


if __name__ == "__main__":
    main()

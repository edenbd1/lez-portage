#!/usr/bin/env python3
"""The outbound half of scripts/e2e-local.sh on anvil: attest a LEZ burn-tree
root with the release committee, and release (or attempt to) from WrapVault.

    python3 scripts/anvil_release.py --anvil URL --lez URL --deployment deployment.json \
        --proof proof.json --label L --expect applied|<error> --out MANIFEST [options]

`proof.json` is `wrap release-proof <owner>`: the burn's opening (token,
amount, recipient, fee, secret, index, path), the root it leads to, its
nullifier, and the LEZ burn-tree account.

**The committee** (CommitteeBurnVerifier, 2 of anvil accounts 5, 6, 7). Each
member, independently, reads the burn tree's root from the LEZ sequencer
(getAccount of the burn-tree account, read between two equal reads of the
tip, so the root is the state at that block), and signs, with anvil's
eth_sign (EIP-191), the verifier's `attestationDigest` of the root *it read*,
not the one the release names: a member never signs a root it did not see.
With `--wait-final` it first waits until the LEZ tip is `finality_depth`
blocks above the block it read, and names the tip as final; without it, it
names the block it read as the final one, which the verifier refuses
(`NotFinal`). On the local standalone sequencer there is no Bedrock: "final"
is the sequencer's tip, a stand-in stated in the manifest.

**The release** is sent by `--sender` (the recipient itself, or a relayer,
who earns the fee the burn committed to), with an explicit gas limit, so a
refused one is mined and recorded too: its revert reason is read first with
eth_call and is the verdict the run expects. Tampering for the refusals:
`--recipient-as-sender` (redirect the release to the sender), `--amount`
(another amount), `--forge` (a burn that never happened: a made-up leaf paying
the sender, in a made-up tree, its root signed by `--outsiders` instead of the
committee).

One line per attempt is appended to the manifest:
    # eth <label> <expected> <verdict> <block> <tx> <error-selector|-> <gas_used> <nullifier> <root>
Uses anvil's unlocked development accounts only; the chain is local.
"""
import argparse
import json
import subprocess
import sys
import time
import urllib.request

ERRORS = ["NotThisRelease()", "BadOpening()", "NotFinal()", "RootNotAttested()", "AlreadyReleased()", "VerificationFailed()",
          "IsFrozen()", "InvalidAmount()", "UnsupportedToken()", "CapExceeded()", "TransferFailed()"]
RELEASE_T = "(address,uint256,address,address,uint256,bytes32,uint64,bytes32[24],uint64,uint64,bytes[])"


def rpc(url, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(url, body, {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


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


def lez_tip(lez):
    return int(ok(lez, "getLastBlockId", []))


def lez_burn_root(lez, burns, program):
    """The burn tree's root at the tip, and that tip (two equal reads around the account)."""
    for _ in range(20):
        t0 = lez_tip(lez)
        res = ok(lez, "getAccount", [burns]) or {}
        data = bytes(((res.get("data") or {}).get("shards") or {}).get(program, []))
        if lez_tip(lez) == t0:
            # borsh Frontier { next_index: u64, filled: Vec<[u8; 32]>, root: [u8; 32] }
            if len(data) != 8 + 4 + 24 * 32 + 32:
                sys.exit(f"the burn tree account holds {len(data)} bytes, not a frontier")
            return "0x" + data[-32:].hex(), int.from_bytes(data[:8], "little"), t0
        time.sleep(0.5)
    sys.exit("the LEZ tip kept moving while reading the burn tree")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--anvil", required=True)
    ap.add_argument("--lez", required=True)
    ap.add_argument("--deployment", required=True)
    ap.add_argument("--proof", required=True)
    ap.add_argument("--label", required=True)
    ap.add_argument("--expect", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--sender", default="recipient", help="recipient | relayer | outsider")
    ap.add_argument("--members", default="0,2", help="which committee members sign (indices)")
    ap.add_argument("--outsiders", action="store_true", help="the two outsiders sign instead")
    ap.add_argument("--wait-final", action="store_true")
    ap.add_argument("--recipient-as-sender", action="store_true")
    ap.add_argument("--amount")
    ap.add_argument("--forge", action="store_true")
    ap.add_argument("--gas-limit", default="600000")
    a = ap.parse_args()
    dep = json.load(open(a.deployment))
    pr = json.load(open(a.proof))
    vault, verifier = dep["vault"], dep["verifier"]
    depth = int(dep["finality_depth"])
    recipient = pr["recipient"]
    sender = {"recipient": recipient, "relayer": dep["relayer"], "outsider": dep["outsiders"][0]}[a.sender]

    # The committee's view of LEZ.
    root_seen, leaves_seen, lez_block = lez_burn_root(a.lez, pr["burns_account"], pr["bridge_program"])
    final = lez_block
    if a.wait_final:
        while lez_tip(a.lez) < lez_block + depth:
            time.sleep(1)
        final = lez_tip(a.lez)

    token, amount, fee, secret, index, path = pr["token"], pr["amount"], pr["fee"], pr["secret"], pr["index"], pr["path"]
    if a.recipient_as_sender:
        recipient = sender
    if a.amount:
        amount = a.amount
    signed_root = root_seen
    if a.forge:
        # A burn that never happened: a leaf paying the sender, alone in a tree.
        secret = "0x" + "66" * 32
        fee = "0"
        recipient = sender
        ec = cast("call", verifier, "ethCommitment(address,uint256,bytes32)(bytes32)", recipient, fee, secret, "--rpc-url", a.anvil)
        pairing = cast("call", vault, "PAIRING()(bytes32)", "--rpc-url", a.anvil)
        leaf = cast("call", verifier, "burnLeaf(bytes32,address,uint256,bytes32)(bytes32)", pairing, token, amount, ec, "--rpc-url", a.anvil)
        zero = "0x" + "00" * 32
        path, z = [], zero
        for _ in range(24):
            path.append(z)
            z = "0x" + __import__("hashlib").sha256(bytes.fromhex(z[2:]) * 2).hexdigest()
        index = 0
        signed_root = cast("call", verifier, f"rootFromPath(bytes32,uint64,bytes32[24])(bytes32)", leaf, "0", "[" + ",".join(path) + "]", "--rpc-url", a.anvil)

    # The root and nullifier the opening leads to, as the verifier computes them.
    pairing = cast("call", vault, "PAIRING()(bytes32)", "--rpc-url", a.anvil)
    ec = cast("call", verifier, "ethCommitment(address,uint256,bytes32)(bytes32)", recipient, fee, secret, "--rpc-url", a.anvil)
    leaf = cast("call", verifier, "burnLeaf(bytes32,address,uint256,bytes32)(bytes32)", pairing, token, amount, ec, "--rpc-url", a.anvil)
    root = cast("call", verifier, "rootFromPath(bytes32,uint64,bytes32[24])(bytes32)", leaf, str(index), "[" + ",".join(path) + "]", "--rpc-url", a.anvil)
    nullifier = cast("call", verifier, "burnNullifier(bytes32,bytes32,uint64)(bytes32)", pairing, secret, str(index), "--rpc-url", a.anvil)

    # The signatures: committee members sign what they saw; outsiders sign what they are told.
    signers = dep["outsiders"] if a.outsiders else [dep["committee"][int(i)] for i in a.members.split(",") if i != ""]
    sigs = []
    for s in signers:
        r = signed_root if (a.outsiders or a.forge and s in dep["outsiders"]) else root_seen
        d = cast("call", verifier, "attestationDigest(address,bytes32,bytes32,uint64,uint64)(bytes32)", vault, pairing, r, str(lez_block), str(final), "--rpc-url", a.anvil)
        sigs.append(ok(a.anvil, "eth_sign", [s, d]))

    release = f"({token},{amount},{recipient},{sender},{fee},{secret},{index},[{','.join(path)}],{lez_block},{final},[{','.join(sigs)}])"
    proof = cast("abi-encode", f"f({RELEASE_T})", release)
    data = cast("calldata", "release(address,uint256,address,address,uint256,bytes32,bytes32,bytes)",
                token, amount, recipient, sender, fee, root, nullifier, proof)
    call = {"from": sender, "to": vault, "data": data, "gas": hex(int(a.gas_limit))}
    sim = rpc(a.anvil, "eth_call", [call, "latest"])
    selectors = {cast("sig", e): e[:-2] for e in ERRORS}
    if "error" in sim:
        err_data = (sim["error"].get("data") or "")[:10]
        pre = selectors.get(err_data, f"unknown:{err_data or sim['error'].get('message')}")
    else:
        pre = "applied"
    h = ok(a.anvil, "eth_sendTransaction", [call])
    rc = None
    for _ in range(60):
        rc = ok(a.anvil, "eth_getTransactionReceipt", [h])
        if rc:
            break
        time.sleep(0.5)
    status = int(rc["status"], 16)
    verdict = "applied" if status == 1 else "reverted"
    sel = "-" if status == 1 else next((k for k, v in selectors.items() if v == pre), "-")
    line = (f"# eth {a.label} {a.expect} {verdict} {int(rc['blockNumber'], 16)} {h} {sel} {int(rc['gasUsed'], 16)} {nullifier} {root}")
    with open(a.out, "a") as f:
        f.write(line + "\n")
        f.write(f"# eth-attestation {a.label} lez_block {lez_block} lez_final_block {final} root_seen {root_seen} leaves_seen {leaves_seen} "
                f"signers {','.join(signers)} sender {sender}\n")
    good = (a.expect == "applied" and verdict == "applied" and pre == "applied") or (a.expect != "applied" and verdict == "reverted" and pre == a.expect)
    print(json.dumps({"label": a.label, "expect": a.expect, "precheck": pre, "verdict": verdict, "tx": h, "block": int(rc["blockNumber"], 16),
                      "gas_used": int(rc["gasUsed"], 16), "lez_block": lez_block, "lez_final_block": final, "root_seen": root_seen,
                      "root": root, "nullifier": nullifier, "ok": good}))
    sys.exit(0 if good else 1)


if __name__ == "__main__":
    main()

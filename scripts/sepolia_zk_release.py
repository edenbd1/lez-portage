#!/usr/bin/env python3
"""A Bedrock-anchored zero-knowledge release on Sepolia: the vault deployed by
`sepolia_vault.py deploy-zk`, whose verifier is ZkBedrockBurnVerifier.

    sepolia_zk_release.py --run DIR --deployment deployment.json --proof PROOF.json \
        --bedrock-node URL --label L --expect applied|<error> --out MANIFEST [options]

`PROOF.json` is `wrap release-proof <owner> --zk --chain CHAIN.json --bedrock
ANCHOR.json --relayer <relayer> --vault <vault>`: an 800-byte journal (the
release preimage, the chain proof's image id, program, burn-tree account,
anchor and tip, the anchor proof's image id, channel, signer, Bedrock block,
top header and depth) and one Groth16 seal for the three guests.

**What each committee member checks before it signs** (three keys, 2 of 3;
one operator holds all three, which the manifest says): that the top Bedrock
header the journal names is on the canonical chain of *its own* Bedrock node
(the node has the block, and walking parent links back from its tip meets
it). It then signs, with its own key (EIP-191), the verifier's
`headerAttestationDigest(vault, pairing, header)` of the header its node has;
if its node does not have the journal's header, it signs its own tip instead,
which the verifier refuses. Nothing else is attested: the burn, the burn
tree, the LEZ blocks and the inscription are all inside the proof.

**The release** is sent by the relayer the proof names (the deployer key),
with an explicit gas limit after an eth_call pre-check, so a refusal is mined
too and its revert is the recorded verdict. Refusals: `--members 0` (one
member: HeaderNotAttested), `--sender outsider` (a front-run that names
itself as the relayer: the proof names another, NotThisRelease; the same
calldata resent by anyone else only pays the relayer the proof names),
`--tamper-journal` (the recipient changed in
the journal too: the seal proves nothing, VerificationFailed), and the replay
(AlreadyReleased).

Keys are read from files and never printed: $SEPOLIA_KEY_FILE (member 0 and
the relayer) and RUN/keys/committee.json (members 1 and 2); the outsider is a
key made here and thrown away.
"""
import argparse
import hashlib
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from eth_account import Account  # noqa: E402
from eth_account.messages import encode_defunct  # noqa: E402

import sepolia_release as sr  # noqa: E402
from anvil_release import bedrock_canonical  # noqa: E402

ERRORS = ["HeaderNotAttested()", "NotThisChannel()", "NotDeepEnough()", "SelectorMismatch(bytes4,bytes4)", "NotThisRelease()",
          "NotThisChain()", "AlreadyReleased()", "VerificationFailed()", "IsFrozen()", "InvalidAmount()", "UnsupportedToken()",
          "CapExceeded()", "TransferFailed()"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True)
    ap.add_argument("--deployment", required=True)
    ap.add_argument("--proof", required=True)
    ap.add_argument("--bedrock-node", required=True)
    ap.add_argument("--label", required=True)
    ap.add_argument("--expect", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--members", default="0,2")
    ap.add_argument("--sender", default="relayer", help="relayer | outsider")
    ap.add_argument("--tamper-journal", action="store_true")
    ap.add_argument("--gas-limit", type=int, default=900000)
    a = ap.parse_args()
    rpc = sr.ETH_RPC
    dep = json.load(open(a.deployment))
    pr = json.load(open(a.proof))
    z = pr["zk"]
    vault, verifier = dep["vault"], dep["verifier"]
    j = bytes.fromhex(z["journal"][2:])
    if len(j) != 800:
        sys.exit(f"a Bedrock-anchored journal is 800 bytes, this one {len(j)}")
    w = lambda k: j[32 * k:32 * (k + 1)]
    addr = lambda k: "0x" + w(k)[12:].hex()
    token, amount, recipient, relayer_j, fee = addr(4), str(int.from_bytes(w(5), "big")), addr(6), addr(7), str(int.from_bytes(w(8), "big"))
    root, nullifier, pairing = "0x" + w(9).hex(), "0x" + w(10).hex(), "0x" + w(3).hex()
    top = w(22).hex()

    members = sr.load_keys(a.run)
    relayer = members[0]
    if relayer.address.lower() != relayer_j.lower():
        sys.exit(f"the proof names relayer {relayer_j}, the key is {relayer.address}")
    sender = relayer
    if a.sender == "outsider":
        sender = Account.create()   # a front-runner, funded below with exactly the gas it may burn
    canonical, node_tip = bedrock_canonical(a.bedrock_node, top)
    seen = top if canonical else node_tip
    if a.tamper_journal:
        recipient = relayer.address
        j = j[:6 * 32] + bytes.fromhex(relayer.address[2:]).rjust(32, b"\0") + j[7 * 32:]
    sigs = []
    signers = [int(i) for i in a.members.split(",") if i != ""]
    d = sr.cast("call", verifier, "headerAttestationDigest(address,bytes32,bytes32)(bytes32)", vault, pairing, "0x" + seen, "--rpc-url", rpc)
    for i in signers:
        sigs.append("0x" + members[i].sign_message(encode_defunct(primitive=bytes.fromhex(d[2:]))).signature.hex().removeprefix("0x"))
    proof = sr.cast("abi-encode", "f((bytes,bytes,bytes[]))", f"({z['seal']},0x{j.hex()},[{','.join(sigs)}])")
    named = sender.address if a.sender == "outsider" else relayer_j
    data = sr.cast("calldata", "release(address,uint256,address,address,uint256,bytes32,bytes32,bytes)",
                   token, amount, recipient, named, fee, root, nullifier, proof)

    # What the calldata does not carry: the burn's opening.
    sec = bytes.fromhex(pr["secret"][2:])
    ec = hashlib.sha256(b"rfp022/eth-commit/v1".ljust(32, b"\0") + bytes.fromhex(pr["recipient"][2:]).rjust(32, b"\0")
                        + int(pr["fee"]).to_bytes(32, "big") + sec).hexdigest()
    zeros, zz = set(), bytes(32)
    for _ in range(25):
        zeros.add(zz.hex()); zz = hashlib.sha256(zz + zz).digest()
    nodes = [p[2:] for p in pr.get("chain_path", pr.get("path", [])) if p[2:] not in zeros]
    hidden = {"leaf": pr["leaf"][2:] not in data, "secret": sec.hex() not in data, "eth_commitment": ec not in data,
              "path_nodes": f"{sum(n not in data for n in nodes)}/{len(nodes)}"}

    selectors = {sr.cast("sig", e): e.split("(")[0] for e in ERRORS}
    call = {"from": sender.address, "to": vault, "data": data, "gas": hex(a.gas_limit)}
    sim = sr.rpc(rpc, "eth_call", [call, "latest"])
    if "error" in sim:
        err = (sim["error"].get("data") or "")[:10]
        pre = selectors.get(err, f"unknown:{err or sim['error'].get('message')}")
    else:
        pre = "applied"
    if pre != a.expect:
        sys.exit(f"{a.label}: pre-check {pre}, expected {a.expect}: not sent")
    if a.sender == "outsider":
        # The front-runner needs gas money: the relayer sends it exactly what the attempt can cost.
        base = int(sr.ok(rpc, "eth_getBlockByNumber", ["latest", False])["baseFeePerGas"], 16)
        tip = max(int(sr.ok(rpc, "eth_maxPriorityFeePerGas", []), 16), 1_000_000)
        fund = a.gas_limit * (2 * base + tip)
        n = int(sr.ok(rpc, "eth_getTransactionCount", [relayer.address, "pending"]), 16)
        tx = {"chainId": sr.CHAIN_ID, "nonce": n, "to": sender.address, "value": fund, "gas": 21000,
              "maxFeePerGas": 2 * base + tip, "maxPriorityFeePerGas": tip, "type": 2}
        s = relayer.sign_transaction(tx)
        h0 = sr.ok(rpc, "eth_sendRawTransaction", ["0x" + (s.raw_transaction if hasattr(s, "raw_transaction") else s.rawTransaction).hex().removeprefix("0x")])
        for _ in range(120):
            if sr.rpc(rpc, "eth_getTransactionReceipt", [h0]).get("result"):
                break
            sr.time.sleep(4)
    h, rc = sr.send_release(sender, vault, data, a.gas_limit, os.path.join(a.run, "relayer.lock"))
    status = int(rc["status"], 16)
    verdict = "applied" if status == 1 else "reverted"
    sel = "-" if status == 1 else next((k for k, v in selectors.items() if v == pre), "-")
    with open(a.out, "a") as f:
        f.write(f"# eth {a.label} {a.expect} {verdict} {int(rc['blockNumber'], 16)} {h} {sel} {int(rc['gasUsed'], 16)} {nullifier} {root}\n")
        f.write(f"# eth-header-attestation {a.label} top_journal {top} on_members_node {canonical} signed {seen} node_tip {node_tip} "
                f"signers {','.join(members[i].address for i in signers)} sender {sender.address}\n")
        f.write(f"# eth-zk {a.label} {h} hides leaf {hidden['leaf']} secret {hidden['secret']} eth_commitment {hidden['eth_commitment']} "
                f"path_nodes {hidden['path_nodes']} calldata_bytes {(len(data) - 2) // 2} seal_bytes {(len(z['seal']) - 2) // 2}\n")
    good = (a.expect == "applied" and verdict == "applied") or (a.expect != "applied" and verdict == "reverted")
    if not (hidden["leaf"] and hidden["secret"] and hidden["eth_commitment"] and hidden["path_nodes"].split("/")[0] == hidden["path_nodes"].split("/")[1]):
        good = False
    print(json.dumps({"label": a.label, "expect": a.expect, "precheck": pre, "verdict": verdict, "tx": h, "block": int(rc["blockNumber"], 16),
                      "gas_used": int(rc["gasUsed"], 16), "top": top, "on_members_node": canonical, "root": root, "nullifier": nullifier,
                      "hidden": hidden, "ok": good}))
    sys.exit(0 if good else 1)


if __name__ == "__main__":
    main()

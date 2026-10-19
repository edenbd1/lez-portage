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

**Zero-knowledge releases** (`--zk`, the vault's verifier is ZkBurnVerifier):
`proof.json` is `wrap release-proof <owner> --zk --relayer <sender>`, whose
`zk` part is the guest's journal (the release preimage: token, amount,
recipient, relayer, fee, root, nullifier, with the chain, the vault and the
pairing) and the Groth16 seal. The release's arguments are read from the
journal, the proof sent is `(seal, journal, lezBlock, lezFinalBlock,
signatures)`: the opening (secret, index, path, leaf) is not sent, which is
checked on the calldata and recorded (`# eth-zk`). Tampering: as above
(`--recipient-as-sender`, `--amount`; a `--sender` other than the relayer
the proof names is a front-run), plus `--tamper-journal` (the recipient
changed in the journal too, so that the vault's digest matches it: the seal
then proves nothing), and `--forged PROOF` (a real proof of a burn that never
happened: a made-up leaf in a made-up tree, proven by the same guest).

**Chain-derived releases** (`--zkchain`, the vault's verifier is
ZkChainBurnVerifier): `proof.json` is `wrap release-proof <owner> --zk
--chain CHAIN.json --relayer <sender>`, whose journal is the release
preimage followed by the chain receipt's image id, the program, the burn-tree
account, the anchor and the LEZ tip the root is as of. The committee attests a
LEZ *block*: each member reads the block at the journal's tip height from the
sequencer (getBlock) and signs `blockAttestationDigest` of the hash *it read*.
Tampering: as for `--zk`, plus `--tamper-anchor` (the journal names another
anchor), `--membership-proof PROOF` (a release of the earlier guest, whose
journal is the bare preimage), and `--forged PROOF` (a real proof over a
made-up LEZ block: the members sign the real block at that height, outsiders
sign the made-up one).

One line per attempt is appended to the manifest:
    # eth <label> <expected> <verdict> <block> <tx> <error-selector|-> <gas_used> <nullifier> <root>
Uses anvil's unlocked development accounts only; the chain is local.
"""
import argparse
import hashlib
import json
import subprocess
import sys
import time
import urllib.request

ERRORS = ["SelectorMismatch(bytes4,bytes4)", "NotThisRelease()", "NotThisChain()", "BlockNotAttested()", "BadOpening()", "NotFinal()", "RootNotAttested()", "AlreadyReleased()", "VerificationFailed()",
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
    ap.add_argument("--zk", action="store_true", help="a zero-knowledge release (ZkBurnVerifier)")
    ap.add_argument("--tamper-journal", action="store_true")
    ap.add_argument("--forged", help="a zk proof of a burn that never happened (wrap zk-prove output)")
    ap.add_argument("--zkchain", action="store_true", help="a chain-derived release (ZkChainBurnVerifier)")
    ap.add_argument("--tamper-anchor", action="store_true")
    ap.add_argument("--membership-proof", help="a burn_membership release proof (wrap release-proof --zk) sent to the chain verifier")
    a = ap.parse_args()
    if a.zkchain:
        return zkchain_release(a)
    if a.zk:
        return zk_release(a)
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
    selectors = {cast("sig", e): e.split("(")[0] for e in ERRORS}
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


def zk_release(a):
    dep = json.load(open(a.deployment))
    pr = json.load(open(a.proof))
    vault, depth = dep["vault"], int(dep["finality_depth"])
    z = json.load(open(a.forged)) if a.forged else pr["zk"]
    j = bytes.fromhex(z["journal"][2:])
    w = lambda k: j[32 * k:32 * (k + 1)]
    addr = lambda k: "0x" + w(k)[12:].hex()
    token, amount, recipient, relayer, fee = addr(4), str(int.from_bytes(w(5), "big")), addr(6), addr(7), str(int.from_bytes(w(8), "big"))
    root, nullifier = "0x" + w(9).hex(), "0x" + w(10).hex()
    sender = {"recipient": recipient, "relayer": dep["relayer"], "outsider": dep["outsiders"][0]}[a.sender]

    root_seen, leaves_seen, lez_block = lez_burn_root(a.lez, pr["burns_account"], pr["bridge_program"])
    final = lez_block
    if a.wait_final:
        while lez_tip(a.lez) < lez_block + depth:
            time.sleep(1)
        final = lez_tip(a.lez)

    # The release names the sender as its relayer (who submits earns the fee).
    relayer = sender
    if a.recipient_as_sender:
        recipient = sender
    if a.amount:
        amount = a.amount
    if a.tamper_journal:
        recipient = sender
        j = j[:6 * 32] + bytes.fromhex(sender[2:]).rjust(32, b"\0") + j[7 * 32:]

    pairing = "0x" + w(3).hex()
    signers = dep["outsiders"] if a.outsiders else [dep["committee"][int(i)] for i in a.members.split(",") if i != ""]
    sigs = []
    for s_ in signers:
        r = root if a.outsiders else root_seen  # members sign what they read; outsiders what they are told
        d = cast("call", dep["verifier"], "attestationDigest(address,bytes32,bytes32,uint64,uint64)(bytes32)", vault, pairing, r,
                 str(lez_block), str(final), "--rpc-url", a.anvil)
        sigs.append(ok(a.anvil, "eth_sign", [s_, d]))
    proof = cast("abi-encode", "f((bytes,bytes,uint64,uint64,bytes[]))", f"({z['seal']},0x{j.hex()},{lez_block},{final},[{','.join(sigs)}])")
    data = cast("calldata", "release(address,uint256,address,address,uint256,bytes32,bytes32,bytes)",
                token, amount, recipient, relayer, fee, root, nullifier, proof)

    # The opening stays with the prover: none of it is in what is sent.
    hidden = {}
    if not a.forged:
        sec = bytes.fromhex(pr["secret"][2:])
        ec = hashlib.sha256(b"rfp022/eth-commit/v1".ljust(32, b"\0") + bytes.fromhex(pr["recipient"][2:]).rjust(32, b"\0")
                            + int(pr["fee"]).to_bytes(32, "big") + sec).hexdigest()
        zeros, zz = set(), bytes(32)
        for _ in range(25):
            zeros.add(zz.hex()); zz = hashlib.sha256(zz + zz).digest()
        nodes = [p[2:] for p in pr["path"] if p[2:] not in zeros]
        hidden = {"leaf": pr["leaf"][2:] not in data, "secret": sec.hex() not in data, "eth_commitment": ec not in data,
                  "path_nodes": f"{sum(n not in data for n in nodes)}/{len(nodes)}"}

    call = {"from": sender, "to": vault, "data": data, "gas": hex(int(a.gas_limit))}
    sim = rpc(a.anvil, "eth_call", [call, "latest"])
    selectors = {cast("sig", e): e.split("(")[0] for e in ERRORS}
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
    with open(a.out, "a") as f:
        f.write(f"# eth {a.label} {a.expect} {verdict} {int(rc['blockNumber'], 16)} {h} {sel} {int(rc['gasUsed'], 16)} {nullifier} {root}\n")
        f.write(f"# eth-attestation {a.label} lez_block {lez_block} lez_final_block {final} root_seen {root_seen} leaves_seen {leaves_seen} "
                f"signers {','.join(signers)} sender {sender}\n")
        if hidden:
            f.write(f"# eth-zk {a.label} {h} hides leaf {hidden['leaf']} secret {hidden['secret']} eth_commitment {hidden['eth_commitment']} "
                    f"path_nodes {hidden['path_nodes']} calldata_bytes {(len(data) - 2) // 2} seal_bytes {(len(z['seal']) - 2) // 2}\n")
    good = (a.expect == "applied" and verdict == "applied" and pre == "applied") or (a.expect != "applied" and verdict == "reverted" and pre == a.expect)
    if hidden and not (hidden["leaf"] and hidden["secret"] and hidden["eth_commitment"] and hidden["path_nodes"].split("/")[0] == hidden["path_nodes"].split("/")[1]):
        good = False
    print(json.dumps({"label": a.label, "expect": a.expect, "precheck": pre, "verdict": verdict, "tx": h, "block": int(rc["blockNumber"], 16),
                      "gas_used": int(rc["gasUsed"], 16), "lez_block": lez_block, "lez_final_block": final, "root_seen": root_seen,
                      "root": root, "nullifier": nullifier, "hidden": hidden, "ok": good}))
    sys.exit(0 if good else 1)


def lez_block_hash(lez, block_id):
    """The hash of LEZ block `block_id` as the sequencer returns it (borsh header: id, prev, hash, timestamp)."""
    import base64
    res = ok(lez, "getBlock", [block_id])
    if not res:
        sys.exit(f"no LEZ block {block_id}")
    raw = base64.b64decode(res)
    return "0x" + raw[40:72].hex()


def zkchain_release(a):
    dep = json.load(open(a.deployment))
    pr = json.load(open(a.proof))
    vault, verifier, depth = dep["vault"], dep["verifier"], int(dep["finality_depth"])
    z = json.load(open(a.forged)) if a.forged else (json.load(open(a.membership_proof))["zk"] if a.membership_proof else pr["zk"])
    j = bytes.fromhex(z["journal"][2:])
    w = lambda k: j[32 * k:32 * (k + 1)]
    addr = lambda k: "0x" + w(k)[12:].hex()
    token, amount, recipient, relayer, fee = addr(4), str(int.from_bytes(w(5), "big")), addr(6), addr(7), str(int.from_bytes(w(8), "big"))
    root, nullifier = "0x" + w(9).hex(), "0x" + w(10).hex()
    sender = {"recipient": recipient, "relayer": dep["relayer"], "outsider": dep["outsiders"][0]}[a.sender]
    # The LEZ tip the root is as of (the chain proof's), or for a bare membership proof the latest block.
    tip_id = int.from_bytes(w(16), "big") if len(j) == 576 else lez_tip(a.lez)
    tip_hash_journal = "0x" + w(17).hex() if len(j) == 576 else None
    hash_seen = lez_block_hash(a.lez, tip_id)
    final = tip_id
    if a.wait_final:
        while lez_tip(a.lez) < tip_id + depth:
            time.sleep(1)
        final = lez_tip(a.lez)

    relayer = sender
    if a.recipient_as_sender:
        recipient = sender
    if a.amount:
        amount = a.amount
    if a.tamper_journal:
        recipient = sender
        j = j[:6 * 32] + bytes.fromhex(sender[2:]).rjust(32, b"\0") + j[7 * 32:]
    if a.tamper_anchor:
        j = j[:15 * 32] + bytes(31) + b"\x01" + j[16 * 32:]

    pairing = "0x" + w(3).hex()
    signers = dep["outsiders"] if a.outsiders else [dep["committee"][int(i)] for i in a.members.split(",") if i != ""]
    sigs = []
    for s_ in signers:
        hsh = (tip_hash_journal or hash_seen) if a.outsiders else hash_seen  # members sign what they read; outsiders what they are told
        d = cast("call", verifier, "blockAttestationDigest(address,bytes32,bytes32,uint64,uint64)(bytes32)", vault, pairing, hsh,
                 str(tip_id), str(final), "--rpc-url", a.anvil)
        sigs.append(ok(a.anvil, "eth_sign", [s_, d]))
    proof = cast("abi-encode", "f((bytes,bytes,uint64,bytes[]))", f"({z['seal']},0x{j.hex()},{final},[{','.join(sigs)}])")
    data = cast("calldata", "release(address,uint256,address,address,uint256,bytes32,bytes32,bytes)",
                token, amount, recipient, relayer, fee, root, nullifier, proof)

    hidden = {}
    if not a.forged and not a.membership_proof:
        sec = bytes.fromhex(pr["secret"][2:])
        ec = hashlib.sha256(b"rfp022/eth-commit/v1".ljust(32, b"\0") + bytes.fromhex(pr["recipient"][2:]).rjust(32, b"\0")
                            + int(pr["fee"]).to_bytes(32, "big") + sec).hexdigest()
        zeros, zz = set(), bytes(32)
        for _ in range(25):
            zeros.add(zz.hex()); zz = hashlib.sha256(zz + zz).digest()
        nodes = [p[2:] for p in pr.get("chain_path", []) if p[2:] not in zeros]
        hidden = {"leaf": pr["leaf"][2:] not in data, "secret": sec.hex() not in data, "eth_commitment": ec not in data,
                  "path_nodes": f"{sum(n not in data for n in nodes)}/{len(nodes)}"}

    call = {"from": sender, "to": vault, "data": data, "gas": hex(int(a.gas_limit))}
    sim = rpc(a.anvil, "eth_call", [call, "latest"])
    selectors = {cast("sig", e): e.split("(")[0] for e in ERRORS}
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
    with open(a.out, "a") as f:
        f.write(f"# eth {a.label} {a.expect} {verdict} {int(rc['blockNumber'], 16)} {h} {sel} {int(rc['gasUsed'], 16)} {nullifier} {root}\n")
        f.write(f"# eth-block-attestation {a.label} lez_block {tip_id} lez_final_block {final} hash_seen {hash_seen} hash_journal {tip_hash_journal or '-'} "
                f"signers {','.join(signers)} sender {sender}\n")
        if hidden:
            f.write(f"# eth-zk {a.label} {h} hides leaf {hidden['leaf']} secret {hidden['secret']} eth_commitment {hidden['eth_commitment']} "
                    f"path_nodes {hidden['path_nodes']} calldata_bytes {(len(data) - 2) // 2} seal_bytes {(len(z['seal']) - 2) // 2}\n")
    good = (a.expect == "applied" and verdict == "applied" and pre == "applied") or (a.expect != "applied" and verdict == "reverted" and pre == a.expect)
    if hidden and not (hidden["leaf"] and hidden["secret"] and hidden["eth_commitment"] and hidden["path_nodes"].split("/")[0] == hidden["path_nodes"].split("/")[1]):
        good = False
    print(json.dumps({"label": a.label, "expect": a.expect, "precheck": pre, "verdict": verdict, "tx": h, "block": int(rc["blockNumber"], 16),
                      "gas_used": int(rc["gasUsed"], 16), "lez_block": tip_id, "lez_final_block": final, "hash_seen": hash_seen,
                      "hash_journal": tip_hash_journal, "root": root, "nullifier": nullifier, "hidden": hidden, "ok": good}))
    sys.exit(0 if good else 1)


if __name__ == "__main__":
    main()

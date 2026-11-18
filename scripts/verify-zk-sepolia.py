#!/usr/bin/env python3
"""Re-check the zero-knowledge release on Sepolia (evidence/zk-sepolia/) from
Sepolia's public JSON-RPC and the committed local LEZ and Bedrock evidence.

    scripts/verify-zk-sepolia.py [--eth-rpc URL] [--dir evidence/zk-sepolia]

What is public, and re-read here from Sepolia: the three contracts (RISC
Zero's Groth16 verifier, ZkBedrockBurnVerifier, the vault) and what they are
pinned to; the two deposits; carol's release, whose calldata carries the
Groth16 seal and the 800-byte journal; the refusals. For the release, the
journal is decoded and the seal is checked again, now, by the deployed RISC
Zero verifier (eth_call `verify(seal, IMAGE_ID, sha256(journal))`): a
release that passed once and whose proof does not verify today fails here.
The two committee signatures are recovered from the calldata.

What is local (the public LEZ testnet halted on 2026-11-17): the LEZ
sequencer and the Bedrock devnet the burn was made and anchored on. Their
part is checked against the committed files: the journal's channel, signer,
Bedrock block, top header and depth are the committed anchor's; its LEZ tip
is the committed chain proof's; carol's burn leaf is in that proof's leaves.
"""
import argparse
import hashlib
import json
import os
import subprocess
import sys
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
fails = 0


def check(label, got, want):
    global fails
    if got == want:
        print(f"  ok   {label}")
    else:
        print(f"  FAIL {label}: got {got}, want {want}")
        fails += 1


def rpc(url, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(url, body, {"Content-Type": "application/json", "User-Agent": "portage-verify"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)


def ok(url, method, params):
    r = rpc(url, method, params)
    if "error" in r:
        sys.exit(f"{method}: {r['error']}")
    return r["result"]


def cast(*a):
    r = subprocess.run(["cast", *a], capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip())
    return r.stdout.strip()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--eth-rpc", default=os.environ.get("SEPOLIA_RPC", "https://rpc.sepolia.ethpandaops.io"))
    ap.add_argument("--dir", default=os.path.join(ROOT, "evidence/zk-sepolia"))
    a = ap.parse_args()
    E, D = a.eth_rpc, a.dir
    dep = json.load(open(f"{D}/deployment.json"))
    rel = json.load(open(f"{D}/releases.json"))
    anchor = json.load(open(f"{D}/anchor-block.json"))
    chain = json.load(open(f"{D}/chain-2.json"))
    proof = json.load(open(f"{D}/proof-carol.json"))
    vault, verifier, risc0 = dep["vault"], dep["verifier"], dep["risc0_verifier"]
    call = lambda to, sig, *x: cast("call", to, sig, *[str(v) for v in x], "--rpc-url", E)

    print("== Sepolia: the contracts")
    check("chain id", int(ok(E, "eth_chainId", []), 16), 11155111)
    for n, x in (("RISC Zero Groth16 verifier", risc0), ("ZkBedrockBurnVerifier", verifier), ("vault", vault)):
        check(f"{n} {x} has code", len(ok(E, "eth_getCode", [x, "latest"])) > 2, True)
    check("the vault's verifier", call(vault, "VERIFIER()(address)").lower(), verifier.lower())
    check("the vault's pairing", call(vault, "PAIRING()(bytes32)"), dep["pairing"])
    check("the verifier's RISC Zero verifier", call(verifier, "RISC0()(address)").lower(), risc0.lower())
    for k, want in (("IMAGE_ID", dep["image_id"]), ("CHAIN_IMAGE_ID", dep["chain_image_id"]), ("ANCHOR_IMAGE_ID", dep["anchor_image_id"]),
                    ("PROGRAM_ID", dep["program_id"]), ("BURNS_ACCOUNT", dep["burns_account"]), ("ANCHOR_HASH", dep["anchor_hash"]),
                    ("CHANNEL_ID", "0x" + dep["channel_id"])):
        check(f"ZkBedrockBurnVerifier.{k}", call(verifier, f"{k}()(bytes32)"), want)
    # The 2026-11-18 deployment pins one sequencer key (deployed from commit f274338); a verifier
    # built from the current source pins the channel's key set instead (docs/ZK-SEPOLIA.md).
    if dep.get("channel_keys"):
        for k in dep["channel_keys"]:
            check(f"ZkBedrockBurnVerifier.isChannelKey({k[:8]}…)", call(verifier, "isChannelKey(bytes32)(bool)", "0x" + k), "true")
    else:
        check("ZkBedrockBurnVerifier.SEQUENCER_KEY", call(verifier, "SEQUENCER_KEY()(bytes32)"), "0x" + dep["sequencer_key"])
    check("ZkBedrockBurnVerifier.MIN_DEPTH", int(call(verifier, "MIN_DEPTH()(uint32)")), dep["min_depth"])
    # The guests the verifier is pinned to are this repository's committed, Docker-reproducible binaries.
    wrap = os.path.join(ROOT, "cli/target/release/wrap")
    if os.path.exists(wrap):
        img = json.loads(subprocess.run([wrap, "image-id"], capture_output=True, text=True).stdout)
        for g, k in (("bedrock_release", "image_id"), ("lez_chain", "chain_image_id"), ("bedrock_anchor", "anchor_image_id")):
            check(f"{g}: the committed guest's image id is the one pinned", "0x" + img[g]["image_id"], dep[k])
    else:
        print("  (cli/target/release/wrap not built: the pinned image ids are not recomputed from the committed guests)")

    print("== Sepolia: the deposits")
    for x in rel["deposits"]:
        r = ok(E, "eth_getTransactionReceipt", [x["tx"]])
        check(f"deposit {x['tx'][:12]}… succeeded, to the vault", (r["status"], r["to"].lower()), ("0x1", vault.lower()))

    print("== Sepolia: carol's release")
    top = None
    for x in rel["attempts"]:
        r = ok(E, "eth_getTransactionReceipt", [x["tx"]])
        t = ok(E, "eth_getTransactionByHash", [x["tx"]])
        want = "0x1" if x["expect"] == "applied" else "0x0"
        check(f"{x['label']}: {x['expect']} ({x['tx'][:12]}…, block {int(r['blockNumber'], 16)})", (r["status"], r["to"].lower()), (want, vault.lower()))
        if x["label"] != "release-carol":
            continue
        dec = cast("decode-calldata", "release(address,uint256,address,address,uint256,bytes32,bytes32,bytes)", t["input"]).splitlines()
        token, amount, recipient, relayer, fee, root, nullifier, pf = dec
        inner = cast("abi-decode", "f()((bytes,bytes,bytes[]))", pf)
        seal, journal, sigs = inner.strip("()").split(", ", 2)
        j = bytes.fromhex(journal[2:])
        w = lambda k: j[32 * k:32 * (k + 1)]
        check("the journal is 800 bytes (release preimage, chain, Bedrock anchor)", len(j), 800)
        check("journal: Sepolia's chain id", int.from_bytes(w(1), "big"), 11155111)
        check("journal: this vault", "0x" + w(2)[12:].hex(), vault.lower())
        check("journal: the pairing", "0x" + w(3).hex(), dep["pairing"])
        check("journal: the recipient carol's burn committed to", "0x" + w(6)[12:].hex(), dep["carol"].lower())
        check("journal: 0.001 ETH", int.from_bytes(w(5), "big"), 10 ** 15)
        check("the calldata's recipient and amount are the journal's", (recipient.lower(), int(amount.split()[0])), ("0x" + w(6)[12:].hex(), int.from_bytes(w(5), "big")))
        check("the calldata's nullifier is the journal's", nullifier, "0x" + w(10).hex())
        digest = "0x" + hashlib.sha256(j).hexdigest()
        try:
            call(risc0, "verify(bytes,bytes32,bytes32)", seal, dep["image_id"], digest)
            v = True
        except RuntimeError as e:
            v = str(e)[:120]
        check("the Groth16 seal verifies today on Sepolia (RISC Zero's verifier, the pinned image id, sha256 of the journal)", v, True)
        check("the vault records the nullifier as spent", call(vault, "nullifierSpent(bytes32)(bool)", nullifier), "true")
        top = w(22).hex()
        check("journal: the channel the verifier pins", w(19).hex(), dep["channel_id"])
        if dep.get("channel_keys"):
            check("journal: the signer is one of the channel keys the verifier pins", w(20).hex() in dep["channel_keys"], True)
        else:
            check("journal: the sequencer key the verifier pins", w(20).hex(), dep["sequencer_key"])
        check("journal: the top Bedrock header is the committed anchor's", top, anchor["top"]["id"].removeprefix("0x"))
        check("journal: the depth is at least MIN_DEPTH", int.from_bytes(w(24), "big") >= dep["min_depth"], True)
        check("journal: the Bedrock block is the committed anchor's", w(21).hex(), anchor["bedrock_block"]["id"].removeprefix("0x"))
        check("journal: the LEZ tip is the committed chain proof's", (int.from_bytes(w(16), "big"), "0x" + w(17).hex()),
              (int(chain["tip"]["id"]), chain["tip"]["hash"]))
        check("the anchor inscribes that LEZ tip", int(anchor["lez_block_id"]), int(chain["tip"]["id"]))
        check("the chain proof's anchor is the one the verifier pins", (int(chain["anchor"]["id"]), chain["anchor"]["hash"]),
              (int(dep["anchor_id"]), dep["anchor_hash"]))
        leaves = [l["leaf"] for l in chain["leaves"]]
        check("carol's burn is a leaf of the burn tree the chain proof rebuilt", proof["leaf"] in leaves, True)
        # The committee signatures: who signed the top header.
        d = call(verifier, "headerAttestationDigest(address,bytes32,bytes32)(bytes32)", vault, dep["pairing"], "0x" + top)
        from eth_account import Account
        from eth_account.messages import encode_defunct
        signers = [Account.recover_message(encode_defunct(primitive=bytes.fromhex(d[2:])), signature=s)
                   for s in sigs.strip("[]").split(", ") if s]
        check("two distinct committee members signed the top header", len({x.lower() for x in signers} & {c.lower() for c in dep["committee"]}), 2)
    if top is None:
        check("carol's release is among the attempts", False, True)
    print(f"== {'PASS' if fails == 0 else 'FAIL'}: {fails} failed")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()

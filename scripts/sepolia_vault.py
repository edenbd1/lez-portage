#!/usr/bin/env python3
"""The Ethereum half of scripts/testnet-inbound.sh, on Sepolia.

    sepolia_vault.py deploy   --plan plan.json --out DIR --keys KEYDIR
    sepolia_vault.py deposit  --plan plan.json --out DIR --keys KEYDIR
    sepolia_vault.py proofs   --out DIR --block-hash 0x...
    sepolia_vault.py predict    --keys KEYDIR
    sepolia_vault.py deploy-zk  --plan plan.json --out DIR --keys KEYDIR --chain-params CP.json --bedrock BEDROCK.json

`predict` prints the deployer and the addresses its next three contracts will
have (CREATE: deployer and nonce). `deploy-zk` deploys a vault whose release
verifier is vault/src/ZkBedrockBurnVerifier.sol (scripts/testnet-zk.sh): RISC
Zero's RiscZeroGroth16Verifier (risc0-ethereum v3.0.1 as vendored, its control
root and BN254 control id), the verifier, pinned to the three guests, the
bridge program and the burn-tree account of the configuration that names the
vault's own address (hence `predict`: the configuration commits to the vault,
the verifier to the configuration's burn tree, the vault to its verifier), the
chain proof's anchor (a public LEZ block before the instance's Init), Bedrock's
channel and the sequencer's key, and MIN_DEPTH; a 2-of-3 committee that attests
Bedrock headers canonical; then the vault, with the plan's pairing.

`deploy` deploys vault/src/WrapVault.sol with the pairing of the LEZ
deployment (`wrap plan`), registers Ether with one permitted amount, and its
release verifier, vault/src/CommitteeBurnVerifier.sol: this run exercises the
inbound path only, and no release is made on Sepolia. `deposit` makes the
deposits of the plan (Ether only). `proofs` records, in the layout of
vectors/anvil, the vault's Deposit events, the block a Sextant checkpoint
commits to, an `eth_getProof` at that block of `knownRoot[root]` for every
root the tree had and for one root it never had, each checked against the
block's state root.

The deployer's key is read from the file named by $SEPOLIA_KEY_FILE and put in
an encrypted keystore in KEYDIR (with a random password there); it is never
printed and never passed on a command line. KEYDIR also keeps the two other
committee members' keys, made here.
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

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ETH = "0x0000000000000000000000000000000000000000"
RPC = os.environ.get("SEPOLIA_RPC", "https://ethereum-sepolia-rpc.publicnode.com")
# An endpoint whose eth_getProof reaches back to a finalized block (publicnode's does not).
PROOF_RPC = os.environ.get("SEPOLIA_PROOF_RPC", "https://sepolia.rpc.thirdweb.com")
CHAIN_ID = 11155111
FINALITY_DEPTH = "30"


def run(*args):
    r = subprocess.run(args, cwd=ROOT, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"{' '.join(args[:3])} failed\n{r.stdout[-2000:]}\n{r.stderr[-2000:]}")
    return r.stdout.strip()


def rpc(url, method, params, tries=6):
    data = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    for i in range(tries):
        try:
            req = urllib.request.Request(url, data=data, headers={"content-type": "application/json", "user-agent": "portage"})
            with urllib.request.urlopen(req, timeout=60) as r:
                j = json.loads(r.read())
            if "error" in j:
                raise RuntimeError(j["error"])
            return j["result"]
        except Exception as e:  # noqa: BLE001
            last = e
            time.sleep(3 * (i + 1))
    sys.exit(f"{method} on {url} failed: {last}")


def keccak(b):
    from Crypto.Hash import keccak as k
    h = k.new(digest_bits=256)
    h.update(b)
    return h.digest()


def keystore(keys):
    """The deployer's keystore and password file in `keys`, made once."""
    ks, pw = os.path.join(keys, "deployer.keystore.json"), os.path.join(keys, "deployer.password")
    if not os.path.exists(ks):
        from eth_account import Account
        key = open(os.environ["SEPOLIA_KEY_FILE"]).read().strip()
        password = secrets.token_hex(16)
        os.makedirs(keys, mode=0o700, exist_ok=True)
        with open(os.open(pw, os.O_WRONLY | os.O_CREAT, 0o600), "w") as f:
            f.write(password)
        with open(os.open(ks, os.O_WRONLY | os.O_CREAT, 0o600), "w") as f:
            json.dump(Account.encrypt(key, password), f)
        del key
    return ["--keystore", ks, "--password-file", pw]


def deployer(keys):
    from eth_account import Account
    return Account.from_key(open(os.environ["SEPOLIA_KEY_FILE"]).read().strip()).address


def receipt(h):
    for _ in range(120):
        r = rpc(RPC, "eth_getTransactionReceipt", [h])
        if r:
            return r
        time.sleep(5)
    sys.exit(f"{h} not mined after 10 minutes")


def send(keys, to, sig, *args, value=None):
    extra = ["--value", value] if value else []
    out = run("cast", "send", "--json", "--async", *keystore(keys), "--rpc-url", RPC, *extra, to, sig, *args)
    h = out.strip().strip('"')
    r = receipt(h)
    assert r["status"] in ("0x1", 1, "1"), f"{sig} reverted: {h}"
    return r


def create(keys, contract, *ctor):
    me = deployer(keys)
    n = int(rpc(RPC, "eth_getTransactionCount", [me, "latest"]), 16)
    p = subprocess.run(["forge", "create", "--root", "vault", "--rpc-url", RPC, *keystore(keys), "--broadcast", contract,
                        *(["--constructor-args", *ctor] if ctor else [])], cwd=ROOT, capture_output=True, text=True)
    if p.returncode == 0:
        field = lambda k: next(l.split()[-1] for l in p.stdout.splitlines() if l.startswith(k))
        to, h = field("Deployed to:"), field("Transaction hash:")
    else:
        # Some public endpoints answer a rebroadcast of the same signed transaction with
        # "nonce too low" after it was mined: find it by its nonce instead.
        if "nonce too low" not in p.stderr:
            sys.exit(f"forge create {contract} failed\n{p.stdout[-2000:]}\n{p.stderr[-2000:]}")
        to, h = None, None
        latest = int(rpc(RPC, "eth_blockNumber", []), 16)
        for b in range(latest, latest - 30, -1):
            for t in rpc(RPC, "eth_getBlockByNumber", [hex(b), True])["transactions"]:
                if t["from"].lower() == me.lower() and t["to"] is None and int(t["nonce"], 16) == n:
                    h = t["hash"]
            if h:
                break
        assert h, f"forge create {contract}: no creation at nonce {n}"
        to = rpc(RPC, "eth_getTransactionReceipt", [h])["contractAddress"]
    r = receipt(h)
    assert r["status"] in ("0x1", 1, "1") and r["contractAddress"].lower() == to.lower()
    return to, h, int(r["blockNumber"], 16)


def dump(out, name, obj):
    with open(os.path.join(out, name), "w") as f:
        json.dump(obj, f, indent=1)
        f.write("\n")


def cmd_deploy(a):
    plan = json.load(open(a.plan))
    os.makedirs(a.out, exist_ok=True)
    if os.path.exists(os.path.join(a.out, "deployment.json")):
        sys.exit(f"{a.out}/deployment.json exists: this vault is deployed")
    assert int(run("cast", "chain-id", "--rpc-url", RPC)) == CHAIN_ID
    os.makedirs(a.keys, mode=0o700, exist_ok=True)
    me = deployer(a.keys)
    # The other two committee members: fresh keys, kept in KEYDIR, never used here.
    cf = os.path.join(a.keys, "committee.json")
    if not os.path.exists(cf):
        from eth_account import Account
        ms = [Account.create() for _ in range(2)]
        with open(os.open(cf, os.O_WRONLY | os.O_CREAT, 0o600), "w") as f:
            json.dump([{"address": m.address, "key": m.key.hex()} for m in ms], f)
    committee = [me] + [m["address"] for m in json.load(open(cf))]
    run("forge", "build", "--root", "vault")
    verifier, vtx, vblock = create(a.keys, "src/CommitteeBurnVerifier.sol:CommitteeBurnVerifier", "[" + ",".join(committee) + "]", "2", FINALITY_DEPTH)
    vault, ttx, tblock = create(a.keys, "src/WrapVault.sol:WrapVault", plan["pairing"], verifier, "86400", me, me)
    amounts = sorted({int(d["amount"]) for d in plan["deposits"]})
    cap = str(10 * max(amounts))
    r = send(a.keys, vault, "registerToken(address,uint8,uint256[],uint128,uint128)", ETH, "18", "[" + ",".join(map(str, amounts)) + "]", cap, cap)
    print(f"verifier {verifier} (tx {vtx}), vault {vault} (tx {ttx}, block {tblock}), Ether registered (tx {r['transactionHash']})")
    dump(a.out, "deployment.json", {
        "chain_id": CHAIN_ID, "vault": vault, "usdt": ETH, "verifier": verifier, "release_verifier": "committee",
        "committee": committee, "threshold": 2, "finality_depth": int(FINALITY_DEPTH), "pairing": plan["pairing"],
        "admin": me, "alice": me, "bob": me, "carol": me, "deploy_block": vblock,
        "txs": {"verifier": vtx, "vault": ttx, "register_eth": r["transactionHash"]}})


def nonce(addr):
    return int(rpc(RPC, "eth_getTransactionCount", [addr, "pending"]), 16)


def create_address(sender, n):
    import rlp  # noqa: F401  (eth_account depends on it)
    from rlp import encode
    return "0x" + keccak(encode([bytes.fromhex(sender[2:]), n]))[12:].hex()


def checksum(a):
    from eth_utils import to_checksum_address
    return to_checksum_address(a)


def cmd_predict(a):
    me = deployer(a.keys)
    n = nonce(me)
    k = 0 if a.risc0 else 1
    print(json.dumps({"deployer": me, "nonce": n, "risc0": a.risc0 or checksum(create_address(me, n)),
                      "verifier": checksum(create_address(me, n + k)), "vault": checksum(create_address(me, n + k + 1))}))


def cmd_deploy_zk(a):
    plan = json.load(open(a.plan))
    cp = json.load(open(a.chain_params))
    bd = json.load(open(a.bedrock))
    os.makedirs(a.out, exist_ok=True)
    if os.path.exists(os.path.join(a.out, "deployment.json")) and json.load(open(os.path.join(a.out, "deployment.json"))).get("txs"):
        sys.exit(f"{a.out}/deployment.json records a deployment: this vault is deployed")
    assert int(run("cast", "chain-id", "--rpc-url", RPC)) == CHAIN_ID
    me = deployer(a.keys)
    n = nonce(me)
    # --risc0 ADDRESS: reuse a RiscZeroGroth16Verifier already deployed (it is stateless and pinned to nothing of ours)
    k = 0 if a.risc0 else 1
    want = {"risc0": a.risc0 or create_address(me, n), "verifier": create_address(me, n + k), "vault": create_address(me, n + k + 1)}
    assert want["vault"].lower() == cp["vault"].lower(), f"the configuration names vault {cp['vault']}, the next one is {want['vault']}"
    assert cp["pairing"] == plan["pairing"], "the chain parameters are another deployment's"
    cf = os.path.join(a.keys, "committee.json")
    committee = [me] + [m["address"] for m in json.load(open(cf))]
    run("forge", "build", "--root", "vault")
    cid = open(os.path.join(ROOT, "vault/lib/risc0-ethereum/contracts/src/groth16/ControlID.sol")).read()
    val = lambda k: "0x" + cid.split(k + ' = hex"')[1].split('"')[0]
    if a.risc0:
        risc0, rtx = a.risc0, None
        assert run("cast", "call", risc0, "SELECTOR()(bytes4)", "--rpc-url", RPC), "no RISC Zero verifier at --risc0"
    else:
        risc0, rtx, _ = create(a.keys, "lib/risc0-ethereum/contracts/src/groth16/RiscZeroGroth16Verifier.sol:RiscZeroGroth16Verifier",
                               val("CONTROL_ROOT"), val("BN254_CONTROL_ID"))
    # The channel's accredited key set (ascending), or its one sequencer key: the verifier pins the set
    keys = sorted(k.lower().removeprefix("0x") for k in (bd.get("channel_keys") or [bd["sequencer_key"]]))
    cfg = (f"({risc0},{bd['release_image_id']},{bd['chain_image_id']},{bd['anchor_image_id']},{cp['program_id']},{cp['burns_account']},"
           f"{cp['block_id']},{cp['block_hash']},0x{bd['channel_id']},[{','.join('0x' + k for k in keys)}],{bd['min_depth']})")
    verifier, vtx, vblock = create(a.keys, "src/ZkBedrockBurnVerifier.sol:ZkBedrockBurnVerifier", cfg, "[" + ",".join(committee) + "]", "2")
    vault, ttx, tblock = create(a.keys, "src/WrapVault.sol:WrapVault", plan["pairing"], verifier, "86400", me, me)
    for k in ("risc0", "verifier", "vault"):
        got = {"risc0": risc0, "verifier": verifier, "vault": vault}[k]
        assert got.lower() == want[k].lower(), f"{k} landed at {got}, not {want[k]}"
    amounts = sorted({int(d["amount"]) for d in plan["deposits"]})
    cap = str(10 * max(amounts))
    r = send(a.keys, vault, "registerToken(address,uint8,uint256[],uint128,uint128)", ETH, "18", "[" + ",".join(map(str, amounts)) + "]", cap, cap)
    rec = json.load(open(os.path.join(a.keys, "recipients.json")))
    print(f"risc0 {risc0} (tx {rtx}), verifier {verifier} (tx {vtx}), vault {vault} (tx {ttx}, block {tblock}), Ether registered (tx {r['transactionHash']})")
    dump(a.out, "deployment.json", {
        "chain_id": CHAIN_ID, "vault": vault, "usdt": ETH, "verifier": verifier, "release_verifier": "zkbedrock", "risc0_verifier": risc0,
        "committee": committee, "threshold": 2, "pairing": plan["pairing"],
        "image_id": bd["release_image_id"], "chain_image_id": bd["chain_image_id"], "anchor_image_id": bd["anchor_image_id"],
        "program_id": cp["program_id"], "burns_account": cp["burns_account"], "anchor_id": cp["block_id"], "anchor_hash": cp["block_hash"],
        "channel_id": bd["channel_id"], "channel_keys": keys, "min_depth": bd["min_depth"],
        "admin": me, "relayer": me, "alice": rec["alice"], "bob": me, "carol": rec["carol"], "deploy_block": vblock,
        "txs": {"risc0": rtx, "verifier": vtx, "vault": ttx, "register_eth": r["transactionHash"]}})


def logs_of(d, to="latest"):
    return rpc(RPC, "eth_getLogs", [{"address": d["vault"], "fromBlock": hex(d["deploy_block"]), "toBlock": to, "topics": [TOPIC]}])


def cmd_deposit(a):
    plan = json.load(open(a.plan))
    d = json.load(open(os.path.join(a.out, "deployment.json")))
    assert plan["pairing"] == d["pairing"], "the plan is another deployment's"
    vault = d["vault"]
    assert run("cast", "call", vault, "PAIRING()(bytes32)", "--rpc-url", RPC) == plan["pairing"]
    # Resumable: a deposit already in the vault's events (by its commitment) is not made again.
    made = {"0x" + l["data"][2 + 64:2 + 128]: l for l in logs_of(d)}
    txs = []
    for x in plan["deposits"]:
        assert x["token"] == "eth", "this run deposits Ether only"
        if x["commitment"] in made:
            h = made[x["commitment"]]["transactionHash"]
            r = receipt(h)
        else:
            r = send(a.keys, vault, "deposit(address,uint256,bytes32)", ETH, x["amount"], x["commitment"], value=x["amount"])
        b = int(r["blockNumber"], 16)
        bal = lambda n: int(rpc(RPC, "eth_getBalance", [vault, hex(n)]), 16)
        assert bal(b) - bal(b - 1) == int(x["amount"]), "the vault received exactly the amount"
        txs.append({"tx": r["transactionHash"], "block": b, "gas": int(r["gasUsed"], 16)})
        print(f"deposit {len(txs) - 1}: {x['from']} {x['amount']} wei, tx {r['transactionHash']}, block {b}, gas {int(r['gasUsed'], 16)}")
    dump(a.out, "deposit-txs.json", txs)


TOPIC = "0x" + keccak(b"Deposit(uint256,bytes32,bytes32,address,uint256,bytes32)").hex()


def cmd_proofs(a):
    d = json.load(open(os.path.join(a.out, "deployment.json")))
    vault = d["vault"]
    block = rpc(RPC, "eth_getBlockByHash", [a.block_hash, False])
    n = int(block["number"], 16)
    logs = logs_of(d, hex(n))
    deposits = []
    for l in logs:
        data = l["data"][2:]
        w = [data[i:i + 64] for i in range(0, len(data), 64)]
        deposits.append({"index": int(l["topics"][1], 16), "token": "0x" + l["topics"][2][-40:], "leaf": "0x" + w[0],
                         "commitment": "0x" + w[1], "amount": str(int(w[2], 16)), "root": "0x" + w[3],
                         "transactionHash": l["transactionHash"], "blockNumber": int(l["blockNumber"], 16)})
    made = json.load(open(os.path.join(a.out, "deposit-txs.json")))
    assert [x["transactionHash"] for x in deposits] == [x["tx"] for x in made], "every deposit is at or before the checkpoint's block"
    # The empty tree's root, then the root after each deposit.
    z = bytes(32)
    for _ in range(24):
        z = hashlib.sha256(z + z).digest()
    roots = ["0x" + z.hex()] + [x["root"] for x in deposits]

    def proof(root):
        slot = "0x" + keccak(bytes.fromhex(root[2:]) + bytes(32)).hex()
        r = rpc(PROOF_RPC, "eth_getProof", [vault, [slot], {"blockHash": a.block_hash}])
        assert "0x" + keccak(bytes.fromhex(r["accountProof"][0][2:])).hex() == block["stateRoot"], "the proof is not under the block's state root"
        return {"root": root, "slot": slot, "proof": {"result": r}}

    proofs = [proof(r) for r in roots]
    for p in proofs:
        assert int(p["proof"]["result"]["storageProof"][0]["value"], 16) == 1, "every root the tree had is known"
    plan = json.load(open(a.plan)) if a.plan else None
    unknown = None
    if plan:
        amount = plan["deposits"][0]["amount"]
        leaf = run("cast", "call", vault, "leafOf(address,uint256,bytes32)(bytes32)", ETH, amount, plan["phantom"], "--rpc-url", RPC)
        node, zero = bytes.fromhex(leaf[2:]), bytes(32)
        for _ in range(24):
            node = hashlib.sha256(node + zero).digest()
            zero = hashlib.sha256(zero + zero).digest()
        unknown = proof("0x" + node.hex())
        unknown["leaf"] = leaf
        assert int(unknown["proof"]["result"]["storageProof"][0]["value"], 16) == 0
        dump(a.out, "proof-unknown.json", unknown)
    dump(a.out, "deposits.json", deposits)
    dump(a.out, "block.json", {"result": block})
    dump(a.out, "proofs.json", proofs)
    print(f"{len(deposits)} deposits, block {n} ({a.block_hash}, state root {block['stateRoot']}), root {roots[-1]}, "
          f"{len(proofs[-1]['proof']['result']['accountProof'])} account and {len(proofs[-1]['proof']['result']['storageProof'][0]['proof'])} storage proof nodes")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["deploy", "deposit", "proofs", "predict", "deploy-zk"])
    ap.add_argument("--plan")
    ap.add_argument("--out", required=True)
    ap.add_argument("--keys")
    ap.add_argument("--block-hash")
    ap.add_argument("--chain-params")
    ap.add_argument("--bedrock")
    ap.add_argument("--risc0")
    a = ap.parse_args()
    {"deploy": cmd_deploy, "deposit": cmd_deposit, "proofs": cmd_proofs, "predict": cmd_predict, "deploy-zk": cmd_deploy_zk}[a.cmd](a)


if __name__ == "__main__":
    main()

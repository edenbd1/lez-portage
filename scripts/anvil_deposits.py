#!/usr/bin/env python3
"""The Ethereum half of scripts/e2e-local.sh, against an anvil that is already running.

Deploys vault/src/WrapVault.sol with the pairing of the LEZ deployment the run
just loaded, a USDT-shaped token whose transfers return nothing, and the
release verifier (vault/src/CommitteeBurnVerifier.sol: anvil accounts 5, 6 and
7 as a 2-of-3 committee attesting LEZ burn-tree roots, `--finality-depth` LEZ
blocks); registers ETH and USDT; makes the deposits of the
plan (`wrap plan`: depositor, token, amount, commitment); then records, in the
layout of vectors/anvil, the Deposit events, the last block, an `eth_getProof`
of `knownRoot[root]` for every root the tree had, and one for a root it never
had.

    python3 scripts/anvil_deposits.py --rpc http://127.0.0.1:8551 --plan plan.json --out DIR

Uses anvil's well-known development keys only; the chain is local.
"""
import argparse
import hashlib
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# anvil's well-known development keys (accounts 0 to 3 of its default mnemonic).
KEYS = {
    "deployer": "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    "alice": "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
    "bob": "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
    "carol": "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
}
ETH = "0x0000000000000000000000000000000000000000"


def run(*args):
    r = subprocess.run(args, cwd=ROOT, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"{' '.join(a if not a.startswith('0x') or len(a) != 66 else '<key>' for a in args)}\n{r.stdout}\n{r.stderr}")
    return r.stdout.strip()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", required=True)
    ap.add_argument("--plan", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--finality-depth", default="3")
    a = ap.parse_args()
    rpc = a.rpc
    plan = json.load(open(a.plan))
    os.makedirs(a.out, exist_ok=True)

    cast = lambda *args: run("cast", *args, "--rpc-url", rpc)

    def send(who, to, sig, *args, value=None):
        extra = ["--value", value] if value else []
        out = cast("send", "--json", "--private-key", KEYS[who], *extra, to, sig, *args)
        r = json.loads(out)
        assert r["status"] in ("0x1", 1, "1"), f"{sig} reverted"
        return r

    def create(contract, *ctor):
        out = run("forge", "create", "--root", "vault", "--rpc-url", rpc, "--private-key", KEYS["deployer"], "--broadcast", contract,
                  *(["--constructor-args", *ctor] if ctor else []))
        return next(l.split()[-1] for l in out.splitlines() if l.startswith("Deployed to:"))

    run("forge", "build", "--root", "vault")
    addr = {k: run("cast", "wallet", "address", v) for k, v in KEYS.items()}
    # The release committee: anvil's dev accounts 5, 6, 7 (unlocked on anvil; they sign
    # with eth_sign, so no key leaves anvil). Their addresses are derived from the
    # node, not written here.
    accounts = json.loads(cast("rpc", "eth_accounts"))
    committee = [accounts[5], accounts[6], accounts[7]]
    verifier = create("src/CommitteeBurnVerifier.sol:CommitteeBurnVerifier", "[" + ",".join(committee) + "]", "2", a.finality_depth)
    usdt = create("test/Mocks.sol:MockUSDT")
    vault = create("src/WrapVault.sol:WrapVault", plan["pairing"], verifier, "86400", addr["deployer"], addr["deployer"])
    print(f"vault {vault} (pairing {plan['pairing']}), usdt {usdt}, verifier {verifier}")

    send("deployer", vault, "registerToken(address,uint8,uint256[],uint128,uint128)", ETH, "18",
         "[100000000000000000,1000000000000000000]", "100000000000000000000", "100000000000000000000")
    send("deployer", vault, "registerToken(address,uint8,uint256[],uint128,uint128)", usdt, "6",
         "[100000000,1000000000]", "10000000000", "10000000000")
    for who in ("alice", "bob", "carol"):
        send("deployer", usdt, "mint(address,uint256)", addr[who], "1000000000000")
        send(who, usdt, "approve(address,uint256)", vault, "1000000000000")

    roots = [cast("call", vault, "root()(bytes32)")]
    txs = []
    for d in plan["deposits"]:
        token = ETH if d["token"] == "eth" else usdt
        bal = lambda: int(cast("balance", vault).split()[0]) if token == ETH else int(cast("call", usdt, "balanceOf(address)(uint256)", vault).split()[0])
        before = bal()
        r = send(d["from"], vault, "deposit(address,uint256,bytes32)", token, d["amount"], d["commitment"],
                 value=d["amount"] if token == ETH else None)
        assert bal() - before == int(d["amount"]), "the vault received exactly the amount"
        roots.append(cast("call", vault, "root()(bytes32)"))
        txs.append(r["transactionHash"])
        print(f"deposit {len(txs) - 1}: {d['from']} {d['token']} {d['amount']}, tx {r['transactionHash']}, gas {int(r['gasUsed'], 16)}")

    logs = json.loads(cast("logs", "--json", "--from-block", "0", "--address", vault,
                           "Deposit(uint256,bytes32,bytes32,address,uint256,bytes32)"))
    deposits = []
    for l in logs:
        data = l["data"][2:]
        words = [data[i:i + 64] for i in range(0, len(data), 64)]
        deposits.append({
            "index": int(l["topics"][1], 16),
            "token": "0x" + l["topics"][2][-40:],
            "leaf": "0x" + words[0],
            "commitment": "0x" + words[1],
            "amount": str(int(words[2], 16)),
            "root": "0x" + words[3],
            "transactionHash": l["transactionHash"],
            "blockNumber": int(l["blockNumber"], 16),
        })
    assert [d["root"] for d in deposits] == roots[1:], "event roots are the vault's roots"
    assert [d["transactionHash"] for d in deposits] == txs

    block = json.loads(cast("rpc", "eth_getBlockByNumber", "latest", "false"))
    number = block["number"]

    def proof(root):
        slot = run("cast", "index", "bytes32", root, "0")
        return {"root": root, "slot": slot,
                "proof": {"result": json.loads(cast("rpc", "eth_getProof", vault, json.dumps([slot]), number))}}

    proofs = [proof(r) for r in roots]
    for p in proofs:
        assert int(p["proof"]["result"]["storageProof"][0]["value"], 16) == 1, "every root the tree had is known"
    # A deposit never made: its leaf, and the root of a tree holding only it.
    leaf = cast("call", vault, "leafOf(address,uint256,bytes32)(bytes32)", ETH, "1000000000000000000", plan["phantom"])
    node = bytes.fromhex(leaf[2:])
    zero = bytes(32)
    for _ in range(24):
        node = hashlib.sha256(node + zero).digest()
        zero = hashlib.sha256(zero + zero).digest()
    unknown = proof("0x" + node.hex())
    unknown["leaf"] = leaf
    assert int(unknown["proof"]["result"]["storageProof"][0]["value"], 16) == 0

    dump = lambda name, obj: json.dump(obj, open(os.path.join(a.out, name), "w"), indent=1)
    dump("deployment.json", {"chain_id": int(cast("chain-id")), "vault": vault, "usdt": usdt, "verifier": verifier,
                             "committee": committee, "threshold": 2, "finality_depth": int(a.finality_depth),
                             "relayer": accounts[4], "outsiders": [accounts[8], accounts[9]],
                             "pairing": plan["pairing"], "admin": addr["deployer"], "alice": addr["alice"],
                             "bob": addr["bob"], "carol": addr["carol"], "block": int(number, 16)})
    dump("deposits.json", deposits)
    dump("block.json", {"result": block})
    dump("proofs.json", proofs)
    dump("proof-unknown.json", unknown)
    print(f"{len(deposits)} deposits, block {int(number, 16)} (state root {block['stateRoot']}), root {roots[-1]}")


if __name__ == "__main__":
    main()

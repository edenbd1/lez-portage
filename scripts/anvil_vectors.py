#!/usr/bin/env python3
"""Make the Ethereum-side vectors the executor tests run on, on a local anvil chain.

Deploys vault/src/WrapVault.sol (with the pairing of the test LEZ deployment),
a USDT-shaped token whose transfers return nothing, and a stand-in release
verifier; registers ETH and USDT; makes five deposits (three by alice, one by
bob, one by carol to a private LEZ account) with the commitments
`executor-tests --bin vectors` derives from each depositor's seed; then
records the Deposit events, the last block, and an `eth_getProof` of
`knownRoot[root]` for every root the tree had, plus one for a root it never
had.

    python3 scripts/anvil_vectors.py      # writes vectors/anvil/*.json

Needs anvil, forge and cast (Foundry); spends nothing: the chain is local and
discarded at the end.
"""
import hashlib
import json
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "vectors", "anvil")
RPC = "http://127.0.0.1:8547"
# anvil's well-known development keys (accounts 0 to 3 of its default mnemonic).
DEPLOYER = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
ALICE = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
BOB = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
CAROL = "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6"
ETH = "0x0000000000000000000000000000000000000000"


def run(*args, cwd=ROOT):
    r = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"{' '.join(args)}\n{r.stdout}\n{r.stderr}")
    return r.stdout.strip()


def cast(*args):
    return run("cast", *args, "--rpc-url", RPC)


def send(key, to, sig, *args, value=None):
    extra = ["--value", value] if value else []
    return cast("send", "--private-key", key, *extra, to, sig, *args)


def create(contract, *ctor):
    out = run("forge", "create", "--root", "vault", "--rpc-url", RPC, "--private-key", DEPLOYER, "--broadcast", contract,
              *(["--constructor-args", *ctor] if ctor else []))
    return next(l.split()[-1] for l in out.splitlines() if l.startswith("Deployed to:"))


def main():
    os.makedirs(OUT, exist_ok=True)
    plan = json.loads(run("cargo", "run", "-q", "--release", "--manifest-path", "executor-tests/Cargo.toml", "--bin", "vectors"))
    anvil = subprocess.Popen(["anvil", "--port", "8547", "--chain-id", "31337", "--hardfork", "cancun", "--silent"])
    try:
        for _ in range(50):
            try:
                cast("chain-id")
                break
            except SystemExit:
                time.sleep(0.2)
        run("forge", "build", "--root", "vault")
        deployer = run("cast", "wallet", "address", DEPLOYER)
        alice = run("cast", "wallet", "address", ALICE)
        bob = run("cast", "wallet", "address", BOB)
        carol = run("cast", "wallet", "address", CAROL)
        verifier = create("test/Mocks.sol:MockVerifier")
        usdt = create("test/Mocks.sol:MockUSDT")
        vault = create("src/WrapVault.sol:WrapVault", plan["pairing"], verifier, "86400", deployer, deployer)

        send(DEPLOYER, vault, "registerToken(address,uint8,uint256[],uint128,uint128)", ETH, "18",
             "[100000000000000000,1000000000000000000]", "100000000000000000000", "100000000000000000000")
        send(DEPLOYER, vault, "registerToken(address,uint8,uint256[],uint128,uint128)", usdt, "6",
             "[100000000,1000000000]", "10000000000", "10000000000")
        send(DEPLOYER, usdt, "mint(address,uint256)", alice, "1000000000000")
        send(ALICE, usdt, "approve(address,uint256)", vault, "1000000000000")

        roots = [cast("call", vault, "root()(bytes32)")]
        for d in plan["deposits"]:
            key = {"alice": ALICE, "bob": BOB, "carol": CAROL}[d["from"]]
            token = ETH if d["token"] == "eth" else usdt
            send(key, vault, "deposit(address,uint256,bytes32)", token, d["amount"], d["commitment"],
                 value=d["amount"] if token == ETH else None)
            roots.append(cast("call", vault, "root()(bytes32)"))

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

        block = json.loads(cast("rpc", "eth_getBlockByNumber", "latest", "false"))
        number = block["number"]

        def proof(root):
            slot = run("cast", "index", "bytes32", root, "0")
            return {"root": root, "slot": slot,
                    "proof": {"result": json.loads(cast("rpc", "eth_getProof", vault, json.dumps([slot]), number))}}

        proofs = [proof(r) for r in roots]
        for p in proofs:
            assert int(p["proof"]["result"]["storageProof"][0]["value"], 16) == 1, "every root the tree had is known"
        # The deposit never made: its leaf, and the root of a tree holding only it.
        leaf = cast("call", vault, "leafOf(address,uint256,bytes32)(bytes32)", ETH, "1000000000000000000", plan["phantom"])
        node = bytes.fromhex(leaf[2:])
        zero = bytes(32)
        for _ in range(24):
            node = hashlib.sha256(node + zero).digest()
            zero = hashlib.sha256(zero + zero).digest()
        unknown = proof("0x" + node.hex())
        unknown["leaf"] = leaf
        assert int(unknown["proof"]["result"]["storageProof"][0]["value"], 16) == 0

        dump = lambda name, obj: json.dump(obj, open(os.path.join(OUT, name), "w"), indent=1)
        dump("deployment.json", {"chain_id": 31337, "vault": vault, "usdt": usdt, "verifier": verifier,
                                 "pairing": plan["pairing"], "admin": deployer, "alice": alice, "bob": bob, "carol": carol,
                                 "block": int(number, 16)})
        dump("deposits.json", deposits)
        dump("block.json", {"result": block})
        dump("proofs.json", proofs)
        dump("proof-unknown.json", unknown)
        print(f"vault {vault}, {len(deposits)} deposits, block {int(number, 16)}, root {roots[-1]}")
    finally:
        anvil.terminate()


if __name__ == "__main__":
    main()

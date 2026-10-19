#!/usr/bin/env python3
"""The ZkReleaseInput of the Foundry fixture (vault/test/ZkBurnVerifier.t.sol).

The test's LEZ burn tree, the same as CommitteeBurnVerifier.t.sol's: three
burns under PAIRING = keccak256("test pairing"); leaf 1 is carol's 1 ETH with a
relayer fee of 0.01 ETH. The vault sits at VAULT on chain 31337 (the test puts
it there with deployCodeTo) and the relayer is 0x7E1A. Prints the input JSON.
Pure hashing (the formulas of core/src/lib.rs); the guest recomputes them all.
"""
import hashlib
import json
import subprocess
import sys

VAULT = "0x000000000000000000000000000000000000fa22"
CHAIN = 31337
ETH = "0x" + "00" * 20
ONE, FEE = 10**18, 10**16
ALICE, CAROL, RELAYER = "0x" + "00" * 19 + "a1", "0x" + "00" * 19 + "ca", "0x" + "00" * 18 + "7e1a"
CAROL_SECRET = (0xCA5EC2E7).to_bytes(32, "big")


def sha(*parts):
    return hashlib.sha256(b"".join(parts)).digest()


def tag(s):
    return s.encode().ljust(32, b"\0")


def word_addr(a):
    return bytes.fromhex(a[2:]).rjust(32, b"\0")


def keccak(b):
    out = subprocess.run(["cast", "keccak", "0x" + b.hex()], capture_output=True, text=True, check=True).stdout.strip()
    return bytes.fromhex(out[2:])


PAIRING = keccak(b"test pairing")


def ec(recipient, fee, secret):
    return sha(tag("rfp022/eth-commit/v1"), word_addr(recipient), fee.to_bytes(32, "big"), secret)


def leaf(token, amount, e):
    return sha(b"rfp022/burn/v1", PAIRING, bytes.fromhex(token[2:]), amount.to_bytes(16, "little"), e)


def path(leaves, index, depth=24):
    z, layer, out = b"\0" * 32, list(leaves), []
    for _ in range(depth):
        sib = index ^ 1
        out.append(layer[sib] if sib < len(layer) else z)
        layer = [sha(layer[i], layer[i + 1] if i + 1 < len(layer) else z) for i in range(0, len(layer), 2)]
        index >>= 1
        z = sha(z, z)
    return out


leaves = [leaf(ETH, ONE, ec(ALICE, 0, (1).to_bytes(32, "big"))),
          leaf(ETH, ONE, ec(CAROL, FEE, CAROL_SECRET)),
          leaf(ETH, ONE // 10, ec(ALICE, 0, (2).to_bytes(32, "big")))]
json.dump({"chain_id": CHAIN, "vault": VAULT, "pairing": "0x" + PAIRING.hex(), "relayer": RELAYER, "token": ETH,
           "amount": str(ONE), "recipient": CAROL, "fee": str(FEE), "secret": "0x" + CAROL_SECRET.hex(), "index": 1,
           "path": ["0x" + p.hex() for p in path(leaves, 1)]}, sys.stdout, indent=1)
print()

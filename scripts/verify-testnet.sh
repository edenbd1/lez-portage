#!/usr/bin/env bash
# Re-check the public inbound run (evidence/testnet/) from its manifest, with
# nothing but JSON-RPC: the LEZ side with scripts/verify-onchain.sh against the
# public LEZ testnet (every transaction resolves to its block and hashes to its
# bytes, each one charged what its verdict implies, the private claim carries
# no signature and not its destination, the deployed programs' ImageIDs, the
# final state of every account the run wrote), then the Sepolia side and what
# ties it to LEZ with scripts/verify-sepolia.py.
#
#   scripts/verify-testnet.sh [--rpc https://testnet.lez.logos.co] [--eth-rpc https://ethereum-sepolia-rpc.publicnode.com]
#
# Needs python3 with pycryptodome (Keccak-256).
set -uo pipefail
cd "$(dirname "$0")/.."
RPC="https://testnet.lez.logos.co"
ETH_RPC="https://ethereum-sepolia-rpc.publicnode.com"
MANIFEST="evidence/testnet/run.tsv"
while [ $# -gt 0 ]; do
  case "$1" in
    --rpc) RPC="${2:?}"; shift 2 ;;
    --eth-rpc) ETH_RPC="${2:?}"; shift 2 ;;
    --manifest) MANIFEST="${2:?}"; shift 2 ;;
    *) echo "usage: $0 [--rpc URL] [--eth-rpc URL] [--manifest FILE]" >&2; exit 2 ;;
  esac
done
fail=0
echo "== LEZ ($RPC)"
scripts/verify-onchain.sh --manifest "$MANIFEST" --rpc "$RPC" || fail=1
echo "== Sepolia ($ETH_RPC) and the link to LEZ"
python3 scripts/verify-sepolia.py --manifest "$MANIFEST" --eth-rpc "$ETH_RPC" --rpc "$RPC" || fail=1
echo "== $( [ $fail = 0 ] && echo PASS || echo FAIL )"
exit $fail

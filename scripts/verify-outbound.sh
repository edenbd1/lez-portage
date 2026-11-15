#!/usr/bin/env bash
# Re-check the public outbound run (evidence/testnet-outbound/) from its
# manifest, with public JSON-RPC only: scripts/verify-onchain.sh against the
# LEZ testnet and Sepolia (the burns resolve and hash to their bytes, each one
# charged what its verdict implies; every release attempt's receipt, the
# Release event, each refusal's revert replayed at the block before; the burn
# tree's root now; the final nullifiers and balances), then
# scripts/verify-outbound.py (the release's opening and attestation decoded
# from its calldata and recomputed, the LEZ burn it pays out, the hash-linked
# LEZ blocks up to the final one, the payout, the burn tree rebuilt from the
# released openings, and the members' saved Bedrock blocks).
#
#   scripts/verify-outbound.sh [--rpc URL] [--eth-rpc URL] [--manifest FILE] [--bedrock-tools BIN]
#
# The Ethereum endpoint must serve historical state (the refusals are replayed
# at their block, the payout is read at the block before): the default
# rpc.sepolia.ethpandaops.io does; ethereum-sepolia-rpc.publicnode.com prunes it.
# Needs python3 with eth_account and pycryptodome.
set -uo pipefail
cd "$(dirname "$0")/.."
RPC="https://testnet.lez.logos.co"
ETH_RPC="https://rpc.sepolia.ethpandaops.io"
MANIFEST="evidence/testnet-outbound/run.tsv"
TOOLS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --rpc) RPC="${2:?}"; shift 2 ;;
    --eth-rpc) ETH_RPC="${2:?}"; shift 2 ;;
    --manifest) MANIFEST="${2:?}"; shift 2 ;;
    --bedrock-tools) TOOLS=(--bedrock-tools "${2:?}"); shift 2 ;;
    *) echo "usage: $0 [--rpc URL] [--eth-rpc URL] [--manifest FILE] [--bedrock-tools BIN]" >&2; exit 2 ;;
  esac
done
BDIR="$(dirname "$MANIFEST")/bedrock"
BED=(); [ -d "$BDIR" ] && BED=(--bedrock-dir "$BDIR")
fail=0
echo "== LEZ ($RPC) and Sepolia ($ETH_RPC): transactions, receipts, final state"
scripts/verify-onchain.sh --manifest "$MANIFEST" --rpc "$RPC" --eth-rpc "$ETH_RPC" || fail=1
echo "== the releases against the burns they pay out"
python3 -W ignore scripts/verify-outbound.py --manifest "$MANIFEST" --rpc "$RPC" --eth-rpc "$ETH_RPC" \
  ${BED[@]+"${BED[@]}"} ${TOOLS[@]+"${TOOLS[@]}"} || fail=1
echo "== $( [ $fail = 0 ] && echo PASS || echo FAIL )"
exit $fail

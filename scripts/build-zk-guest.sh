#!/usr/bin/env bash
# Build the zero-knowledge release guest (zk/burn_membership) in RISC Zero's
# pinned builder image (r0.1.91.1, the one wrap_bridge.bin is built with) and
# copy it to artifacts/zk/burn_membership.bin, the binary the prover runs and
# whose image id ZkBurnVerifier is pinned to. Reproducible: CI and a second
# build must produce the same bytes (cmp).
#   scripts/build-zk-guest.sh
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=artifacts/zk/burn_membership.bin
mkdir -p "$(dirname "$OUT")"
RISC0_DOCKER_CONTAINER_TAG=r0.1.91.1 cargo risczero build --manifest-path zk/burn_membership/Cargo.toml
cp zk/burn_membership/target/riscv32im-risc0-zkvm-elf/docker/burn_membership.bin "$OUT"
echo "$(wc -c < "$OUT" | tr -d ' ') bytes, sha256 $(shasum -a 256 "$OUT" | cut -d' ' -f1)"

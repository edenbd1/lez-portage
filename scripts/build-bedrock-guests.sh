#!/usr/bin/env bash
# Build the Bedrock-anchored release's two guests (zk/bedrock_anchor, then
# zk/bedrock_release) in RISC Zero's pinned builder image (r0.1.91.1, the one
# wrap_bridge.bin and burn_membership.bin are built with) and copy them to
# artifacts/zk/, the binaries the prover runs and whose image ids
# ZkBedrockBurnVerifier is pinned to. Reproducible: a second build must give the
# same bytes (cmp).
#   scripts/build-bedrock-guests.sh
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p artifacts/zk
for g in bedrock_anchor bedrock_release; do
  RISC0_DOCKER_CONTAINER_TAG=r0.1.91.1 cargo risczero build --manifest-path "zk/$g/Cargo.toml"
  cp "zk/$g/target/riscv32im-risc0-zkvm-elf/docker/$g.bin" "artifacts/zk/$g.bin"
  echo "$g: $(wc -c < "artifacts/zk/$g.bin" | tr -d ' ') bytes, sha256 $(shasum -a 256 "artifacts/zk/$g.bin" | cut -d' ' -f1)"
done

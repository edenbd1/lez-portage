#!/usr/bin/env bash
# Build the wrap_bridge LEZ program and pack it as the R0BF binary LEZ loads.
#   scripts/build-guest.sh            local RISC Zero toolchains (rzup: rust, cpp)
#   DOCKER=1 scripts/build-guest.sh   RISC Zero's pinned builder (reproducible)
# The other two programs the tests run are committed as built elsewhere:
#   artifacts/programs/v0.3/sextant_lc.bin  edenbd1/lez-sextant main 8153272 (ImageID fa229edd...)
#   artifacts/programs/v0.3/token.bin       logos-execution-zone v0.3.0 db66590a, artifacts/lez/programs/token.bin
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=artifacts/programs/v0.3/wrap_bridge.bin
if [ "${DOCKER:-0}" = 1 ]; then
  RISC0_DOCKER_CONTAINER_TAG=r0.1.91.1 cargo risczero build --manifest-path programs/wrap_bridge/Cargo.toml
  cp programs/wrap_bridge/target/riscv32im-risc0-zkvm-elf/docker/wrap_bridge.bin "$OUT"
  cargo run -q --release --manifest-path executor-tests/Cargo.toml --bin image_id -- "$OUT"
  exit 0
fi
FLAGS=$(printf '%s\x1f' -C passes=lower-atomic -C link-arg=-Ttext=0x00200800 -C link-arg=--fatal-warnings \
  -C panic=abort --cfg 'getrandom_backend="custom"')
FLAGS=${FLAGS%$'\x1f'}
TC="${RISC0_RUST:-$(ls -d "$HOME"/.risc0/toolchains/v1.*-rust-* | sort -V | tail -1)/bin}"
# blst is C: the RISC Zero C toolchain (rzup install cpp), on macOS or Linux.
CC_RV="${RISC0_CC:-$(ls -d "$HOME"/.risc0/toolchains/v*-cpp-*/riscv32im-*/bin/riscv32-unknown-elf-gcc | sort -V | tail -1)}"
(cd programs/wrap_bridge && RUSTC="$TC/rustc" CARGO_ENCODED_RUSTFLAGS="$FLAGS" CC=/no_cc RISC0_FEATURE_bigint2= \
  CC_riscv32im_risc0_zkvm_elf="$CC_RV" CFLAGS_riscv32im_risc0_zkvm_elf="-march=rv32im -nostdlib" \
  cargo build -j "${JOBS:-4}" --release --target riscv32im-risc0-zkvm-elf --bin wrap_bridge)
cargo run -q --release --manifest-path executor-tests/Cargo.toml --bin pack -- \
  programs/wrap_bridge/target/riscv32im-risc0-zkvm-elf/release/wrap_bridge "$OUT"

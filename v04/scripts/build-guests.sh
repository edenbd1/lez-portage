#!/usr/bin/env bash
# Build the three LEZ v0.4.0-rc1 programs the v0.4 spike runs, with the local
# RISC Zero toolchains (rzup: rust, cpp), and pack each as the R0BF binary LEZ
# loads (v04/executor-tests `pack`):
#   v04/artifacts/token.bin           lez-programs refactor/lez-v0.4.x @ 2801ea1 (the canonical
#                                     token program's actor-model port), its lee_core pin moved
#                                     from 616256b9 to the v0.4.0-rc1 tag (the program ABI,
#                                     lee_core::program, is identical at both)
#   v04/artifacts/sextant_attest.bin  v04/programs/sextant_attest
#   v04/artifacts/wrap_bridge.bin     v04/programs/wrap_bridge
#   v04/artifacts/lez_chain_v04.bin   zk/lez_chain_v04 (the chain guest on v0.4 blocks; built on request)
# Not reproducible builds (no Docker): ImageIDs are recorded in evidence/v04-spike, not pinned.
# Local paths are remapped (/cargo, /work, /portage) so none lands in a binary.
#   v04/scripts/build-guests.sh [token|sextant_attest|wrap_bridge ...]
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT=$(pwd)
WORK="${WORK:-${TMPDIR:-/tmp}/portage-v04-guests}"
mkdir -p "$WORK" v04/artifacts
WORK=$(cd "$WORK" && pwd -P)
FLAGS=$(printf '%s\x1f' -C passes=lower-atomic -C link-arg=-Ttext=0x00200800 -C link-arg=--fatal-warnings \
  -C panic=abort --cfg 'getrandom_backend="custom"' \
  "--remap-path-prefix=$HOME/.cargo=/cargo" "--remap-path-prefix=$WORK=/work" "--remap-path-prefix=$ROOT=/portage")
FLAGS=${FLAGS%$'\x1f'}
TC="${RISC0_RUST:-$(ls -d "$HOME"/.risc0/toolchains/v1.*-rust-* | sort -V | tail -1)/bin}"
CC_RV="${RISC0_CC:-$(ls -d "$HOME"/.risc0/toolchains/v*-cpp-*/riscv32im-*/bin/riscv32-unknown-elf-gcc | sort -V | tail -1)}"

guest() { # <crate dir> <bin>
  (cd "$1" && RUSTC="$TC/rustc" CARGO_ENCODED_RUSTFLAGS="$FLAGS" CC=/no_cc RISC0_FEATURE_bigint2= \
    CC_riscv32im_risc0_zkvm_elf="$CC_RV" CFLAGS_riscv32im_risc0_zkvm_elf="-march=rv32im -nostdlib" \
    CARGO_TARGET_DIR="$WORK/target-$2" cargo build -j "${JOBS:-4}" --release --target riscv32im-risc0-zkvm-elf --bin "$2")
  cargo run -q --release -j "${JOBS:-4}" --manifest-path v04/executor-tests/Cargo.toml --bin pack -- \
    "$WORK/target-$2/riscv32im-risc0-zkvm-elf/release/$2" "v04/artifacts/$2.bin"
}

token() {
  local src="$WORK/lez-programs"
  if [ ! -d "$src" ]; then
    git clone -q https://github.com/logos-blockchain/lez-programs.git "$src"
  fi
  git -C "$src" fetch -q origin 2801ea19ea7fed003295eaa89f7c0dc8f01cece2 2>/dev/null || git -C "$src" fetch -q origin refactor/lez-v0.4.x
  git -C "$src" checkout -q -f 2801ea19ea7fed003295eaa89f7c0dc8f01cece2
  for f in programs/token/Cargo.toml programs/token/core/Cargo.toml programs/token/methods/guest/Cargo.toml; do
    sed -i.bak 's/rev = "616256b9c9efd2c28d4f7683b7b5d2348df7314e"/tag = "v0.4.0-rc1"/' "$src/$f"
  done
  rm -f "$src/programs/token/methods/guest/Cargo.lock"
  guest "$src/programs/token/methods/guest" token
}

for t in "${@:-token sextant_attest wrap_bridge}"; do
  for x in $t; do
    case "$x" in
      token) token ;;
      sextant_attest | wrap_bridge) guest "$ROOT/v04/programs/$x" "$x" ;;
      lez_chain_v04) guest "$ROOT/zk/lez_chain_v04" "$x" ;;
      *) echo "unknown guest $x" >&2; exit 2 ;;
    esac
  done
done

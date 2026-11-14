#!/usr/bin/env bash
# Build the LEZ v0.4.0-rc3 programs the v0.4 port runs and the v0.4 chain guest:
#   v04/artifacts/token.bin           lez-programs refactor/lez-v0.4.x @ 2801ea1 (the canonical
#                                     token program's actor-model port; the branch's later commits
#                                     change only comments in programs/token), its lee_core pin
#                                     moved from 616256b9 to the v0.4.0-rc3 tag, built with the
#                                     lockfile committed here (v04/locks/token-guest.Cargo.lock)
#   v04/artifacts/wrap_bridge.bin     v04/programs/wrap_bridge
#   v04/artifacts/lez_chain_v04.bin   zk/lez_chain_v04 (the chain guest on v0.4 blocks)
#   v04/artifacts/sextant_lc.bin      Sextant's light client on v0.4: lez-sextant branch `v0.4`,
#                                     v04/programs/sextant, built by that repository's own
#                                     v04/scripts/build-guests.sh (cloned at SEXTANT_REV)
#
#   DOCKER=1 v04/scripts/build-guests.sh [token wrap_bridge lez_chain_v04 sextant]
#       RISC Zero's pinned builder image (r0.1.91.1, the one lez-sextant uses): reproducible,
#       the committed binaries; CI's reproduce-v04 job rebuilds them and cmp's.
#   v04/scripts/build-guests.sh [...]
#       the local RISC Zero toolchains (rzup: rust, cpp), local paths remapped (/cargo, /work,
#       /portage) so none lands in a binary; for iterating only, the bytes differ from Docker's.
# Default: token wrap_bridge. Each build prints the binary's ImageID.
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
# The local toolchains (not needed by the Docker build).
TC="${RISC0_RUST:-$(ls -d "$HOME"/.risc0/toolchains/v1.*-rust-* 2>/dev/null | sort -V | tail -1 || true)/bin}"
CC_RV="${RISC0_CC:-$(ls -d "$HOME"/.risc0/toolchains/v*-cpp-*/riscv32im-*/bin/riscv32-unknown-elf-gcc 2>/dev/null | sort -V | tail -1 || true)}"

docker_guest() { # <crate dir, relative to the repository> <bin>
  RISC0_DOCKER_CONTAINER_TAG=r0.1.91.1 cargo risczero build --manifest-path "$1/Cargo.toml"
  cp "$1/target/riscv32im-risc0-zkvm-elf/docker/$2.bin" "v04/artifacts/$2.bin"
  image_id "v04/artifacts/$2.bin"
}

image_id() {
  [ "${NO_IMAGE_ID:-0}" = 1 ] && return 0
  echo "$(basename "$1" .bin): $(cargo run -q --release -j "${JOBS:-4}" --manifest-path v04/executor-tests/Cargo.toml --bin image_id -- "$1")"
}

guest() { # <crate dir> <bin>
  (cd "$1" && RUSTC="$TC/rustc" CARGO_ENCODED_RUSTFLAGS="$FLAGS" CC=/no_cc RISC0_FEATURE_bigint2= \
    CC_riscv32im_risc0_zkvm_elf="$CC_RV" CFLAGS_riscv32im_risc0_zkvm_elf="-march=rv32im -nostdlib" \
    CARGO_TARGET_DIR="$WORK/target-$2" cargo build -j "${JOBS:-4}" --release --target riscv32im-risc0-zkvm-elf --bin "$2")
  cargo run -q --release -j "${JOBS:-4}" --manifest-path v04/executor-tests/Cargo.toml --bin pack -- \
    "$WORK/target-$2/riscv32im-risc0-zkvm-elf/release/$2" "v04/artifacts/$2.bin"
  image_id "v04/artifacts/$2.bin"
}

own() { # <crate dir, relative> <bin>
  if [ "${DOCKER:-0}" = 1 ]; then docker_guest "$1" "$2"; else guest "$ROOT/$1" "$2"; fi
}

token() {
  if [ "${DOCKER:-0}" = 1 ]; then
    token_src "$ROOT/v04/third_party/lez-programs"
    docker_guest v04/third_party/lez-programs/programs/token/methods/guest token
  else
    token_src "$WORK/lez-programs"
    guest "$WORK/lez-programs/programs/token/methods/guest" token
  fi
}

# The Docker build's sources must lie inside its context (this repository):
# lez-programs is checked out under v04/third_party (ignored by git).
token_src() {
  local src="$1"
  if [ ! -d "$src/.git" ]; then
    rm -rf "$src"; mkdir -p "$(dirname "$src")"
    git clone -q https://github.com/logos-blockchain/lez-programs.git "$src"
  fi
  git -C "$src" fetch -q origin 2801ea19ea7fed003295eaa89f7c0dc8f01cece2 2>/dev/null || git -C "$src" fetch -q origin refactor/lez-v0.4.x
  git -C "$src" checkout -q -f 2801ea19ea7fed003295eaa89f7c0dc8f01cece2
  git -C "$src" clean -q -fdx -e target
  for f in programs/token/Cargo.toml programs/token/core/Cargo.toml programs/token/methods/guest/Cargo.toml; do
    sed -i.bak -e 's/rev = "616256b9c9efd2c28d4f7683b7b5d2348df7314e"/tag = "v0.4.0-rc3"/' -e 's/tag = "v0.4.0-rc1"/tag = "v0.4.0-rc3"/' "$src/$f"
    rm -f "$src/$f.bak"
  done
  cp v04/locks/token-guest.Cargo.lock "$src/programs/token/methods/guest/Cargo.lock"
}

sextant() {
  local src="$WORK/lez-sextant"
  [ -d "$src" ] || git clone -q https://github.com/edenbd1/lez-sextant.git "$src"
  git -C "$src" fetch -q origin v0.4
  git -C "$src" checkout -q -f "${SEXTANT_REV:-1e55629a80117bc9e8caed35439df86ec6433dff}"
  "$src/v04/scripts/build-guests.sh"
  cp "$src/v04/artifacts/sextant_lc.bin" v04/artifacts/sextant_lc.bin
  image_id v04/artifacts/sextant_lc.bin
}

for t in "${@:-token wrap_bridge}"; do
  for x in $t; do
    case "$x" in
      token) token ;;
      sextant) sextant ;;
      wrap_bridge) own v04/programs/wrap_bridge wrap_bridge ;;
      lez_chain_v04) own zk/lez_chain_v04 lez_chain_v04 ;;
      *) echo "unknown guest $x" >&2; exit 2 ;;
    esac
  done
done

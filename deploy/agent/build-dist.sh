#!/usr/bin/env bash
# Builds the pnex-agent distribution served by pnex-server at
# /api/v1/agent/download/{target} (D95):
#   Linux x86_64 / aarch64 / armv7 — static musl (any distro, Raspberry Pi)
#   Windows x86_64                 — mingw
# plus SHA256SUMS. Usage: deploy/agent/build-dist.sh [OUT_DIR]
#
# Linux targets link through zig (cargo-zigbuild): no per-target C cross
# toolchain for ring. zig is fetched (pinned sha256) when not on PATH.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-$ROOT/target/agent-dist}"
ZIG_VERSION=0.14.1
ZIG_SHA256=24aeeec8af16c381934a6cd7d95c807a8cb2cf7df9fa40d359aa884195c4716c

cd "$ROOT"
if ! command -v zig >/dev/null 2>&1; then
  ZIG_HOME="$ROOT/target/tools/zig-x86_64-linux-$ZIG_VERSION"
  if [ ! -x "$ZIG_HOME/zig" ]; then
    [ "$(uname -m)" = "x86_64" ] || { echo "zig not found: install zig $ZIG_VERSION" >&2; exit 1; }
    mkdir -p "$ROOT/target/tools"
    TARBALL="$ROOT/target/tools/zig-$ZIG_VERSION.tar.xz"
    curl -fsSL "https://ziglang.org/download/$ZIG_VERSION/zig-x86_64-linux-$ZIG_VERSION.tar.xz" -o "$TARBALL"
    echo "$ZIG_SHA256  $TARBALL" | sha256sum -c -
    tar -xJf "$TARBALL" -C "$ROOT/target/tools"
    rm -f "$TARBALL"
  fi
  export PATH="$ZIG_HOME:$PATH"
fi
command -v cargo-zigbuild >/dev/null 2>&1 || { echo "cargo-zigbuild missing: cargo install --locked cargo-zigbuild" >&2; exit 1; }

LINUX_TARGETS=(
  "x86_64-unknown-linux-musl:x86_64-linux"
  "aarch64-unknown-linux-musl:aarch64-linux"
  "armv7-unknown-linux-musleabihf:armv7-linux"
)
if command -v rustup >/dev/null 2>&1; then
  rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl \
    armv7-unknown-linux-musleabihf x86_64-pc-windows-gnu >/dev/null
fi

mkdir -p "$OUT"
for entry in "${LINUX_TARGETS[@]}"; do
  triple="${entry%%:*}"; id="${entry##*:}"
  cargo zigbuild --release --locked -p pnex-edge-agent --bin pnex-agent --target "$triple"
  install -m 0755 "target/$triple/release/pnex-agent" "$OUT/pnex-agent-$id"
done

CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
  cargo build --release --locked -p pnex-edge-agent --bin pnex-agent --target x86_64-pc-windows-gnu
install -m 0644 "target/x86_64-pc-windows-gnu/release/pnex-agent.exe" "$OUT/pnex-agent-x86_64-windows.exe"

(cd "$OUT" && sha256sum pnex-agent-* > SHA256SUMS)
echo "pnex-agent distribution in $OUT:"
cat "$OUT/SHA256SUMS"

#!/usr/bin/env bash
# Builds the PNeX browser agent as an unpacked extension into $1 (the
# server zips it on download, D-distribution in extension-collector.md).
# wasm-bindgen must match the crate version pinned in Cargo.lock: taken
# from PATH, else from dx's tool cache (dx downloads it for the frontend).
set -euo pipefail
out=${1:?usage: build-dist.sh <out-dir>}
root=$(cd "$(dirname "$0")/../.." && pwd)
ver=$(awk '$0 == "name = \"wasm-bindgen\"" { getline; gsub(/version = |"/, ""); print; exit }' "$root/Cargo.lock")
wb=$(command -v wasm-bindgen || true)
if [ -z "$wb" ] || ! "$wb" --version | grep -q " $ver"; then
  wb=$(find "$HOME/.local/share" -path "*wasm-bindgen-$ver/wasm-bindgen" -type f -print -quit 2>/dev/null || true)
fi
if [ ! -x "${wb:-}" ]; then
  echo "wasm-bindgen $ver not found: cargo install wasm-bindgen-cli --version $ver" >&2
  exit 1
fi
# getrandom 0.3 (via snow) needs its browser backend chosen at compile time.
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' \
  cargo build --manifest-path "$root/Cargo.toml" -p pnex-extension --release --target wasm32-unknown-unknown
rm -rf "$out"
mkdir -p "$out"
cp -r "$root/crates/pnex-extension/extension/." "$out/"
"$wb" --target web --no-typescript --out-dir "$out/pkg" \
  "${CARGO_TARGET_DIR:-$root/target}/wasm32-unknown-unknown/release/pnex_extension.wasm"

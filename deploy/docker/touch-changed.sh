#!/usr/bin/env bash
# Content-based freshness for cargo inside an image build (O33).
#
# target/ lives in a BuildKit cache mount and cargo judges freshness by
# mtime, while `COPY . .` keeps the mtimes of the build context. A source
# edited while an earlier image was building can look older than its cached
# artefact and is never recompiled (stale binary, missing migration).
#
# This script hashes the source tree, touches every file whose content
# differs from the manifest of the last successful build, and writes the
# new manifest next to it as <manifest>.new. The caller promotes it after
# cargo succeeds:
#
#   deploy/docker/touch-changed.sh target/.src-manifest
#   cargo build …
#   mv target/.src-manifest.new target/.src-manifest
#
# A failed build keeps the old manifest, so the next one touches again.
set -euo pipefail

manifest="$1"
new="$manifest.new"

find crates vendor Cargo.toml Cargo.lock \
    \( -name target -o -name node_modules -o -name .git \) -prune -o \
    -type f -print0 \
  | LC_ALL=C sort -z \
  | xargs -0 sha256sum > "$new"

if [ -f "$manifest" ]; then
  # Lines (hash + path) absent from the previous manifest: new or changed.
  changed="$(LC_ALL=C comm -13 <(LC_ALL=C sort "$manifest") <(LC_ALL=C sort "$new") | cut -c67-)"
else
  # No manifest yet: the cache cannot be trusted, rebuild every local crate.
  changed="$(cut -c67- "$new")"
fi

if [ -n "$changed" ]; then
  printf '%s\n' "$changed" | tr '\n' '\0' | xargs -0 touch
  echo "touch-changed: $(printf '%s\n' "$changed" | wc -l) file(s) marked fresh"
else
  echo "touch-changed: sources unchanged"
fi

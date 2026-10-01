#!/bin/sh
# PNeX edge agent installer (Linux) — served by pnex-server at
# /api/v1/agent/install.sh (D95, docs/architecture/edge-agent.md).
#
# Usage (the UI prints the exact one-liner):
#   curl -fsSL --cacert pnex-ca.pem https://HOST/api/v1/agent/install.sh \
#     | sudo sh -s -- --server https://HOST --enroll ABCD-EFGH-JKMN [--ca-sha256 HEX]
#
# Extra flags are forwarded to `pnex-agent install` (--listen, --allow,
# --user, --no-service).
set -eu

SERVER=""
CODE=""
CA_SHA=""
EXTRA=""
while [ $# -gt 0 ]; do
  case "$1" in
    --server) SERVER="$2"; shift 2 ;;
    --enroll) CODE="$2"; shift 2 ;;
    --ca-sha256) CA_SHA="$2"; shift 2 ;;
    *) EXTRA="$EXTRA $1"; shift ;;
  esac
done

die() { echo "pnex-agent install: $*" >&2; exit 1; }
[ -n "$SERVER" ] || die "--server is required"
[ -n "$CODE" ] || die "--enroll is required"
SERVER="${SERVER%/}"

[ "$(uname -s)" = "Linux" ] || die "this script supports Linux only (use install.ps1 on Windows)"
case "$(uname -m)" in
  x86_64|amd64) TARGET="x86_64-linux" ;;
  aarch64|arm64) TARGET="aarch64-linux" ;;
  armv7l|armv7|armhf) TARGET="armv7-linux" ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
CURL="curl -fsSL"
if [ -n "$CA_SHA" ]; then
  # Trust on first use, pinned by the fingerprint printed in the UI.
  curl -fsSLk "$SERVER/api/v1/meta/ca" -o "$TMP/ca.pem" || die "cannot fetch the server CA"
  GOT="$(sha256_of "$TMP/ca.pem")"
  [ "$GOT" = "$CA_SHA" ] || die "CA fingerprint mismatch (expected $CA_SHA, got $GOT)"
  CURL="curl -fsSL --cacert $TMP/ca.pem"
fi

FILE="pnex-agent-$TARGET"
echo "Downloading $FILE from $SERVER ..."
$CURL "$SERVER/api/v1/agent/download/$TARGET" -o "$TMP/$FILE" || die "download failed (is this platform shipped by the server?)"
$CURL "$SERVER/api/v1/agent/download/SHA256SUMS" -o "$TMP/SHA256SUMS" || die "checksum list download failed"
WANT="$(grep " $FILE\$" "$TMP/SHA256SUMS" | cut -d' ' -f1)"
[ -n "$WANT" ] || die "no checksum published for $FILE"
[ "$(sha256_of "$TMP/$FILE")" = "$WANT" ] || die "binary checksum mismatch"

if [ "$(id -u)" = "0" ]; then BIN_DIR="/usr/local/bin"; else BIN_DIR="$HOME/.local/bin"; mkdir -p "$BIN_DIR"; fi
install -m 0755 "$TMP/$FILE" "$BIN_DIR/pnex-agent"
echo "Installed $BIN_DIR/pnex-agent"

if [ -n "$CA_SHA" ]; then
  # shellcheck disable=SC2086
  exec "$BIN_DIR/pnex-agent" install --server "$SERVER" --enroll "$CODE" --ca-sha256 "$CA_SHA" $EXTRA
else
  # shellcheck disable=SC2086
  exec "$BIN_DIR/pnex-agent" install --server "$SERVER" --enroll "$CODE" $EXTRA
fi

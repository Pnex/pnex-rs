#!/usr/bin/env bash
# TLS edge (D70) — `task edge:trust`: trust the local root CA on THIS Linux
# machine (system store + the NSS databases used by Chrome/Chromium and
# Firefox). Other clients: download deploy/edge/pki-data/ca.pem and import
# it (Android: Settings › Security › Install a certificate › CA).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CA="$ROOT/deploy/edge/pki-data/ca.pem"
NAME="PNeX Local CA"
[[ -s $CA ]] || { echo "ERROR: $CA missing — run task edge:up first"; exit 1; }

# System store (curl, Rust native-tls/rustls-native-certs, python…).
sudo install -m 644 "$CA" /usr/local/share/ca-certificates/pnex-local-ca.crt
sudo update-ca-certificates >/dev/null
echo "  ✓ system trust store"

# Browsers use NSS: ~/.pki/nssdb (Chrome) and each Firefox profile.
if ! command -v certutil >/dev/null; then
    echo "  • certutil missing (apt install libnss3-tools) — browsers not updated"
    exit 0
fi
dbs=("$HOME/.pki/nssdb")
for p in "$HOME"/.mozilla/firefox/*.default* "$HOME"/snap/firefox/common/.mozilla/firefox/*.default*; do
    [[ -f $p/cert9.db ]] && dbs+=("$p")
done
for db in "${dbs[@]}"; do
    mkdir -p "$db"
    [[ -f $db/cert9.db ]] || certutil -N -d "sql:$db" --empty-password
    certutil -D -d "sql:$db" -n "$NAME" >/dev/null 2>&1 || true
    certutil -A -d "sql:$db" -n "$NAME" -t "C,," -i "$CA"
    echo "  ✓ NSS $db"
done
echo "Restart the browsers to pick up the new root."

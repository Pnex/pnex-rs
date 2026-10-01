#!/bin/sh
# TLS edge PKI bootstrap (D70) — runs as the one-shot `edge-pki` service on
# every `task edge:up`. Idempotent.
#
# Layout (bind mount deploy/edge/pki-data → /pki, gitignored):
#   ca/ca.key, ca/ca.pem   local root CA (ECDSA P-256, 10 years) — generated
#                          ONCE; firmware pins it (PNEX_CA_CERT), clients
#                          import it. Never regenerated automatically.
#   ca.pem                 public copy of the root, for download/import.
#   current/{fullchain,privkey}.pem   what nginx serves.
#   current/source         local | placeholder | acme
#   current/sans           SAN list the local leaf was issued for.
#   device-ca.pem          the root firmware builds pin (PNEX_CA_CERT):
#                          local CA in local mode, ISRG Root X1 in cloud.
set -eu

PKI=/pki
DOMAIN="${PNEX_DOMAIN:?PNEX_DOMAIN is required}"
MODE="${PNEX_EDGE_MODE:-local}"
CUR="$PKI/current"
mkdir -p "$PKI/ca" "$CUR"

# Atomic install: nginx's watcher reloads on fullchain change, so the key
# lands first and the chain last.
install_pair() { # $1=key $2=cert $3=source
    cp "$1" "$CUR/privkey.pem.tmp" && mv "$CUR/privkey.pem.tmp" "$CUR/privkey.pem"
    chmod 600 "$CUR/privkey.pem"
    cp "$2" "$CUR/fullchain.pem.tmp" && mv "$CUR/fullchain.pem.tmp" "$CUR/fullchain.pem"
    echo "$3" > "$CUR/source"
}

if [ "$MODE" = "cloud" ]; then
    # certbot owns the certificate; only unblock nginx's first start.
    if [ ! -s "$CUR/fullchain.pem" ]; then
        tmp=$(mktemp -d)
        openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
            -keyout "$tmp/key.pem" -out "$tmp/cert.pem" -days 7 \
            -subj "/CN=$DOMAIN" -addext "subjectAltName=DNS:$DOMAIN" 2>/dev/null
        install_pair "$tmp/key.pem" "$tmp/cert.pem" placeholder
        rm -rf "$tmp"
        echo "[pki] cloud mode: placeholder cert installed, waiting for certbot"
    else
        echo "[pki] cloud mode: existing cert kept ($(cat "$CUR/source" 2>/dev/null || echo unknown))"
    fi
    # Devices pin Let's Encrypt's root (the chain nginx serves ends there).
    if wget -q -O "$PKI/device-ca.pem.tmp" https://letsencrypt.org/certs/isrgrootx1.pem \
        && openssl x509 -in "$PKI/device-ca.pem.tmp" -noout 2>/dev/null; then
        mv "$PKI/device-ca.pem.tmp" "$PKI/device-ca.pem"
        chmod 644 "$PKI/device-ca.pem"
        echo "[pki] device CA: ISRG Root X1"
    else
        rm -f "$PKI/device-ca.pem.tmp" "$PKI/device-ca.pem"
        echo "[pki] WARNING: ISRG Root X1 download failed — firmware builds get no CA (ESP32 wss will fail)"
    fi
    exit 0
fi

# ── Local CA (created once) ─────────────────────────────────────────────
if [ ! -s "$PKI/ca/ca.key" ]; then
    openssl ecparam -name prime256v1 -genkey -noout -out "$PKI/ca/ca.key"
    chmod 600 "$PKI/ca/ca.key"
    openssl req -x509 -new -key "$PKI/ca/ca.key" -sha256 -days 3650 \
        -subj "/O=PNeX/CN=PNeX Local CA ($DOMAIN)" \
        -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
        -addext "keyUsage=critical,keyCertSign,cRLSign" \
        -out "$PKI/ca/ca.pem"
    echo "[pki] new local CA generated"
fi
cp "$PKI/ca/ca.pem" "$PKI/ca.pem"
cp "$PKI/ca/ca.pem" "$PKI/device-ca.pem"
chmod 644 "$PKI/ca.pem" "$PKI/ca/ca.pem" "$PKI/device-ca.pem"

# ── Leaf for DOMAIN ─────────────────────────────────────────────────────
SANS="DNS:$DOMAIN,DNS:localhost,IP:127.0.0.1"
[ -n "${PNEX_EXTRA_SANS:-}" ] && SANS="$SANS,$PNEX_EXTRA_SANS"

reason=""
if [ ! -s "$CUR/fullchain.pem" ] || [ ! -s "$CUR/privkey.pem" ]; then
    reason="missing"
elif [ "$(cat "$CUR/source" 2>/dev/null)" != "local" ]; then
    reason="source was not local"
elif [ "$(cat "$CUR/sans" 2>/dev/null)" != "$SANS" ]; then
    reason="SAN list changed"
elif ! openssl x509 -in "$CUR/fullchain.pem" -noout -checkend 2592000 >/dev/null; then
    reason="expires within 30 days"
elif ! openssl verify -CAfile "$PKI/ca/ca.pem" "$CUR/fullchain.pem" >/dev/null 2>&1; then
    reason="not issued by the current CA"
fi

if [ -z "$reason" ]; then
    echo "[pki] leaf for $DOMAIN is valid, nothing to do"
    exit 0
fi

tmp=$(mktemp -d)
openssl ecparam -name prime256v1 -genkey -noout -out "$tmp/key.pem"
openssl req -new -key "$tmp/key.pem" -subj "/CN=$DOMAIN" -out "$tmp/leaf.csr"
cat > "$tmp/ext" <<EOF
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
subjectAltName=$SANS
EOF
# 397 days: the ceiling Apple platforms accept for TLS server certs.
openssl x509 -req -in "$tmp/leaf.csr" -CA "$PKI/ca/ca.pem" -CAkey "$PKI/ca/ca.key" \
    -CAcreateserial -CAserial "$PKI/ca/ca.srl" -days 397 -sha256 \
    -extfile "$tmp/ext" -out "$tmp/cert.pem" 2>/dev/null
install_pair "$tmp/key.pem" "$tmp/cert.pem" local
echo "$SANS" > "$CUR/sans"
rm -rf "$tmp"
echo "[pki] leaf issued for $SANS ($reason)"

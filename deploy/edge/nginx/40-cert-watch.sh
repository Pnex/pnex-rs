#!/bin/sh
# TLS edge (D70) — hot-reload nginx when the served certificate changes
# (local leaf re-issued by edge-pki, or certbot renewal in cloud mode).
# Runs from /docker-entrypoint.d/ before nginx starts; the loop survives
# in the background.
CERT=/pki/current/fullchain.pem

(
    last=$(md5sum "$CERT" 2>/dev/null | cut -d' ' -f1)
    while :; do
        sleep 30
        now=$(md5sum "$CERT" 2>/dev/null | cut -d' ' -f1)
        if [ -n "$now" ] && [ "$now" != "$last" ]; then
            echo "[cert-watch] certificate changed, reloading nginx"
            nginx -s reload && last=$now
        fi
    done
) &

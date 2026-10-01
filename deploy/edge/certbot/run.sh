#!/bin/sh
# TLS edge (D70), cloud mode — Let's Encrypt via HTTP-01 webroot (served by
# edge-nginx on :80). Issues then renews every 12 h; each new certificate
# is copied to /pki/current, where nginx's cert-watch picks it up.
set -u

DOMAIN="${PNEX_DOMAIN:?PNEX_DOMAIN is required}"
EMAIL="${ACME_EMAIL:?ACME_EMAIL is required}"
LIVE="/etc/letsencrypt/live/$DOMAIN"
CUR=/pki/current

# Staging endpoint while testing (avoids Let's Encrypt rate limits).
STAGING=""
[ -n "${ACME_STAGING:-}" ] && STAGING="--staging"

publish() {
    [ -s "$LIVE/fullchain.pem" ] || return 0
    if ! cmp -s "$LIVE/fullchain.pem" "$CUR/fullchain.pem"; then
        cp -L "$LIVE/privkey.pem" "$CUR/privkey.pem.tmp" && mv "$CUR/privkey.pem.tmp" "$CUR/privkey.pem"
        chmod 600 "$CUR/privkey.pem"
        cp -L "$LIVE/fullchain.pem" "$CUR/fullchain.pem.tmp" && mv "$CUR/fullchain.pem.tmp" "$CUR/fullchain.pem"
        echo acme > "$CUR/source"
        echo "[certbot] certificate published for $DOMAIN"
    fi
}

# Let nginx come up with the placeholder before the first challenge.
sleep 5
while :; do
    certbot certonly --webroot -w /var/www/acme -d "$DOMAIN" \
        --email "$EMAIL" --agree-tos --non-interactive --keep-until-expiring \
        $STAGING || echo "[certbot] issuance/renewal failed, retrying in 1 h"
    publish
    if [ -s "$LIVE/fullchain.pem" ]; then sleep 12h; else sleep 1h; fi
done

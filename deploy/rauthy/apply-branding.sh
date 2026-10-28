#!/usr/bin/env bash
# Applies the PNeX branding to the Rauthy clients — light/dark theme + logo +
# favicon for `pnex` (login pages) AND `rauthy` (register/account/admin,
# default fallback of every client without a theme).
#
# Idempotent: replayable after a db:reset / re-bootstrap of the Rauthy DB.
#
# Auth — the Rauthy admin API (0.36) accepts NEITHER Bearer JWT NOR password grant,
# only an API key (header "API-Key") or the Admin UI session:
#   - export RAUTHY_API_KEY='<name>$<secret>'
#   - in dev, the `pnex-branding` key is versioned in bootstrap/api_keys.json
#     (read only on the FIRST initialisation of the DB); if the DB already
#     exists, create the key in the Admin UI → API Keys (group: Clients).
#
# Palette: see branding/theme.json (navy/red X/cyan of the logo + UI blue).
set -euo pipefail

BASE="${RAUTHY_URL:-http://localhost:8080}"
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
THEME_JSON="$DIR/branding/theme.json"
# IdP client logo: wordmark on a rounded white badge — readable on both
# light AND dark themes (the transparent navy wordmark would be invisible in
# dark, the white variant in light). The favicon keeps the X mark
# (readable at 16-32 px, unlike the wordmark).
LOGO="$DIR/branding/logo-card.png"
LOGO_FAVICON="$DIR/../../crates/pnex-frontend/assets/logo-mark.png"

[[ -f $THEME_JSON ]] || { echo "ERROR: theme.json not found ($THEME_JSON)"; exit 1; }
[[ -f $LOGO ]] || { echo "ERROR: logo-card.png not found ($LOGO)"; exit 1; }
[[ -f $LOGO_FAVICON ]] || { echo "ERROR: logo-mark.png not found ($LOGO_FAVICON)"; exit 1; }
[[ -n "${RAUTHY_API_KEY:-}" ]] || {
    echo "ERROR: RAUTHY_API_KEY missing (format '<name>\$<secret>')."
    echo "  - Fresh DB: the 'pnex-branding' key is versioned in bootstrap/api_keys.json."
    echo "  - Existing DB: Admin UI ${BASE}/auth/v1/admin → API Keys → New Key (group Clients),"
    echo "    then generate the secret and export RAUTHY_API_KEY='<name>\$<secret>'."
    exit 1
}
AUTH="Authorization: API-Key $RAUTHY_API_KEY"

put() { # $1 = description, rest = curl args; HTTP code captured without the body
    local desc=$1
    shift
    local code
    code=$(curl -s -o /dev/null -w '%{http_code}' "$@") || code="curl"
    if [[ $code != 2* ]]; then
        echo "ERROR: $desc → HTTP $code (invalid API key or insufficient rights?)"
        exit 1
    fi
    echo "  ✓ $desc"
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

for client in pnex rauthy; do
    echo "Client: $client"
    # The body must carry the path's client_id (Rauthy validation).
    sed "s/\"client_id\": \"pnex\"/\"client_id\": \"$client\"/" \
        "$THEME_JSON" > "$TMP/theme-$client.json"
    put "light/dark theme" \
        -X PUT "$BASE/auth/v1/theme/$client" -H "$AUTH" \
        -H 'Content-Type: application/json' --data @"$TMP/theme-$client.json"

    # Multipart: a single field, content-type required (Rauthy validation).
    put "logo" \
        -X PUT "$BASE/auth/v1/clients/$client/logo" -H "$AUTH" \
        -F "image=@$LOGO;type=image/png"
    put "favicon" \
        -X PUT "$BASE/auth/v1/clients/$client/favicon" -H "$AUTH" \
        -F "image=@$LOGO_FAVICON;type=image/png"
done

echo ""
echo "PNeX branding applied to pnex + rauthy."
echo "NB: the theme CSS is served with a 1-year cache — force a browser refresh."

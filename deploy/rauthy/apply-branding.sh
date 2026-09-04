#!/usr/bin/env bash
# Applique le branding PNeX aux clients Rauthy — thème light/dark + logo +
# favicon pour `pnex` (pages login) ET `rauthy` (register/account/admin,
# fallback par défaut de tout client sans thème).
#
# Idempotent : rejouable après un db:reset / re-bootstrap de la DB Rauthy.
#
# Auth — l'API admin Rauthy (0.36) n'accepte NI Bearer JWT NI password grant,
# uniquement une clé API (header "API-Key") ou la session Admin UI :
#   - export RAUTHY_API_KEY='<nom>$<secret>'
#   - en dev, la clé `pnex-branding` est versionnée dans bootstrap/api_keys.json
#     (lue uniquement à la PREMIÈRE initialisation de la DB) ; si la DB existe
#     déjà, créer la clé dans l'Admin UI → API Keys (groupe : Clients).
#
# Palette : cf. branding/theme.json (navy/rouge X/cyan du logo + bleu UI).
set -euo pipefail

BASE="${RAUTHY_URL:-http://localhost:8080}"
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
THEME_JSON="$DIR/branding/theme.json"
# Logo client IdP : wordmark sur pastille blanche arrondie — lisible sur les
# thèmes clair ET sombre (le wordmark transparent navy serait invisible en
# dark, la variante blanche en light). Le favicon, lui, garde le mark X
# (lisible en 16-32 px, contrairement au wordmark).
LOGO="$DIR/branding/logo-card.png"
LOGO_FAVICON="$DIR/../../crates/pnex-frontend/assets/logo-mark.png"

[[ -f $THEME_JSON ]] || { echo "ERREUR : theme.json introuvable ($THEME_JSON)"; exit 1; }
[[ -f $LOGO ]] || { echo "ERREUR : logo-card.png introuvable ($LOGO)"; exit 1; }
[[ -f $LOGO_FAVICON ]] || { echo "ERREUR : logo-mark.png introuvable ($LOGO_FAVICON)"; exit 1; }
[[ -n "${RAUTHY_API_KEY:-}" ]] || {
    echo "ERREUR : RAUTHY_API_KEY manquante (format '<nom>\$<secret>')."
    echo "  - DB fraîche : la clé 'pnex-branding' est versionnée dans bootstrap/api_keys.json."
    echo "  - DB existante : Admin UI ${BASE}/auth/v1/admin → API Keys → New Key (groupe Clients),"
    echo "    puis générer le secret et exporter RAUTHY_API_KEY='<nom>\$<secret>'."
    exit 1
}
AUTH="Authorization: API-Key $RAUTHY_API_KEY"

put() { # $1 = description, reste = args curl ; code HTTP capté sans le body
    local desc=$1
    shift
    local code
    code=$(curl -s -o /dev/null -w '%{http_code}' "$@") || code="curl"
    if [[ $code != 2* ]]; then
        echo "ERREUR : $desc → HTTP $code (clé API invalide ou droits insuffisants ?)"
        exit 1
    fi
    echo "  ✓ $desc"
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

for client in pnex rauthy; do
    echo "Client : $client"
    # Le body doit porter le client_id du path (validation Rauthy).
    sed "s/\"client_id\": \"pnex\"/\"client_id\": \"$client\"/" \
        "$THEME_JSON" > "$TMP/theme-$client.json"
    put "thème light/dark" \
        -X PUT "$BASE/auth/v1/theme/$client" -H "$AUTH" \
        -H 'Content-Type: application/json' --data @"$TMP/theme-$client.json"

    # Multipart : un seul champ, content-type obligatoire (validation Rauthy).
    put "logo" \
        -X PUT "$BASE/auth/v1/clients/$client/logo" -H "$AUTH" \
        -F "image=@$LOGO;type=image/png"
    put "favicon" \
        -X PUT "$BASE/auth/v1/clients/$client/favicon" -H "$AUTH" \
        -F "image=@$LOGO_FAVICON;type=image/png"
done

echo ""
echo "Branding PNeX appliqué sur pnex + rauthy."
echo "NB : le CSS thème est servi avec un cache d'1 an — forcer un refresh navigateur."

#!/usr/bin/env bash
# Run canonique LAN — `task rauthy:lan` — à jouer à CHAQUE changement de
# réseau (nouveau spot WiFi, DHCP qui re-numérote) : détecte l'IP de
# l'interface active, réécrit les configs épinglées, redémarre Rauthy +
# backend, puis étend le client OIDC `pnex` EN LIVE via l'API admin.
#
# Pourquoi le patch live est indispensable : les bootstrap/*.json ne sont
# lus qu'à la PREMIÈRE init de la DB Rauthy — modifier clients.json ne
# change PAS un client déjà en base. Les URIs de redirection OAuth sont en
# match EXACT (pas de wildcard possible côté Rauthy) : on AJOUTE donc les
# URIs de la nouvelle IP sans jamais retirer les anciennes — la machine
# fonctionne alors sur tout spot déjà visité, sans sourciller.
#
# Auth — l'API admin Rauthy (0.36) n'accepte qu'une clé API (header
# "API-Key"). En dev, la clé `pnex-branding` (groupe Clients, CRUD) est
# versionnée dans bootstrap/api_keys.json (lue à la première init) ;
# sinon exporter RAUTHY_API_KEY='<nom>$<secret>'.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

# ── 1) IP LAN de l'interface active (route par défaut) ──────────────────
IP=$(ip -4 route get 1.1.1.1 2>/dev/null | grep -oP 'src \K[0-9.]+' | head -1 || true)
[[ -n $IP ]] || IP=$(hostname -I 2>/dev/null | awk '{print $1}')
if [[ ! $IP =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "ERREUR : IP LAN indétectable (route par défaut absente ?)"
    exit 1
fi
echo "IP LAN détectée : $IP"

# ── 2) Réécriture des configs épinglées ──────────────────────────────────
# Un seul sed sur l'IPv4 (les schémas/ports sont hors de l'IP) :
#   compose.yaml                     PUB_URL
#   deploy/rauthy/config.toml        pub_url (miroir hors compose)
#   deploy/rauthy/bootstrap/clients.json   URIs de la PREMIÈRE init
#   crates/pnex-backend/config/development.yaml  défaut issuer_url
FILES=(
    compose.yaml
    deploy/rauthy/config.toml
    deploy/rauthy/bootstrap/clients.json
    crates/pnex-backend/config/development.yaml
)
for f in "${FILES[@]}"; do
    [[ -f $f ]] || { echo "ERREUR : $f introuvable"; exit 1; }
    if grep -qE '192\.168\.[0-9]+\.[0-9]+' "$f"; then
        sed -i -E "s/192\.168\.[0-9]+\.[0-9]+/$IP/g" "$f"
        echo "  ✓ $f → $IP"
    else
        echo "  • $f déjà à jour"
    fi
done

# ── 3) Redémarrages (PUB_URL lu au boot des deux services) ───────────────
docker compose up -d rauthy >/dev/null
echo "  ✓ Rauthy redémarré (PUB_URL=$IP:8443)"

BACKEND_PID=$(pgrep -f 'target/debug/pnex-server start' | head -1 || true)
if [[ -n $BACKEND_PID ]]; then
    kill "$BACKEND_PID"
    sleep 3
    (cd crates/pnex-backend && \
        nohup "$ROOT/target/debug/pnex-server" start --server-and-worker \
        > /tmp/pnex-server.log 2>&1 &)
    echo "  ✓ Backend redémarré (issuer par défaut → $IP)"
else
    echo "  • Backend non lancé — il lira le nouvel issuer à son prochain boot"
fi

# ── 4) Extension LIVE du client OIDC `pnex` ──────────────────────────────
KEY="${RAUTHY_API_KEY:-pnex-branding\$c1cb445debf753d916623fc2e0d996cef9a73f3b4664a9d3d29d16a5e5113e5c}"
BASE="${RAUTHY_URL:-https://localhost:8443}"
AUTH="Authorization: API-Key $KEY"

JSON=$(curl -sk -H "$AUTH" "$BASE/auth/v1/clients/pnex") || {
    echo "ERREUR : GET client pnex impossible (Rauthy up ? clé valide ?)"
    exit 1
}

NEW_JSON=$(IP="$IP" python3 - "$JSON" <<'PY'
import json, os, sys

client = json.loads(sys.argv[1])
ip = os.environ["IP"]
uris = [
    f"http://{ip}:5150/api/v1/oauth2/native",
    f"http://{ip}:5150/auth/callback",
    f"http://{ip}:5151/auth/callback",
]
origins = [f"http://{ip}:5150", f"http://{ip}:5151"]
red = client.get("redirect_uris") or []
# Rauthy valide ^scheme://[a-z0-9.:-]+$ sur allowed_origins : SANS slash.
org = [o.rstrip("/") for o in (client.get("allowed_origins") or [])]
added = 0
for u in uris:
    if u not in red:
        red.append(u)
        added += 1
for o in origins:
    if o not in org:
        org.append(o)
        added += 1
client["redirect_uris"] = red
client["allowed_origins"] = org
print(json.dumps(client))
if added == 0:
    print(f"  • client déjà à jour pour {ip}", file=sys.stderr)
else:
    print(f"  ✓ +{added} URIs/origins pour {ip}", file=sys.stderr)
PY
)

code=$(curl -sk -o /dev/null -w '%{http_code}' -X PUT \
    -H "$AUTH" -H 'Content-Type: application/json' \
    -d "$NEW_JSON" "$BASE/auth/v1/clients/pnex") || code="curl"
if [[ $code != 2* ]]; then
    echo "ERREUR : PUT client pnex → HTTP $code"
    exit 1
fi

# ── 5) Vérification discovery ────────────────────────────────────────────
ISSUER=$(curl -sk "$BASE/auth/v1/.well-known/openid-configuration" \
    | python3 -c "import json,sys; print(json.load(sys.stdin)['issuer'])")
echo "  ✓ discovery issuer : $ISSUER"
[[ $ISSUER == *"$IP"* ]] || {
    echo "ATTENTION : l'issuer ne porte pas la nouvelle IP (Rauthy pas encore"
    echo "reparti avec le nouveau PUB_URL ? rejouer la task dans quelques s)"
}
echo ""
echo "LAN $IP appliqué — spots précédents toujours acceptés (URIs cumulées)."

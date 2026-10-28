#!/usr/bin/env bash
# Canonical LAN run — `task rauthy:lan` — to replay on EVERY network
# change (new WiFi spot, DHCP renumbering): detects the IP of the
# active interface, rewrites the pinned configs, restarts Rauthy +
# backend, then extends the OIDC client `pnex` LIVE through the admin API.
#
# Why the live patch is required: bootstrap/*.json are only read
# on the FIRST init of the Rauthy DB — editing clients.json does
# NOT change a client already in the DB. OAuth redirect URIs are
# EXACT match (no wildcard possible on the Rauthy side): so we ADD the
# URIs of the new IP without ever removing the old ones — the machine
# then works on every spot already visited, without a hitch.
#
# Auth — the Rauthy admin API (0.36) only accepts an API key (header
# "API-Key"). In dev, the `pnex-branding` key (Clients group, CRUD) is
# versioned in bootstrap/api_keys.json (read on first init);
# otherwise export RAUTHY_API_KEY='<name>$<secret>'.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

# ── 1) LAN IP of the active interface (default route) ───────────────────
IP=$(ip -4 route get 1.1.1.1 2>/dev/null | grep -oP 'src \K[0-9.]+' | head -1 || true)
[[ -n $IP ]] || IP=$(hostname -I 2>/dev/null | awk '{print $1}')
if [[ ! $IP =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "ERROR: LAN IP not detectable (no default route?)"
    exit 1
fi
echo "LAN IP detected: $IP"

# ── 2) Rewriting the pinned configs ──────────────────────────────────────
# A single sed on the IPv4 (schemes/ports are outside the IP):
#   compose.yaml                     PUB_URL
#   deploy/rauthy/config.toml        pub_url (mirror outside compose)
#   deploy/rauthy/bootstrap/clients.json   URIs of the FIRST init
#   crates/pnex-backend/config/development.yaml  default issuer_url
FILES=(
    compose.yaml
    deploy/rauthy/config.toml
    deploy/rauthy/bootstrap/clients.json
    crates/pnex-backend/config/development.yaml
)
for f in "${FILES[@]}"; do
    [[ -f $f ]] || { echo "ERROR: $f not found"; exit 1; }
    if grep -qE '192\.168\.[0-9]+\.[0-9]+' "$f"; then
        sed -i -E "s/192\.168\.[0-9]+\.[0-9]+/$IP/g" "$f"
        echo "  ✓ $f → $IP"
    else
        echo "  • $f already up to date"
    fi
done

# ── 3) Restarts (PUB_URL read at boot by both services) ──────────────────
docker compose up -d rauthy >/dev/null
echo "  ✓ Rauthy restarted (PUB_URL=$IP:8443)"

BACKEND_PID=$(pgrep -f 'target/debug/pnex-server start' | head -1 || true)
if [[ -n $BACKEND_PID ]]; then
    kill "$BACKEND_PID"
    sleep 3
    (cd crates/pnex-backend && \
        nohup "$ROOT/target/debug/pnex-server" start --server-and-worker \
        > /tmp/pnex-server.log 2>&1 &)
    echo "  ✓ Backend restarted (default issuer → $IP)"
else
    echo "  • Backend not running — it will read the new issuer on its next boot"
fi

# ── 4) LIVE extension of the OIDC client `pnex` ──────────────────────────
KEY="${RAUTHY_API_KEY:-pnex-branding\$c1cb445debf753d916623fc2e0d996cef9a73f3b4664a9d3d29d16a5e5113e5c}"
BASE="${RAUTHY_URL:-https://localhost:8443}"
AUTH="Authorization: API-Key $KEY"

JSON=$(curl -sk -H "$AUTH" "$BASE/auth/v1/clients/pnex") || {
    echo "ERROR: GET client pnex failed (Rauthy up? valid key?)"
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
# Rauthy validates ^scheme://[a-z0-9.:-]+$ on allowed_origins: NO slash.
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
    print(f"  • client already up to date for {ip}", file=sys.stderr)
else:
    print(f"  ✓ +{added} URIs/origins for {ip}", file=sys.stderr)
PY
)

code=$(curl -sk -o /dev/null -w '%{http_code}' -X PUT \
    -H "$AUTH" -H 'Content-Type: application/json' \
    -d "$NEW_JSON" "$BASE/auth/v1/clients/pnex") || code="curl"
if [[ $code != 2* ]]; then
    echo "ERROR: PUT client pnex → HTTP $code"
    exit 1
fi

# ── 5) Discovery check ───────────────────────────────────────────────────
ISSUER=$(curl -sk "$BASE/auth/v1/.well-known/openid-configuration" \
    | python3 -c "import json,sys; print(json.load(sys.stdin)['issuer'])")
echo "  ✓ discovery issuer: $ISSUER"
[[ $ISSUER == *"$IP"* ]] || {
    echo "WARNING: the issuer does not carry the new IP (Rauthy not yet"
    echo "restarted with the new PUB_URL? replay the task in a few s)"
}
echo ""
echo "LAN $IP applied — previous spots still accepted (cumulated URIs)."

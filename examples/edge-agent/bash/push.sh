#!/bin/sh
# Push values to the local PNeX edge agent with curl.
set -eu
AGENT="${PNEX_AGENT_URL:-http://127.0.0.1:7070}"

# One point with a unit.
curl -fsS -X POST "$AGENT/v1/points" -H 'content-type: application/json' \
  -d '{"key":"temperature","value":21.5,"unit":"°C"}'

# Several values at once (flat object, shared timestamp).
curl -fsS -X POST "$AGENT/v1/points" -H 'content-type: application/json' \
  -d "{\"cpu_load\": $(cut -d' ' -f1 /proc/loadavg), \"uptime_s\": $(cut -d. -f1 /proc/uptime)}"

# Plain text lines, recorded in PNeX history (OpenObserve).
printf 'disk_used_pct=%s\nhostname=%s\n' "$(df --output=pcent / | tail -1 | tr -dc 0-9)" "$(hostname)" |
  curl -fsS -X POST "$AGENT/v1/points?record=true" -H 'content-type: text/plain' --data-binary @-
echo

#!/bin/sh
# Runtime de flow de fixture (tests D18) — remplace pnex-flow-runtime.
#
# Contrat identique au vrai binaire : args `<flows.json> --home <dir>`,
# runtime.json (santé process), événements par flow (`flow_started` émis
# pour TOUS les tabs au boot et à chaque SIGUSR1 — le vrai runtime ré-annonce
# tous ses tabs à chaque cycle, c'est par là que le superviseur acquitte les
# deploys), SIGINT/SIGTERM = sortie propre.
# `--check` = pré-flight : rapport `check_flow` par tab, exit 1 si un tab
# porte le marqueur de test `"type": "pnex-check-fail"`. Le contenu de
# l'artefact projeté reste vérifiable en lisant flows.json.

# ── Args : [--check] <flows.json> [--home <dir>] ─────────────────────────
CHECK=0
if [ "$1" = "--check" ]; then
  CHECK=1
  shift
fi
# Mode test-fonction : réponse canned — le contrat contrôleur (200 ok:true /
# 503 gate) se teste là ; la vraie exécution JS/Starlark est couverte par les
# tests du crate runtime + le smoke binaire.
if [ "$1" = "--test-function" ]; then
  printf '{"ok":true,"outputs":[{"payload":{"echo":true}}],"logs":[],"duration_ms":1}\n'
  exit 0
fi
# Mode validation compile-only : réponse canned `ok:true` — le contrat
# contrôleur (200 ok:true / 503 gate) se teste là ; les vrais diagnostics
# sont couverts par les tests du crate runtime + pnex-node-starlark.
if [ "$1" = "--check-function" ]; then
  printf '{"ok":true,"diagnostics":[]}\n'
  exit 0
fi
FLOWS="$1"
shift
STATE_DIR="./flow-state"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --home) STATE_DIR="$2"; shift 2 ;;
    *) shift ;;
  esac
done
mkdir -p "$STATE_DIR"

# ── Mode pré-flight : `--check <flows.json>` ─────────────────────────────
# Un check_flow par tab ; ok=false si la section du tab (entre son
# `"pnex_flow_id"` et le tab suivant) contient le marqueur de test.
if [ "$CHECK" = 1 ]; then
  out=$(awk -v marker='"pnex-check-fail"' '
    /"pnex_flow_id": *[0-9]+/ {
      if (cur != "") {
        printf "{\"event\":\"check_flow\",\"flow\":%s,\"ok\":%s}\n", cur, (bad ? "false" : "true")
        if (bad) bads++
      }
      cur = $0; sub(/.*"pnex_flow_id": */, "", cur); sub(/[},].*/, "", cur)
      bad = 0; n++
      next
    }
    $0 ~ marker && cur != "" { bad = 1 }
    END {
      if (cur != "") {
        printf "{\"event\":\"check_flow\",\"flow\":%s,\"ok\":%s}\n", cur, (bad ? "false" : "true")
        if (bad) bads++
      }
      printf "{\"event\":\"check_done\",\"ok\":%s,\"total\":%d,\"errors\":%d}\n", (bads > 0 ? "false" : "true"), n, bads + 0
    }
  ' "$FLOWS")
  printf '%s\n' "$out"
  exit $(printf '%s\n' "$out" | grep -c '"ok":false')
fi

# ── Mode serveur ─────────────────────────────────────────────────────────
count=0
write_state() {
  printf '{"pid":%s,"running":true,"started_at":0,"redeploys":%s,"flow_rev":null}\n' \
    "$$" "$count" > "$STATE_DIR/runtime.json.tmp"
  mv "$STATE_DIR/runtime.json.tmp" "$STATE_DIR/runtime.json"
}

# flow_started pour TOUS les tabs de l'artefact (boot et rechargements) —
# sort -u : les nœuds custom portent aussi pnex_flow_id.
flow_ids() {
  sed -n 's/.*"pnex_flow_id": *\([0-9]*\).*/\1/p' "$FLOWS" | sort -u
}

emit_flow_started_all() {
  for f in $(flow_ids); do
    printf '{"event":"flow_started","flow":%s,"tab":"pnexflow%s","rev":"fixture"}\n' "$f" "$f"
  done
}

# Tabs connus au cycle précédent (persistés dans STATE_DIR) : émettre
# flow_stopped pour les ids disparus, comme le vrai runtime (ferme — le
# retrait d'un tab de l'artefact stoppe cet engine, les autres continuent).
sync_tabs() {
  if [ -f "$STATE_DIR/tabs.prev" ]; then
    for f in $(cat "$STATE_DIR/tabs.prev"); do
      flow_ids | grep -qx "$f" || \
        printf '{"event":"flow_stopped","flow":%s,"tab":"pnexflow%s"}\n' "$f" "$f"
    done
  fi
  flow_ids > "$STATE_DIR/tabs.prev"
}

on_usr1() {
  count=$((count + 1))
  write_state
  sync_tabs
  emit_flow_started_all
  # Ligne debug immédiate après rechargement : le feed du panneau reflète
  # l'artefact frais sans attendre le tick suivant.
  printf '{"event":"debug","node":"deadbeef","node_red":"n2","flow":%s,"name":"n2","msg":"reload","msgid":"m1"}\n' "$(flow_ids | head -1)"
}

trap on_usr1 USR1
trap 'exit 0' INT TERM HUP

write_state
sync_tabs
emit_flow_started_all
printf '{"event":"fixture_started"}\n'
# Ligne debug d'amorce : permet au feed du panneau d'être non vide dès le
# deploy (le seq backend, le ts et l'attribution `flow` viennent du
# superviseur — honnêtes : flow dérivé de l'artefact).
printf '{"event":"debug","node":"deadbeef","node_red":"n2","flow":%s,"name":"n2","msg":"bonjour","msgid":"m1"}\n' "$(flow_ids | head -1)"
while :; do
  sleep 1
  printf '{"event":"debug","node":"deadbeef","node_red":"n2","flow":%s,"name":"n2","msg":"tick","msgid":"m1"}\n' "$(flow_ids | head -1)"
done

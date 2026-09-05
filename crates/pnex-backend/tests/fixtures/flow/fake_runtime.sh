#!/bin/sh
# Runtime de flow de fixture (tests D18) — remplace pnex-flow-runtime.
#
# Contrat identique au vrai binaire : args `<flows.json> --home <dir>`,
# état écrit dans `<home>/runtime.json`, SIGUSR1 = rechargement (ici :
# incrément du compteur `redeploys`), SIGUSR2 + cmd.json = run-once (ack
# `run_once_done` avec écho du seq), SIGINT/SIGTERM = sortie propre.
# Le contenu de l'artefact projeté reste vérifiable en lisant flows.json.

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

# Flow de l'artefact courant, dérivé du tab (attribution honnête).
flow=$(sed -n 's/.*"pnex_flow_id": *\([0-9]*\).*/\1/p' "$FLOWS" | head -1)

count=0
write_state() {
  printf '{"pid":%s,"running":true,"started_at":0,"redeploys":%s,"flow_rev":null,"flow_id":null,"version_number":null}\n' \
    "$$" "$count" > "$STATE_DIR/runtime.json.tmp"
  mv "$STATE_DIR/runtime.json.tmp" "$STATE_DIR/runtime.json"
}
on_usr1() {
  count=$((count + 1))
  write_state
  # Re-dérive le flow depuis l'artefact rechargé.
  flow=$(sed -n 's/.*"pnex_flow_id": *\([0-9]*\).*/\1/p' "$FLOWS" | head -1)
  # Ligne debug immédiate après rechargement : le feed du panneau reflète
  # l'artefact frais sans attendre le tick suivant.
  printf '{"event":"debug","node":"deadbeef","node_red":"n2","flow":%s,"name":"n2","msg":"reload","msgid":"m1"}\n' "${flow:-1}"
}
on_usr2() {
  seq=$(sed -n 's/.*"seq": *\([0-9]*\).*/\1/p' "$STATE_DIR/cmd.json" 2>/dev/null | head -1)
  if [ -n "$seq" ]; then
    printf '{"event":"run_once_done","seq":%s,"flow":"pnexflow%s","nodes":1,"injected":1}\n' "$seq" "$flow"
    rm -f "$STATE_DIR/cmd.json"
  else
    printf '{"event":"run_once_failed","seq":0,"flow":"pnexflow%s","error":"cmd_illisible"}\n' "$flow"
  fi
}
trap on_usr1 USR1
trap on_usr2 USR2
trap 'exit 0' INT TERM HUP

write_state
printf '{"event":"fixture_started"}\n'
# Ligne debug d'amorce : permet au feed du panneau d'être non vide dès le
# deploy (le seq backend, le ts et l'attribution `flow` viennent du
# superviseur — honnêtes : flow dérivé de l'artefact).
printf '{"event":"debug","node":"deadbeef","node_red":"n2","flow":%s,"name":"n2","msg":"bonjour","msgid":"m1"}\n' "${flow:-1}"
while :; do
  sleep 1
  printf '{"event":"debug","node":"deadbeef","node_red":"n2","flow":%s,"name":"n2","msg":"tick","msgid":"m1"}\n' "${flow:-1}"
done

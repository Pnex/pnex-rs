#!/bin/sh
# Runtime de flow de fixture (tests D18) — remplace pnex-flow-runtime.
#
# Contrat identique au vrai binaire : args `<flows.json> --home <dir>`,
# état écrit dans `<home>/runtime.json`, SIGUSR1 = rechargement (ici :
# incrément du compteur `redeploys`), SIGINT/SIGTERM = sortie propre.
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

count=0
write_state() {
  printf '{"pid":%s,"running":true,"started_at":0,"redeploys":%s,"flow_rev":null,"flow_id":null,"version_number":null}\n' \
    "$$" "$count" > "$STATE_DIR/runtime.json.tmp"
  mv "$STATE_DIR/runtime.json.tmp" "$STATE_DIR/runtime.json"
}
on_usr1() {
  count=$((count + 1))
  write_state
}
trap on_usr1 USR1
trap 'exit 0' INT TERM HUP

write_state
printf '{"event":"fixture_started"}\n'
while :; do
  sleep 1
done

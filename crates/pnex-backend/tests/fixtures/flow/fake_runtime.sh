#!/bin/sh
# Fixture flow runtime (D18 tests) — replaces pnex-flow-runtime.
#
# Same contract as the real binary: args `<flows.json> --home <dir>`,
# runtime.json (process health), per-flow events (`flow_started` emitted
# for ALL tabs at boot and on each SIGUSR1 — the real runtime re-announces
# all its tabs on each cycle, that is how the supervisor acknowledges
# deploys), SIGINT/SIGTERM = clean exit.
# `--check` = pre-flight: `check_flow` report per tab, exit 1 if a tab
# carries the test marker `"pnex-check-fail"` (any string value). The content of
# the projected artifact stays checkable by reading flows.json.

# ── Args : [--check] <flows.json> [--home <dir>] ─────────────────────────
CHECK=0
if [ "$1" = "--check" ]; then
  CHECK=1
  shift
fi
# Function-test mode: canned reply — the controller contract (200 ok:true /
# 503 gate) is tested here; the real JS/Starlark execution is covered by the
# runtime crate tests + the binary smoke test.
if [ "$1" = "--test-function" ]; then
  printf '{"ok":true,"outputs":[{"payload":{"echo":true}}],"logs":[],"duration_ms":1}\n'
  exit 0
fi
# Compile-only validation mode: canned `ok:true` reply — the controller
# contract (200 ok:true / 503 gate) is tested here; the real diagnostics
# are covered by the runtime crate tests + pnex-node-starlark.
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

# ── Pre-flight mode: `--check <flows.json>` ─────────────────────────────
# One check_flow per tab; ok=false if the tab's section (between its
# `"pnex_flow_id"` and the next tab) contains the test marker.
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

# flow_started for ALL tabs of the artifact (boot and reloads) —
# sort -u: custom nodes carry pnex_flow_id too.
flow_ids() {
  sed -n 's/.*"pnex_flow_id": *\([0-9]*\).*/\1/p' "$FLOWS" | sort -u
}

emit_flow_started_all() {
  for f in $(flow_ids); do
    printf '{"event":"flow_started","flow":%s,"tab":"pnexflow%s","rev":"fixture"}\n' "$f" "$f"
  done
}

# Tabs known at the previous cycle (persisted in STATE_DIR): emit
# flow_stopped for the vanished ids, like the real runtime (ferme —
# removing a tab from the artifact stops that engine, the others go on).
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
  # Immediate debug line after a reload: the panel feed reflects the
  # fresh artifact without waiting for the next tick.
  printf '{"event":"debug","node":"deadbeef","node_red":"n2","flow":%s,"name":"n2","msg":"reload","msgid":"m1"}\n' "$(flow_ids | head -1)"
}

trap on_usr1 USR1
trap 'exit 0' INT TERM HUP

write_state
sync_tabs
emit_flow_started_all
printf '{"event":"fixture_started"}\n'
# Priming debug line: lets the panel feed be non-empty right from the
# deploy (the backend seq, the ts and the `flow` attribution come from the
# supervisor — honest: flow derived from the artifact).
printf '{"event":"debug","node":"deadbeef","node_red":"n2","flow":%s,"name":"n2","msg":"bonjour","msgid":"m1"}\n' "$(flow_ids | head -1)"
while :; do
  sleep 1
  printf '{"event":"debug","node":"deadbeef","node_red":"n2","flow":%s,"name":"n2","msg":"tick","msgid":"m1"}\n' "$(flow_ids | head -1)"
done

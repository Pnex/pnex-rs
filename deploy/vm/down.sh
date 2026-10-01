#!/usr/bin/env bash
# Stop the test VM gracefully (ACPI power button through QMP), keeping its
# disk. deploy/vm/down.sh [--arch arm64|amd64]
set -euo pipefail
# shellcheck source=deploy/vm/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

REQ=""
[[ ${1:-} == --arch ]] && REQ=${2:-}
[[ ${1:-} == --arch=* ]] && REQ=${1#*=}
ARCH=$(resolve_arch "$REQ")
STATE=$(vm_state_dir "$ARCH")
pid=$(vm_pid "$ARCH") || { ok "$ARCH VM is not running"; exit 0; }

if [[ -S $(vm_qmp_sock "$ARCH") ]] && command -v python3 >/dev/null; then
    # ACPI power button: the guest shuts down cleanly (no fsck next boot).
    python3 - "$(vm_qmp_sock "$ARCH")" <<'PY' || true
import socket, sys
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
f = s.makefile("rw")
f.readline()
for cmd in ('{"execute":"qmp_capabilities"}', '{"execute":"system_powerdown"}'):
    f.write(cmd + "\n"); f.flush(); f.readline()
PY
else
    vm_ssh "$ARCH" 'sudo systemctl poweroff' >/dev/null 2>&1 || true
fi
info "waiting for $ARCH VM (pid $pid) to power off"
for _ in $(seq 1 120); do
    kill -0 "$pid" 2>/dev/null || { ok "$ARCH VM stopped"; rm -f "$STATE/qemu.pid"; exit 0; }
    sleep 1
done
kill "$pid" 2>/dev/null || true
rm -f "$STATE/qemu.pid"
ok "$ARCH VM killed after a 120 s grace period"

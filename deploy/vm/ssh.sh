#!/usr/bin/env bash
# SSH into the test VM: deploy/vm/ssh.sh [--arch arm64|amd64] [command...]
set -euo pipefail
# shellcheck source=deploy/vm/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

REQ=""
if [[ ${1:-} == --arch ]]; then REQ=${2:?}; shift 2; fi
if [[ ${1:-} == --arch=* ]]; then REQ=${1#*=}; shift; fi
ARCH=$(resolve_arch "$REQ")
vm_pid "$ARCH" >/dev/null || [[ $(vm_ssh_host "$ARCH") != 127.0.0.1 ]] ||
    die "$ARCH VM is not running (deploy/vm/up.sh --arch $ARCH)"
if (($#)); then
    vm_ssh "$ARCH" "$@"
else
    opts=()
    mapfile -t opts < <(ssh_opts "$ARCH")
    exec ssh -t "${opts[@]}" "$VM_USER@$(vm_ssh_host "$ARCH")"
fi

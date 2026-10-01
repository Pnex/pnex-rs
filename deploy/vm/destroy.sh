#!/usr/bin/env bash
# Stop the test VM and delete its disk + cloud-init seed (the downloaded
# base image is kept). deploy/vm/destroy.sh [--arch arm64|amd64] [--all]
#   --all  also delete the cached base images and the VM SSH key.
set -euo pipefail
# shellcheck source=deploy/vm/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

REQ="" ALL=0
while (($#)); do
    case $1 in
        --arch) REQ=${2:?}; shift ;;
        --arch=*) REQ=${1#*=} ;;
        --all) ALL=1 ;;
        *) die "unknown argument '$1'" ;;
    esac
    shift
done
ARCH=$(resolve_arch "$REQ")
bash "$VM_DIR/down.sh" --arch "$ARCH"
rm -rf "$(vm_state_dir "$ARCH")"
ok "$ARCH VM disk and seed deleted"
if ((ALL)); then
    rm -f "$VM_CACHE"/debian-13-genericcloud-*.qcow2 "$VM_CACHE"/id_ed25519*
    ok "base images and VM key deleted"
fi

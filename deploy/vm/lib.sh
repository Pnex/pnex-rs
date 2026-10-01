#!/usr/bin/env bash
# Shared helpers for the PNeX VM test harness (sourced, not executed).
# A VM simulates the target Raspberry Pi: Debian 13 (trixie) generic cloud
# image, cloud-init seeded, one VM per architecture under $VM_CACHE/<arch>/.

# Variables below are used by the scripts sourcing this file.
# shellcheck disable=SC2034
VM_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$VM_DIR/../.." && pwd)"
# Downloaded base images + per-VM state (gitignored). Override to keep the
# large images outside the repository.
VM_CACHE="${PNEX_VM_CACHE:-$VM_DIR/.cache}"
VM_HOSTNAME="${PNEX_VM_HOSTNAME:-pnex-pi}"
VM_USER="${PNEX_VM_USER:-pnex}"
DEBIAN_IMAGE_BASE="${PNEX_VM_IMAGE_BASE:-https://cloud.debian.org/images/cloud/trixie/latest}"

die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
info() { printf '  • %s\n' "$*"; }
ok() { printf '  \e[32m✓\e[0m %s\n' "$*"; }

vm_state_dir() { echo "$VM_CACHE/$1"; }

vm_pid() { # $1 = arch — prints the qemu pid when running
    local pidf
    pidf="$(vm_state_dir "$1")/qemu.pid"
    [[ -f $pidf ]] || return 1
    local pid
    pid=$(<"$pidf")
    kill -0 "$pid" 2>/dev/null || return 1
    echo "$pid"
}

# The architecture to act on: explicit --arch, else the only running VM,
# else arm64 (the faithful Pi simulation).
resolve_arch() { # $1 = requested arch (may be empty)
    if [[ -n $1 ]]; then
        [[ $1 == arm64 || $1 == amd64 ]] || die "--arch must be arm64 or amd64"
        echo "$1"
        return
    fi
    local running=() a
    for a in arm64 amd64; do
        vm_pid "$a" >/dev/null && running+=("$a")
    done
    if ((${#running[@]} == 1)); then
        echo "${running[0]}"
    elif ((${#running[@]} > 1)); then
        die "both VMs are running: pass --arch"
    else
        echo arm64
    fi
}

vm_ssh_port() { cat "$(vm_state_dir "$1")/ssh_port" 2>/dev/null || echo 2222; }
vm_ssh_host() { cat "$(vm_state_dir "$1")/ssh_host" 2>/dev/null || echo 127.0.0.1; }
vm_key() { echo "$VM_CACHE/id_ed25519"; }

ssh_opts() { # $1 = arch
    printf '%s\n' -i "$(vm_key)" -p "$(vm_ssh_port "$1")" \
        -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
        -o LogLevel=ERROR -o ConnectTimeout=5 -o SetEnv=LC_ALL=C.UTF-8
}

vm_ssh() { # $1 = arch, rest = remote command
    local arch=$1
    shift
    local opts
    mapfile -t opts < <(ssh_opts "$arch")
    # shellcheck disable=SC2029 # the command is meant to run remotely as given
    ssh "${opts[@]}" "$VM_USER@$(vm_ssh_host "$arch")" "$@"
}

vm_scp() { # $1 = arch, $2 = local path, $3 = remote path
    local opts
    mapfile -t opts < <(ssh_opts "$1")
    opts=("${opts[@]/#-p/-P}")
    scp -q -r "${opts[@]}" "$2" "$VM_USER@$(vm_ssh_host "$1"):$3"
}

# QMP control socket: kept short (unix socket paths are capped at 108 bytes,
# the cache dir can be deep).
vm_qmp_sock() { echo "${XDG_RUNTIME_DIR:-/tmp}/pnex-vm-$1.qmp"; }

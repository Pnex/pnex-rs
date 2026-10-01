#!/usr/bin/env bash
# qemu options are comma-separated by design; remote commands are single-quoted on purpose.
# shellcheck disable=SC2054,SC2016
# Boot a Raspberry-Pi-like test VM (Debian 13 generic cloud image +
# cloud-init) with plain qemu — no Vagrant, no libvirt.
#
#   deploy/vm/up.sh [--arch arm64|amd64] [--usb VID:PID]... [options]
#
#   --arch arm64    (default) qemu-system-aarch64 -M virt, cortex-a72 (Pi 4
#                   core), TCG emulation on an x86 host: faithful but slow
#                   (first boot ~5-10 min, image pulls much longer).
#   --arch amd64    KVM, fast: iterate on the installer.
#   --cpus N / --mem MB / --disk SIZE     default 4 / 4096 / 32G (a Pi 4 4GB)
#   --ssh-port N    host port forwarded to the VM's 22 (default 2222)
#   --https-port N  host port forwarded to 443 (default 18443)
#   --http-port N   host port forwarded to 80 (default 18080)
#   --guest-https-port N
#                   VM port behind --https-port (default 443). Use 8443 with an
#                   install made with `--domain localhost --https-port 8443` to
#                   log in from the host browser at https://localhost:8443.
#   --net user|bridge:BR
#                   user (default): NAT + the port forwards above.
#                   bridge:BR: the VM joins host bridge BR and gets its own LAN
#                   address (real ESP devices can reach it) — see README.md.
#   --usb VID:PID   pass a USB device through (repeatable), e.g. 10c4:ea60.
#   --console       stay attached to the serial console (no daemon mode).
#
# Host packages (Ubuntu): sudo apt install qemu-system-x86 qemu-utils cloud-image-utils
#   arm64 also needs:     sudo apt install qemu-system-arm qemu-efi-aarch64
set -euo pipefail
# shellcheck source=deploy/vm/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

ARCH="" CPUS=4 MEM=4096 DISK=32G SSH_PORT=2222 HTTPS_PORT=18443 HTTP_PORT=18080 GUEST_HTTPS=443
NET=user CONSOLE=0
USB=()
while (($#)); do
    case $1 in
        --arch) ARCH=${2:?}; shift ;;
        --arch=*) ARCH=${1#*=} ;;
        --cpus) CPUS=${2:?}; shift ;;
        --mem) MEM=${2:?}; shift ;;
        --disk) DISK=${2:?}; shift ;;
        --ssh-port) SSH_PORT=${2:?}; shift ;;
        --https-port) HTTPS_PORT=${2:?}; shift ;;
        --http-port) HTTP_PORT=${2:?}; shift ;;
        --guest-https-port) GUEST_HTTPS=${2:?}; shift ;;
        --net) NET=${2:?}; shift ;;
        --net=*) NET=${1#*=} ;;
        --usb) USB+=("${2:?}"); shift ;;
        --usb=*) USB+=("${1#*=}") ;;
        --console) CONSOLE=1 ;;
        -h | --help) awk 'NR > 3 && /^#/ {sub(/^# ?/, ""); print; next} NR > 3 {exit}' "$0"; exit 0 ;;
        *) die "unknown argument '$1' (see --help)" ;;
    esac
    shift
done
ARCH=${ARCH:-arm64}
[[ $ARCH == arm64 || $ARCH == amd64 ]] || die "--arch must be arm64 or amd64"

STATE=$(vm_state_dir "$ARCH")
if pid=$(vm_pid "$ARCH"); then
    ok "$ARCH VM already running (pid $pid) — ssh: deploy/vm/ssh.sh --arch $ARCH"
    exit 0
fi

# ── Host tooling ────────────────────────────────────────────────────────
need() { command -v "$1" >/dev/null || die "$1 not found — $2"; }
need qemu-img "sudo apt install qemu-utils"
need cloud-localds "sudo apt install cloud-image-utils"
if [[ $ARCH == amd64 ]]; then
    need qemu-system-x86_64 "sudo apt install qemu-system-x86"
    QEMU=qemu-system-x86_64
else
    need qemu-system-aarch64 "sudo apt install qemu-system-arm qemu-efi-aarch64"
    QEMU=qemu-system-aarch64
    EFI=""
    for f in /usr/share/qemu-efi-aarch64/QEMU_EFI.fd /usr/share/AAVMF/AAVMF_CODE.fd \
        /usr/share/edk2/aarch64/QEMU_EFI.fd; do
        [[ -f $f ]] && { EFI=$f; break; }
    done
    [[ -n $EFI ]] || die "no aarch64 UEFI firmware — sudo apt install qemu-efi-aarch64"
fi

port_free() { ! ss -ltnH "sport = :$1" 2>/dev/null | grep -q .; }
if [[ $NET == user ]]; then
    for p in "$SSH_PORT" "$HTTPS_PORT" "$HTTP_PORT"; do
        port_free "$p" || die "host port $p is busy (the dev stack uses 8080/8443?) — pick others: --ssh-port/--https-port/--http-port"
    done
fi

mkdir -p "$STATE"
# ── Base image + overlay ────────────────────────────────────────────────
IMAGE="debian-13-genericcloud-$ARCH.qcow2"
BASE="$VM_CACHE/$IMAGE"
if [[ ! -s $BASE ]]; then
    info "downloading $IMAGE (~400 MB, once)"
    curl -fL --progress-bar -o "$BASE.part" "$DEBIAN_IMAGE_BASE/$IMAGE"
    mv "$BASE.part" "$BASE"
fi
DISK_FILE="$STATE/disk.qcow2"
if [[ ! -f $DISK_FILE ]]; then
    qemu-img create -q -f qcow2 -b "$BASE" -F qcow2 "$DISK_FILE" "$DISK"
    ok "overlay disk $DISK_FILE ($DISK, base untouched)"
fi

# ── cloud-init seed ─────────────────────────────────────────────────────
KEY=$(vm_key)
[[ -f $KEY ]] || ssh-keygen -q -t ed25519 -N "" -C "pnex-vm" -f "$KEY"
keys=("$(cat "$KEY.pub")")
for pub in "$HOME"/.ssh/id_*.pub; do
    [[ -f $pub ]] && keys+=("$(cat "$pub")")
done
USER_DATA="$STATE/user-data"
{
    echo "#cloud-config"
    echo "hostname: $VM_HOSTNAME"
    echo "fqdn: $VM_HOSTNAME"
    echo "manage_etc_hosts: true"
    echo "users:"
    echo "  - name: $VM_USER"
    echo "    groups: [sudo, dialout]"
    echo "    shell: /bin/bash"
    echo "    sudo: ALL=(ALL) NOPASSWD:ALL"
    echo "    ssh_authorized_keys:"
    for k in "${keys[@]}"; do echo "      - $k"; done
    echo "ssh_pwauth: false"
    echo "package_update: false"
    echo "growpart: {mode: auto, devices: ['/']}"
    echo "timezone: UTC"
} >"$USER_DATA"
printf 'instance-id: pnex-%s-%s\nlocal-hostname: %s\n' "$ARCH" "$(date +%s)" "$VM_HOSTNAME" >"$STATE/meta-data"
# Keep the instance-id stable across restarts (cloud-init runs once).
[[ -f $STATE/seed.iso ]] || cloud-localds "$STATE/seed.iso" "$USER_DATA" "$STATE/meta-data"

# ── qemu command line ───────────────────────────────────────────────────
args=(-name "pnex-$ARCH" -smp "$CPUS" -m "$MEM"
    -drive "file=$DISK_FILE,if=virtio,format=qcow2,discard=unmap"
    -drive "file=$STATE/seed.iso,if=virtio,format=raw,readonly=on"
    -device virtio-rng-pci
    -qmp "unix:$(vm_qmp_sock "$ARCH"),server,nowait"
    -pidfile "$STATE/qemu.pid")
if [[ $ARCH == amd64 ]]; then
    if [[ -r /dev/kvm && -w /dev/kvm ]]; then
        args+=(-machine q35,accel=kvm -cpu host)
    else
        info "no /dev/kvm access: TCG emulation (slow) — add yourself to the kvm group"
        args+=(-machine q35,accel=tcg -cpu max)
    fi
else
    if [[ $(uname -m) == aarch64 && -r /dev/kvm ]]; then
        args+=(-machine virt,accel=kvm -cpu host)
    else
        # Pi 4 = 4x Cortex-A72. Multi-threaded TCG uses one host thread per vCPU.
        args+=(-machine virt -accel tcg,thread=multi -cpu cortex-a72)
    fi
    args+=(-bios "$EFI")
fi

case $NET in
    user)
        args+=(-netdev "user,id=n0,hostfwd=tcp:127.0.0.1:$SSH_PORT-:22,hostfwd=tcp:127.0.0.1:$HTTPS_PORT-:$GUEST_HTTPS,hostfwd=tcp:127.0.0.1:$HTTP_PORT-:80"
            -device virtio-net-pci,netdev=n0)
        echo "$SSH_PORT" >"$STATE/ssh_port"
        echo 127.0.0.1 >"$STATE/ssh_host"
        ;;
    bridge:*)
        BR=${NET#bridge:}
        # qemu-bridge-helper must be allowed to use the bridge:
        # echo "allow $BR" | sudo tee -a /etc/qemu/bridge.conf
        MAC="52:54:00:$(printf '%02x:%02x:%02x' $((RANDOM % 256)) $((RANDOM % 256)) $((RANDOM % 256)))"
        [[ -f $STATE/mac ]] && MAC=$(<"$STATE/mac") || echo "$MAC" >"$STATE/mac"
        args+=(-netdev "bridge,id=n0,br=$BR" -device "virtio-net-pci,netdev=n0,mac=$MAC")
        echo 22 >"$STATE/ssh_port"
        echo "$VM_HOSTNAME.local" >"$STATE/ssh_host"
        info "bridged on $BR (MAC $MAC): ssh goes to $VM_HOSTNAME.local:22 (mDNS)"
        ;;
    *) die "--net must be user or bridge:<bridge>" ;;
esac

if ((${#USB[@]})); then
    args+=(-device qemu-xhci,id=xhci)
    for dev in "${USB[@]}"; do
        [[ $dev =~ ^([0-9a-fA-F]{4}):([0-9a-fA-F]{4})$ ]] || die "--usb expects VID:PID (got '$dev')"
        args+=(-device "usb-host,bus=xhci.0,vendorid=0x${BASH_REMATCH[1]},productid=0x${BASH_REMATCH[2]}")
        # qemu opens /dev/bus/usb/BBB/DDD itself: the node must be writable by
        # this user (udev rule, see README.md) or qemu silently skips it.
        info "USB passthrough $dev (needs write access to its /dev/bus/usb node)"
    done
fi

if ((CONSOLE)); then
    exec "$QEMU" "${args[@]}" -nographic
fi
"$QEMU" "${args[@]}" -display none -serial "file:$STATE/serial.log" -daemonize
ok "$ARCH VM started (pid $(<"$STATE/qemu.pid"), serial log: $STATE/serial.log)"

# ── Wait for SSH (cloud-init done) ──────────────────────────────────────
budget=180
[[ $ARCH == arm64 && $(uname -m) != aarch64 ]] && budget=1200
info "waiting for SSH + cloud-init (up to ${budget}s)"
deadline=$((SECONDS + budget))
until vm_ssh "$ARCH" 'cloud-init status --wait >/dev/null 2>&1; true' 2>/dev/null; do
    ((SECONDS < deadline)) || die "VM not reachable over SSH — see $STATE/serial.log"
    sleep 5
done
ok "VM ready: $(vm_ssh "$ARCH" 'echo "$(hostname) $(uname -m) $(. /etc/os-release; echo "$PRETTY_NAME")"')"
cat <<EOF

  ssh:       deploy/vm/ssh.sh --arch $ARCH
  install:   deploy/vm/install-in-vm.sh --arch $ARCH            (local pnex-deploy checkout)
EOF
if [[ $NET == user ]]; then
    echo "  web:       https://localhost:$HTTPS_PORT (forwarded; OIDC redirects need the VM's own name, see README.md)"
fi

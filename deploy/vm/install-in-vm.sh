#!/usr/bin/env bash
# Run the PNeX production installer (github.com/Pnex/pnex-deploy) inside the
# test VM started by deploy/vm/up.sh.
#
#   deploy/vm/install-in-vm.sh [--arch A] [--source=local|remote] [options] [-- INSTALL_FLAGS...]
#
#   --source=local   (default) copy a pnex-deploy checkout into the VM and run
#                    its install.sh with --source=local: validates the recipe
#                    BEFORE it is pushed.
#   --deploy-dir D   checkout to copy (default: ../pnex-deploy next to this repo)
#   --source=remote  run the real one-liner from raw.githubusercontent.com
#                    (--ref R picks the branch/tag, default main).
#   --load-images TAG
#                    docker-save local images into the VM, tagged as the
#                    published names at TAG, then install with --no-pull --tag
#                    TAG (images not published yet). Sources default to
#                    pnex-server:dev / pnex-builder:dev (`task docker:build`),
#                    override with --server-image / --builder-image. Their
#                    architecture must match the VM's.
#
# INSTALL_FLAGS default to a throw-away admin:
#   --non-interactive --admin-user admin@example.com --admin-password pnex-vm-admin
set -euo pipefail
# shellcheck source=deploy/vm/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

REQ="" SOURCE=local DEPLOY_DIR="$(cd "$REPO_ROOT/.." && pwd)/pnex-deploy" REF=main
LOAD_TAG="" SERVER_IMAGE=pnex-server:dev BUILDER_IMAGE=pnex-builder:dev
REGISTRY=docker.io/shanisma
INSTALL_ARGS=()
while (($#)); do
    case $1 in
        --arch) REQ=${2:?}; shift ;;
        --arch=*) REQ=${1#*=} ;;
        --source) SOURCE=${2:?}; shift ;;
        --source=*) SOURCE=${1#*=} ;;
        --deploy-dir) DEPLOY_DIR=${2:?}; shift ;;
        --deploy-dir=*) DEPLOY_DIR=${1#*=} ;;
        --ref) REF=${2:?}; shift ;;
        --ref=*) REF=${1#*=} ;;
        --load-images) LOAD_TAG=${2:?}; shift ;;
        --load-images=*) LOAD_TAG=${1#*=} ;;
        --server-image) SERVER_IMAGE=${2:?}; shift ;;
        --builder-image) BUILDER_IMAGE=${2:?}; shift ;;
        --) shift; INSTALL_ARGS=("$@"); break ;;
        -h | --help) sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument '$1' (install flags go after --)" ;;
    esac
    shift
done
((${#INSTALL_ARGS[@]})) ||
    INSTALL_ARGS=(--non-interactive --admin-user admin@example.com --admin-password pnex-vm-admin)

ARCH=$(resolve_arch "$REQ")
vm_pid "$ARCH" >/dev/null || [[ $(vm_ssh_host "$ARCH") != 127.0.0.1 ]] ||
    die "$ARCH VM is not running (deploy/vm/up.sh --arch $ARCH)"
vm_ssh "$ARCH" true || die "cannot SSH into the $ARCH VM"

# Quote every argument for the remote shell.
quoted() { local a out=""; for a in "$@"; do out+=" $(printf '%q' "$a")"; done; printf '%s' "$out"; }

if [[ -n $LOAD_TAG ]]; then
    echo "==> Loading local images into the VM as $REGISTRY/pnex-{server,builder}-rs:$LOAD_TAG"
    want=$ARCH
    for img in "$SERVER_IMAGE" "$BUILDER_IMAGE"; do
        have=$(docker image inspect -f '{{.Architecture}}' "$img" 2>/dev/null) ||
            die "local image $img not found (task docker:build)"
        [[ $have == "$want" ]] || die "$img is $have, the VM is $want"
    done
    vm_ssh "$ARCH" 'command -v docker >/dev/null || curl -fsSL https://get.docker.com | sudo sh >/dev/null'
    for pair in "$SERVER_IMAGE:pnex-server-rs" "$BUILDER_IMAGE:pnex-builder-rs"; do
        src=${pair%:*}
        name=${pair##*:}
        info "$src -> $REGISTRY/$name:$LOAD_TAG ($(docker image inspect -f '{{.Size}}' "$src" | awk '{printf "%.0f MB", $1 / 1e6}'))"
        docker save "$src" | gzip -1 | vm_ssh "$ARCH" "gunzip | sudo docker load -q && sudo docker tag $(printf '%q' "$src") $(printf '%q' "$REGISTRY/$name:$LOAD_TAG")"
    done
    INSTALL_ARGS+=(--no-pull --tag "$LOAD_TAG")
fi

case $SOURCE in
    local)
        [[ -f $DEPLOY_DIR/install.sh && -f $DEPLOY_DIR/compose.yaml ]] ||
            die "no pnex-deploy checkout in $DEPLOY_DIR (--deploy-dir)"
        echo "==> Copying $DEPLOY_DIR into the VM"
        tar -C "$DEPLOY_DIR" --exclude=.git -czf - . |
            vm_ssh "$ARCH" 'rm -rf ~/pnex-deploy && mkdir -p ~/pnex-deploy && tar -C ~/pnex-deploy -xzf -'
        echo "==> sudo bash ~/pnex-deploy/install.sh --source=local$(quoted "${INSTALL_ARGS[@]}")"
        vm_ssh "$ARCH" "sudo bash ~/pnex-deploy/install.sh --source=local$(quoted "${INSTALL_ARGS[@]}")"
        ;;
    remote)
        url="https://raw.githubusercontent.com/Pnex/pnex-deploy/$REF/install.sh"
        echo "==> curl -fsSL $url | sudo bash -s --$(quoted --ref "$REF" "${INSTALL_ARGS[@]}")"
        vm_ssh "$ARCH" "curl -fsSL $(printf '%q' "$url") | sudo bash -s --$(quoted --ref "$REF" "${INSTALL_ARGS[@]}")"
        ;;
    *) die "--source must be local or remote" ;;
esac
